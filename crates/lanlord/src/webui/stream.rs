//! `GET /api/events?topic=...`: live events for the browser as server-sent events. a page that
//! cannot hold a thread or a socket of its own opens one `EventSource` per topic and gets the
//! monitor samples (the Task Manager and the tray) or the progress of an apply pushed to it.
//!
//! the rules are those of `/api/op`: a login, a request that comes from the page itself and the
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
use std::path::PathBuf;
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
use lanlord_core::admins::ADMINS_GROUP;
use lanlord_core::monitor::Sampler;
use lanlord_model::access::{Access, Grants, Level};
use lanlord_model::apply::ApplyEvent;
use lanlord_model::ops::{topic, Op};
use lanlord_model::settings::Settings;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{interval, MissedTickBehavior};

use super::auth::{require_peek, Auth};

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

/// where the web server learns what an account may do: the gateway's role assignments in the
/// plain settings (which the unprivileged `lanlord` account owns) and the live membership of the
/// admins group, the same inputs the operations path reads. nothing here needs root, ssh or a
/// network.
pub struct Roster {
    settings: PathBuf,
    group: PathBuf,
    passwd: PathBuf,
}

impl Roster {
    pub fn system() -> Arc<Self> {
        Arc::new(Self {
            settings: "/opt/lanlord/config/settings.yml".into(),
            group: "/etc/group".into(),
            passwd: "/etc/passwd".into(),
        })
    }

    /// is `user` in the admins group now, as a member or by primary group?
    fn is_admin(&self, user: &str) -> bool {
        let fields = |line: &str| line.split(':').map(str::to_string).collect::<Vec<_>>();
        let group = std::fs::read_to_string(&self.group).unwrap_or_default();
        let Some(admins) = group
            .lines()
            .map(fields)
            .find(|f| f.first().map(String::as_str) == Some(ADMINS_GROUP) && f.len() >= 4)
        else {
            return false;
        };
        if admins[3].split(',').any(|member| member == user) {
            return true;
        }
        std::fs::read_to_string(&self.passwd)
            .unwrap_or_default()
            .lines()
            .map(fields)
            .any(|f| f.len() >= 4 && f[0] == user && f[3] == admins[2])
    }

    /// the stored assignments: none when the gateway never saved any, an error when they cannot
    /// be read.
    fn access(&self) -> anyhow::Result<Option<Access>> {
        let text = match std::fs::read_to_string(&self.settings) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        Access::from_settings(&Settings::parse(&text)?).map_err(|e| anyhow::anyhow!(e))
    }

    /// what `user` may do: administrators everything, the others what their roles give. what
    /// cannot be read gives nothing, so a damaged settings file closes streams rather than
    /// opening them.
    pub fn grants(&self, user: &str) -> Grants {
        if self.is_admin(user) {
            return Grants::all(Level::Manage);
        }
        match self.access() {
            Ok(access) => access.unwrap_or_default().grants(user, false),
            Err(_) => Grants::default(),
        }
    }

    /// as `grants`, off the async threads.
    async fn grants_of(self: &Arc<Self>, user: &str) -> Grants {
        let (roster, user) = (self.clone(), user.to_string());
        tokio::task::spawn_blocking(move || roster.grants(&user))
            .await
            .unwrap_or_default()
    }
}

/// the route of the streams, reading accounts' permissions from `roster`.
pub fn route(roster: Arc<Roster>) -> MethodRouter<Arc<Auth>> {
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
// nothing in the server calls it until the operations service is connected
#[cfg_attr(not(test), allow(dead_code))]
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
    if !roster.grants_of(&user).await.allows(need) {
        return refuse(StatusCode::FORBIDDEN, "your account may not follow this");
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
                if !roster.grants_of(&user).await.allows(need) {
                    return;
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
    use crate::webui::auth::{Check, Limits, Verifier};
    use axum::body::Body;
    use axum::http::Request;
    use lanlord_model::access::{Area, Role};
    use std::future::Future;
    use std::net::IpAddr;
    use std::sync::atomic::AtomicU64;
    use std::time::Instant;
    use tower::ServiceExt;

    const HOST: &str = "gw.example.test";

    /// accepts the password `right` for these names; whether they are administrators is the
    /// roster's business, not the login's.
    struct Fake;

    impl Verifier for Fake {
        fn check(
            &self,
            user: &str,
            password: &[u8],
            _ip: Option<IpAddr>,
        ) -> Pin<Box<dyn Future<Output = Check> + Send>> {
            let answer = match (user, password) {
                ("admin" | "carol" | "olive" | "viewer", b"right") => Check::Ok { admin: false },
                _ => Check::Denied,
            };
            Box::pin(async move { answer })
        }
    }

    struct Rig {
        app: axum::Router,
        auth: Arc<Auth>,
        folder: PathBuf,
        offset: Arc<AtomicU64>,
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

    /// a server with its own folder of settings and groups: `admin` is in the admins group.
    fn app() -> Rig {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let folder = std::env::temp_dir().join(format!(
            "lanlord-stream-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("group"), "root:x:0:\nadmins:x:900:admin\n").unwrap();
        std::fs::write(
            folder.join("passwd"),
            "carol:x:1001:1001::/home/carol:/bin/sh\n",
        )
        .unwrap();
        let roster = Arc::new(Roster {
            settings: folder.join("settings.yml"),
            group: folder.join("group"),
            passwd: folder.join("passwd"),
        });
        let offset = Arc::new(AtomicU64::new(0));
        let (base, seen) = (Instant::now(), offset.clone());
        let auth = Auth::with_clock(
            Box::new(Fake),
            Limits {
                fail_delay: Duration::ZERO,
                ..Limits::default()
            },
            Box::new(move || base + Duration::from_secs(seen.load(Ordering::SeqCst))),
        );
        let app = axum::Router::new()
            .route("/api/login", axum::routing::post(super::super::auth::login))
            .route("/api/events", route(roster))
            .with_state(auth.clone());
        Rig {
            app,
            auth,
            folder,
            offset,
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
            std::fs::write(rig.folder.join("group"), "admins:x:900:\n").unwrap();
            assert_eq!(
                status_of(&rig, "apply", Some(&cookie)).await,
                StatusCode::FORBIDDEN
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
