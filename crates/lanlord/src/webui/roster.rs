//! what the web server knows of an account's rights, and the one route that tells the page.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{FromRef, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, MethodRouter};
use axum::Extension;
use lanlord_model::access::{Access, Grants, Level};
use lanlord_model::settings::Settings;

use super::auth::{require_current, Auth, Standing};

/// where the web server learns what an account may do: whether it is an administrator now, asked
/// of the system by the same lookup the operations path uses (the groups of the account, as
/// `id -nG` gives them, so LDAP and winbind accounts count), and for everyone else the gateway's
/// role assignments in the plain settings, which the unprivileged `lanlord` account owns. nothing
/// here needs root, ssh or a network.
pub struct Roster {
    pub(super) settings: PathBuf,
    pub(super) auth: Arc<Auth>,
}

impl Roster {
    pub fn new(auth: Arc<Auth>) -> Arc<Self> {
        Self::at("/opt/lanlord/config/settings.yml".into(), auth)
    }

    /// as `new`, reading the assignments from `settings`.
    pub(super) fn at(settings: PathBuf, auth: Arc<Auth>) -> Arc<Self> {
        Arc::new(Self { settings, auth })
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

    /// what `user` may do, given whether they are an administrator now: administrators
    /// everything, the others what their roles give. what cannot be read gives nothing, so a
    /// damaged settings file closes streams rather than opening them.
    fn grants(&self, user: &str, admin: bool) -> Grants {
        if admin {
            return Grants::all(Level::Manage);
        }
        match self.access() {
            Ok(access) => access.unwrap_or_default().grants(user, false),
            Err(_) => Grants::default(),
        }
    }

    /// as `grants`, off the async threads.
    pub(super) async fn grants_given(self: &Arc<Self>, user: &str, admin: bool) -> Grants {
        let (roster, user) = (self.clone(), user.to_string());
        tokio::task::spawn_blocking(move || roster.grants(&user, admin))
            .await
            .unwrap_or_default()
    }

    /// what `user` may do now: nothing for an account that is gone, and `None` when the account
    /// could not be looked up (which is worth trying again, not a refusal).
    pub(super) async fn grants_of(self: &Arc<Self>, user: &str) -> Option<Grants> {
        match self.auth.standing(user).await {
            Standing::Account { admin } => Some(self.grants_given(user, admin).await),
            Standing::Unknown => Some(Grants::default()),
            Standing::Failed => None,
        }
    }
}

/// the route of `/api/session`, which also says what the account may do, read now.
pub fn route<S>(roster: Arc<Roster>) -> MethodRouter<S>
where
    S: Clone + Send + Sync + 'static,
    Arc<Auth>: FromRef<S>,
{
    get(session).layer(Extension(roster))
}

/// GET /api/session: who is logged in, whether they are an admin, and their grants. both are
/// read from the account now, not as they were at login.
async fn session(
    State(auth): State<Arc<Auth>>,
    Extension(roster): Extension<Arc<Roster>>,
    headers: HeaderMap,
) -> Response {
    match require_current(&auth, &headers).await {
        Ok((user, admin)) => {
            let grants = roster.grants_given(&user, admin).await;
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, "application/json"),
                    (header::CACHE_CONTROL, "no-store"),
                ],
                serde_json::json!({ "user": user, "admin": admin, "grants": grants }).to_string(),
            )
                .into_response()
        }
        Err(unauthorized) => unauthorized,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use axum::body::Body;
    use axum::http::Request;
    use lanlord_model::access::{Area, Role};
    use tower::ServiceExt;

    use super::*;
    use crate::webui::auth::{Check, Limits, Lookup, Standing, Verifier};

    struct Fake;

    impl Verifier for Fake {
        fn check(
            &self,
            user: &str,
            _password: &[u8],
            _ip: Option<std::net::IpAddr>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Check> + Send>> {
            let answer = if user == "pat" {
                Check::Ok { admin: false }
            } else {
                Check::Denied
            };
            Box::pin(async move { answer })
        }

        fn admin(&self, user: &str) -> Lookup {
            let known = user == "pat";
            Box::pin(async move {
                if known {
                    Standing::Account { admin: false }
                } else {
                    Standing::Unknown
                }
            })
        }
    }

    #[test]
    fn the_session_carries_the_grants_of_the_account_roles() {
        let folder = std::env::temp_dir().join(format!("lanlord-roster-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let mut access = Access {
            roles: vec![Role {
                name: "Watcher".into(),
                grants: [(Area::Status, Level::View), (Area::Services, Level::View)]
                    .into_iter()
                    .collect(),
            }],
            ..Access::default()
        };
        access.members.insert("pat".into(), vec!["Watcher".into()]);
        let mut settings = Settings::empty();
        access.store(&mut settings);
        std::fs::write(folder.join("settings.yml"), settings.text()).unwrap();
        let auth = Auth::with_clock(
            Box::new(Fake),
            Limits {
                fail_delay: Duration::ZERO,
                ..Limits::default()
            },
            Box::new(Instant::now),
        );
        let roster = Roster::at(folder.join("settings.yml"), auth.clone());
        let app = axum::Router::new()
            .route("/api/login", axum::routing::post(super::super::auth::login))
            .route("/api/session", route(roster))
            .with_state(auth);
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let host = "gw.example.test";
                let login = Request::builder()
                    .method("POST")
                    .uri("/api/login")
                    .header("host", host)
                    .header("origin", format!("https://{host}"))
                    .header("sec-fetch-site", "same-origin")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"user":"pat","password":"right"}"#))
                    .unwrap();
                let response = app.clone().oneshot(login).await.unwrap();
                let set = response.headers()[header::SET_COOKIE].to_str().unwrap();
                let cookie = set.split(';').next().unwrap().to_string();
                let request = Request::builder()
                    .uri("/api/session")
                    .header("host", host)
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap();
                let response = app.clone().oneshot(request).await.unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                let bytes = axum::body::to_bytes(response.into_body(), 1 << 16)
                    .await
                    .unwrap();
                let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(value["user"], "pat");
                assert_eq!(value["admin"], false);
                let grants: Grants = serde_json::from_value(value["grants"].clone()).unwrap();
                assert_eq!(grants.level(Area::Services), Level::View);
                assert_eq!(grants.level(Area::Network), Level::Hidden);
            });
        let _ = std::fs::remove_dir_all(&folder);
    }
}
