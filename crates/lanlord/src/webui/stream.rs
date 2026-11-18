//! `GET /api/events?topic=...`: live events for the browser as server-sent events. a page that
//! cannot hold a thread or a socket of its own opens one `EventSource` per topic and gets the
//! monitor samples (the Task Manager and the tray) or the progress of an apply pushed to it.
//!
//! the rules are those of `/api/operations`: a login, a request that comes from the page itself and the
//! permission the subscription needs (`Op::requires`), read from the account's real roles and
//! its live group membership (see [`Roster`]) when the stream opens and at every recheck. a
//! stream keeps checking, so ending the session (logout, idle, a revoked user) or losing the
//! permission ends the stream within a few seconds. nothing in the stream path counts as
//! activity (opening, rechecking), so the idle logout still fires.
//!
//! nothing queues without bound: a stream holds a few events for its client, and a client that
//! does not read them is dropped (it reconnects and starts from the newest data); the apply
//! events come through a small broadcast buffer that drops a stream that falls behind; the number
//! of streams is capped, per user and in all.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, MethodRouter};
use axum::Extension;
use futures_core::Stream;
use lanlord_core::monitor::Sampler;
use lanlord_model::apply::ApplyEvent;
use lanlord_model::ops::{topic, Op};
use tokio::sync::{broadcast, mpsc};
use tokio::time::{interval, MissedTickBehavior};

use super::auth::{require_peek, Auth};
use super::roster::Roster;

/// events a stream may hold for a client that is slow to read; one more and it is dropped.
const QUEUE: usize = 8;
/// apply events the broadcast keeps for the streams; one that is further behind is dropped.
const BACKLOG: usize = 256;
/// the most streams one user may have open, and the most in all.
// a few browser tabs, each following the monitor and the apply
const PER_USER: usize = 8;
const IN_ALL: usize = 64;
/// how often an open stream checks that its session is still there.
const RECHECK: Duration = if cfg!(test) {
    Duration::from_millis(50)
} else {
    Duration::from_secs(5)
};
/// the seconds between samples, with the process list and without.
const FAST: Duration = Duration::from_secs(1);
const SLOW: Duration = Duration::from_secs(2);

fn refuse(status: StatusCode, text: &str) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        serde_json::json!({ "error": text }).to_string(),
    )
        .into_response()
}

/// an `EventSource` is a plain get, which carries no body and, from the same origin, may carry no
/// `Origin` header; what it always carries is the fetch metadata. so: same-origin by that, and
/// an `Origin`, when there is one, that is this site.
#[allow(clippy::result_large_err)]
fn same_origin_get(headers: &HeaderMap) -> Result<(), Response> {
    let text = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let forbidden = || refuse(StatusCode::FORBIDDEN, "forbidden");
    let host = text("host").ok_or_else(forbidden)?;
    if text("sec-fetch-site") != Some("same-origin") {
        return Err(forbidden());
    }
    match text("origin") {
        None => Ok(()),
        Some(origin) if origin == format!("https://{host}") => Ok(()),
        Some(_) => Err(forbidden()),
    }
}

/// the route of the streams, reading accounts' permissions from `roster`.
pub fn route<S>(roster: Arc<Roster>) -> MethodRouter<S>
where
    S: Clone + Send + Sync + 'static,
    Arc<Auth>: axum::extract::FromRef<S>,
{
    get(stream).layer(Extension(roster))
}

/// the topic named in the query, if it is one.
fn topic_in(uri: &Uri) -> Option<&'static str> {
    let given = uri
        .query()?
        .split('&')
        .find_map(|p| p.strip_prefix("topic="))?;
    [topic::MONITOR, topic::PROCS, topic::APPLY]
        .into_iter()
        .find(|t| *t == given)
}

/// one event for a client: its name and the json it carries.
struct Frame {
    name: &'static str,
    data: String,
}

/// the events a client reads, in the shape `Sse` wants.
struct Frames(mpsc::Receiver<Frame>);

impl Stream for Frames {
    type Item = Result<Event, std::convert::Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0
            .poll_recv(cx)
            .map(|f| f.map(|f| Ok(Event::default().event(f.name).data(f.data))))
    }
}

/// the streams open now, by user.
static OPEN: Mutex<Option<HashMap<String, usize>>> = Mutex::new(None);
static TOTAL: AtomicUsize = AtomicUsize::new(0);

/// a place among the open streams, given back when dropped.
struct Slot(String);

impl Slot {
    fn take(user: &str) -> Option<Slot> {
        let mut open = OPEN.lock().unwrap_or_else(PoisonError::into_inner);
        let open = open.get_or_insert_with(HashMap::new);
        if TOTAL.load(Ordering::SeqCst) >= IN_ALL
            || open.get(user).copied().unwrap_or(0) >= PER_USER
        {
            return None;
        }
        *open.entry(user.to_string()).or_default() += 1;
        TOTAL.fetch_add(1, Ordering::SeqCst);
        Some(Slot(user.to_string()))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut open = OPEN.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(open) = open.as_mut() {
            if let Some(n) = open.get_mut(&self.0) {
                *n -= 1;
                if *n == 0 {
                    open.remove(&self.0);
                }
            }
        }
        TOTAL.fetch_sub(1, Ordering::SeqCst);
    }
}

/// where apply events enter the web server. the web server runs unprivileged and applies run
/// in the root operations service, so the code that talks to that service passes each event it
/// receives to [`publish_apply`]; every stream that follows `apply` gets it.
fn apply_hub() -> &'static broadcast::Sender<Arc<str>> {
    static HUB: OnceLock<broadcast::Sender<Arc<str>>> = OnceLock::new();
    HUB.get_or_init(|| broadcast::channel(BACKLOG).0)
}

/// hand an apply event to the streams that follow applies. none open is not an error.
pub fn publish_apply(event: &ApplyEvent) {
    if let Ok(text) = serde_json::to_string(event) {
        let _ = apply_hub().send(text.into());
    }
}

/// GET /api/events?topic=monitor | monitor:procs | apply
async fn stream(
    State(auth): State<Arc<Auth>>,
    Extension(roster): Extension<Arc<Roster>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Err(refused) = same_origin_get(&headers) {
        return refused;
    }
    // a stream is not the person doing something: it must not keep the session alive
    let (user, _) = match require_peek(&auth, &headers) {
        Ok(who) => who,
        Err(unauthorized) => return unauthorized,
    };
    let Some(topic) = topic_in(&uri) else {
        return refuse(StatusCode::BAD_REQUEST, "no such stream");
    };
    let need = Op::Subscribe {
        topic: topic.into(),
    }
    .requires();
    match roster.grants_of(&user).await {
        Some(grants) if grants.allows(need) => {}
        Some(_) => return refuse(StatusCode::FORBIDDEN, "your account may not follow this"),
        None => {
            return refuse(
                StatusCode::SERVICE_UNAVAILABLE,
                "the account could not be checked",
            )
        }
    }
    // a page asking only whether it would be served (after a dropped connection) gets the
    // answer without taking a place among the open streams
    if uri
        .query()
        .is_some_and(|q| q.split('&').any(|p| p == "check=1"))
    {
        return (
            StatusCode::NO_CONTENT,
            [(header::CACHE_CONTROL, "no-store")],
        )
            .into_response();
    }
    let Some(slot) = Slot::take(&user) else {
        return refuse(StatusCode::TOO_MANY_REQUESTS, "too many open streams");
    };
    let (sender, receiver) = mpsc::channel(QUEUE);
    // subscribed before the answer goes out, so no event after it is missed
    let events = (topic == topic::APPLY).then(|| apply_hub().subscribe());
    tokio::spawn(feed(auth, roster, headers, topic, events, sender, slot));
    let mut response = Sse::new(Frames(receiver))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

/// push the events of `topic` until the client is gone or too slow, the session is over, or
/// the source ends. dropping `sender` ends the response.
async fn feed(
    auth: Arc<Auth>,
    roster: Arc<Roster>,
    headers: HeaderMap,
    topic: &'static str,
    mut events: Option<broadcast::Receiver<Arc<str>>>,
    sender: mpsc::Sender<Frame>,
    _slot: Slot,
) {
    let mut recheck = interval(RECHECK);
    recheck.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let need = Op::Subscribe {
        topic: topic.into(),
    }
    .requires();
    let procs = topic == topic::PROCS;
    let mut sampling = interval(if procs { FAST } else { SLOW });
    sampling.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        let frame = tokio::select! {
            _ = sender.closed() => return,
            _ = recheck.tick() => {
                let Some((user, _)) = auth.peek(&headers) else {
                    return;
                };
                // roles and groups change while a stream is open
                match roster.grants_of(&user).await {
                    Some(grants) if grants.allows(need) => {}
                    _ => return,
                }
                continue;
            }
            _ = sampling.tick(), if events.is_none() => {
                let read = tokio::task::spawn_blocking(move || Sampler::default().sample(procs));
                match read.await {
                    Ok(Ok(sample)) => match serde_json::to_string(&sample) {
                        Ok(data) => Frame { name: "sample", data },
                        Err(_) => return,
                    },
                    _ => return,
                }
            }
            got = async { events.as_mut().unwrap().recv().await }, if events.is_some() => {
                match got {
                    Ok(text) => Frame { name: "apply", data: text.to_string() },
                    // fell behind or the source is gone: the client reconnects
                    Err(_) => return,
                }
            }
        };
        // a full queue is a client that does not read: drop it
        if sender.try_send(frame).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::webui::auth::{Check, Limits, Lookup, Standing, Verifier};
    use crate::webui::roster::Roster;
    use axum::body::Body;
    use axum::http::Request;
    use lanlord_model::access::{Access, Area, Level, Role};
    use lanlord_model::settings::Settings;
    use std::future::Future;
    use std::net::IpAddr;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicU64;
    use std::time::Instant;
    use tower::ServiceExt;

    const HOST: &str = "gw.example.test";

    /// accepts the password `right` for these names; who is an administrator now is the set a
    /// test changes, as a change of group would.
    struct Fake {
        admins: Arc<Mutex<Vec<String>>>,
        /// when set, the account lookup fails
        failing: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Verifier for Fake {
        fn check(
            &self,
            user: &str,
            password: &[u8],
            _ip: Option<IpAddr>,
        ) -> Pin<Box<dyn Future<Output = Check> + Send>> {
            let answer = match (user, password) {
                ("admin" | "carol" | "olive" | "pat" | "viewer", b"right") => {
                    Check::Ok { admin: false }
                }
                _ => Check::Denied,
            };
            Box::pin(async move { answer })
        }

        fn admin(&self, user: &str) -> Lookup {
            let admin = self.admins.lock().unwrap().iter().any(|a| a == user);
            let failing = self.failing.load(Ordering::SeqCst);
            Box::pin(async move {
                if failing {
                    Standing::Failed
                } else {
                    Standing::Account { admin }
                }
            })
        }
    }

    struct Rig {
        app: axum::Router,
        auth: Arc<Auth>,
        folder: PathBuf,
        offset: Arc<AtomicU64>,
        admins: Arc<Mutex<Vec<String>>>,
        failing: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Rig {
        fn settings(&self, access: &Access) {
            let mut settings = Settings::empty();
            access.store(&mut settings);
            std::fs::write(self.folder.join("settings.yml"), settings.text()).unwrap();
        }
    }

    fn assigned(user: &str, role: &str) -> Access {
        let mut access = Access::default();
        access.members.insert(user.into(), vec![role.into()]);
        access
    }

    /// a server with its own folder of settings: `admin` is in the admins group.
    fn app() -> Rig {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let folder = std::env::temp_dir().join(format!(
            "lanlord-stream-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let admins = Arc::new(Mutex::new(vec!["admin".to_string()]));
        let failing = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let offset = Arc::new(AtomicU64::new(0));
        let (base, seen) = (Instant::now(), offset.clone());
        let auth = Auth::with_clock(
            Box::new(Fake {
                admins: admins.clone(),
                failing: failing.clone(),
            }),
            Limits {
                fail_delay: Duration::ZERO,
                ..Limits::default()
            },
            Box::new(move || base + Duration::from_secs(seen.load(Ordering::SeqCst))),
        );
        let roster = Roster::at(folder.join("settings.yml"), auth.clone());
        let app = axum::Router::new()
            .route("/api/login", axum::routing::post(super::super::auth::login))
            .route("/api/events", route(roster))
            .with_state(auth.clone());
        Rig {
            app,
            auth,
            folder,
            offset,
            admins,
            failing,
        }
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    async fn cookie_for(app: &axum::Router, user: &str) -> String {
        let body = serde_json::json!({ "user": user, "password": "right" }).to_string();
        let request = Request::builder()
            .method("POST")
            .uri("/api/login")
            .header("host", HOST)
            .header("origin", format!("https://{HOST}"))
            .header("sec-fetch-site", "same-origin")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let set = response.headers()[header::SET_COOKIE].to_str().unwrap();
        set.split(';').next().unwrap().to_string()
    }

    fn subscribe(topic: &str, cookie: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .uri(format!("/api/events?topic={topic}"))
            .header("host", HOST)
            .header("sec-fetch-site", "same-origin");
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        builder.body(Body::empty()).unwrap()
    }

    async fn status_of(rig: &Rig, topic: &str, cookie: Option<&str>) -> StatusCode {
        let response = rig
            .app
            .clone()
            .oneshot(subscribe(topic, cookie))
            .await
            .unwrap();
        response.status()
    }

    /// the next chunk of the body as text, or none when the stream has ended.
    async fn next(body: &mut Body) -> Option<String> {
        use axum::body::HttpBody;
        loop {
            let frame = std::future::poll_fn(|cx| Pin::new(&mut *body).poll_frame(cx)).await?;
            if let Ok(data) = frame.unwrap().into_data() {
                return Some(String::from_utf8_lossy(&data).into_owned());
            }
        }
    }

    /// wait for the stream to end by itself.
    async fn ends(body: &mut Body) -> bool {
        tokio::time::timeout(Duration::from_secs(5), async {
            while next(body).await.is_some() {}
        })
        .await
        .is_ok()
    }

    #[test]
    fn a_stream_is_refused_without_a_login() {
        runtime().block_on(async {
            let rig = app();
            assert_eq!(
                status_of(&rig, "monitor", None).await,
                StatusCode::UNAUTHORIZED
            );
        });
    }

    #[test]
    fn a_stream_is_refused_from_another_site_or_for_another_topic() {
        runtime().block_on(async {
            let rig = app();
            let cookie = cookie_for(&rig.app, "admin").await;
            let mut other = subscribe("monitor", Some(&cookie));
            other
                .headers_mut()
                .insert("sec-fetch-site", "cross-site".parse().unwrap());
            let response = rig.app.clone().oneshot(other).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            let mut foreign = subscribe("monitor", Some(&cookie));
            foreign
                .headers_mut()
                .insert("origin", "https://evil.example.test".parse().unwrap());
            let response = rig.app.clone().oneshot(foreign).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(
                status_of(&rig, "secrets", Some(&cookie)).await,
                StatusCode::BAD_REQUEST
            );
        });
    }

    #[test]
    fn a_stream_needs_the_permission_of_its_topic() {
        runtime().block_on(async {
            let rig = app();
            let cookie = cookie_for(&rig.app, "carol").await;
            // the Users role may watch the monitor, not the output of an apply
            assert_eq!(
                status_of(&rig, "monitor", Some(&cookie)).await,
                StatusCode::OK
            );
            assert_eq!(
                status_of(&rig, "apply", Some(&cookie)).await,
                StatusCode::FORBIDDEN
            );
        });
    }

    #[test]
    fn a_custom_role_with_services_view_gets_apply_and_one_without_status_no_monitor() {
        runtime().block_on(async {
            let rig = app();
            let role = |name: &str, areas: &[(Area, Level)]| Role {
                name: name.into(),
                grants: areas.iter().copied().collect(),
            };
            let mut access = Access {
                roles: vec![
                    role(
                        "Watcher",
                        &[(Area::Status, Level::View), (Area::Services, Level::View)],
                    ),
                    role("Blind", &[(Area::Clock, Level::View)]),
                ],
                ..Access::default()
            };
            access
                .members
                .insert("viewer".into(), vec!["Watcher".into()]);
            access.members.insert("olive".into(), vec!["Blind".into()]);
            rig.settings(&access);
            let viewer = cookie_for(&rig.app, "viewer").await;
            let olive = cookie_for(&rig.app, "olive").await;
            assert_eq!(
                status_of(&rig, "apply", Some(&viewer)).await,
                StatusCode::OK
            );
            assert_eq!(
                status_of(&rig, "monitor", Some(&olive)).await,
                StatusCode::FORBIDDEN
            );
        });
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_login_gets_monitor_samples() {
        runtime().block_on(async {
            let rig = app();
            let cookie = cookie_for(&rig.app, "admin").await;
            let response = rig
                .app
                .clone()
                .oneshot(subscribe("monitor:procs", Some(&cookie)))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                "text/event-stream"
            );
            let mut body = response.into_body();
            let mut seen = String::new();
            while !seen.contains("\n\n") {
                seen.push_str(&next(&mut body).await.expect("the stream ended"));
            }
            assert!(seen.contains("event: sample"), "{seen}");
            let data = seen.lines().find_map(|l| l.strip_prefix("data: ")).unwrap();
            let sample: lanlord_model::monitor::Sample = serde_json::from_str(data).unwrap();
            assert!(!sample.procs.is_empty());
        });
    }

    #[test]
    fn a_login_gets_apply_events() {
        runtime().block_on(async {
            let rig = app();
            let cookie = cookie_for(&rig.app, "admin").await;
            let response = rig
                .app
                .clone()
                .oneshot(subscribe("apply", Some(&cookie)))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let mut body = response.into_body();
            let event = ApplyEvent::Total { n: 3 };
            publish_apply(&event);
            let event = serde_json::to_string(&event).unwrap();
            let mut seen = String::new();
            while !seen.contains("\n\n") {
                seen.push_str(&next(&mut body).await.expect("the stream ended"));
            }
            assert!(seen.contains("event: apply"), "{seen}");
            assert!(seen.contains(&format!("data: {event}")), "{seen}");
        });
    }

    #[test]
    fn a_revoked_session_is_disconnected() {
        runtime().block_on(async {
            let rig = app();
            let cookie = cookie_for(&rig.app, "carol").await;
            let response = rig
                .app
                .clone()
                .oneshot(subscribe("monitor", Some(&cookie)))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let mut body = response.into_body();
            assert_eq!(rig.auth.revoke_user("carol"), 1);
            assert!(ends(&mut body).await, "the stream outlived its session");
        });
    }

    #[test]
    fn a_demoted_user_loses_the_apply_stream() {
        runtime().block_on(async {
            let rig = app();
            rig.settings(&assigned("olive", "Operators"));
            let cookie = cookie_for(&rig.app, "olive").await;
            let response = rig
                .app
                .clone()
                .oneshot(subscribe("apply", Some(&cookie)))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let mut body = response.into_body();
            // the role changes while the stream is open
            rig.settings(&assigned("olive", "Users"));
            assert!(ends(&mut body).await, "the stream outlived the permission");
            assert_eq!(
                status_of(&rig, "apply", Some(&cookie)).await,
                StatusCode::FORBIDDEN
            );
        });
    }

    #[test]
    fn leaving_the_admins_group_takes_the_rights_of_the_group_away() {
        runtime().block_on(async {
            let rig = app();
            let cookie = cookie_for(&rig.app, "admin").await;
            assert_eq!(
                status_of(&rig, "apply", Some(&cookie)).await,
                StatusCode::OK
            );
            rig.admins.lock().unwrap().clear();
            assert_eq!(
                status_of(&rig, "apply", Some(&cookie)).await,
                StatusCode::FORBIDDEN
            );
        });
    }

    #[test]
    fn a_failed_account_lookup_is_retryable_not_a_refusal() {
        runtime().block_on(async {
            let rig = app();
            let cookie = cookie_for(&rig.app, "admin").await;
            rig.failing.store(true, Ordering::SeqCst);
            assert_eq!(
                status_of(&rig, "apply", Some(&cookie)).await,
                StatusCode::SERVICE_UNAVAILABLE
            );
            rig.failing.store(false, Ordering::SeqCst);
            assert_eq!(
                status_of(&rig, "apply", Some(&cookie)).await,
                StatusCode::OK
            );
        });
    }

    #[test]
    fn a_stream_does_not_keep_an_idle_session_alive() {
        runtime().block_on(async {
            let rig = app();
            let cookie = cookie_for(&rig.app, "carol").await;
            rig.offset.fetch_add(10 * 60, Ordering::SeqCst);
            assert_eq!(
                status_of(&rig, "monitor", Some(&cookie)).await,
                StatusCode::OK
            );
            // twenty minutes after the last real use, opening a stream finds the session gone
            rig.offset.fetch_add(10 * 60, Ordering::SeqCst);
            assert_eq!(
                status_of(&rig, "monitor", Some(&cookie)).await,
                StatusCode::UNAUTHORIZED
            );
        });
    }

    #[test]
    fn a_probe_takes_no_place_and_a_full_user_is_told_so() {
        runtime().block_on(async {
            let rig = app();
            let role = Role {
                name: "Watcher".into(),
                grants: [(Area::Status, Level::View)].into_iter().collect(),
            };
            let mut access = Access {
                roles: vec![role],
                ..Access::default()
            };
            access.members.insert("pat".into(), vec!["Watcher".into()]);
            rig.settings(&access);
            let cookie = cookie_for(&rig.app, "pat").await;
            let held: Vec<_> = (0..PER_USER).map(|_| Slot::take("pat").unwrap()).collect();
            // every place is taken: a stream is turned away, a probe is still answered
            assert_eq!(
                status_of(&rig, "monitor", Some(&cookie)).await,
                StatusCode::TOO_MANY_REQUESTS
            );
            let probe = |topic: &str, cookie: Option<&str>| {
                let mut request = subscribe(topic, cookie);
                *request.uri_mut() = format!("/api/events?topic={topic}&check=1")
                    .parse()
                    .unwrap();
                request
            };
            let answer = rig
                .app
                .clone()
                .oneshot(probe("monitor", Some(&cookie)))
                .await
                .unwrap();
            assert_eq!(answer.status(), StatusCode::NO_CONTENT);
            // a probe is checked like a stream: no login, or no permission, is no 204
            let answer = rig
                .app
                .clone()
                .oneshot(probe("monitor", None))
                .await
                .unwrap();
            assert_eq!(answer.status(), StatusCode::UNAUTHORIZED);
            let answer = rig
                .app
                .clone()
                .oneshot(probe("apply", Some(&cookie)))
                .await
                .unwrap();
            assert_eq!(answer.status(), StatusCode::FORBIDDEN);
            // and it left nothing behind: freeing one place lets one stream in
            drop(held);
            assert_eq!(
                status_of(&rig, "monitor", Some(&cookie)).await,
                StatusCode::OK
            );
        });
    }

    #[test]
    fn a_user_has_a_few_streams_at_most() {
        let first = Slot::take("slot-user").unwrap();
        let rest: Vec<_> = (1..PER_USER).map(|_| Slot::take("slot-user")).collect();
        assert!(rest.iter().all(Option::is_some));
        assert!(Slot::take("slot-user").is_none());
        // someone else is not held back, and a freed place can be used again
        assert!(Slot::take("slot-other").is_some());
        drop(first);
        assert!(Slot::take("slot-user").is_some());
    }
}
