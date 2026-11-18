//! what the web server knows of an account's rights, and the one route that tells the page.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, MethodRouter};
use axum::Extension;
use lanlord_core::admins::ADMINS_GROUP;
use lanlord_model::access::{Access, Grants, Level};
use lanlord_model::settings::Settings;

use super::auth::{require, Auth};

/// where the web server learns what an account may do: the gateway's role assignments in the
/// plain settings (which the unprivileged `lanlord` account owns) and the live membership of the
/// admins group, the same inputs the operations path reads. nothing here needs root, ssh or a
/// network.
///
/// the group is read from `/etc/group` and `/etc/passwd`, where the operations path asks the
/// system (`id -nG`, which also sees LDAP or winbind). on a gateway whose accounts live
/// elsewhere nobody is found in the admins group here, so it fails closed: such administrators
/// get what their roles give, and no more.
pub struct Roster {
    pub(super) settings: PathBuf,
    pub(super) group: PathBuf,
    pub(super) passwd: PathBuf,
}

impl Roster {
    pub fn system() -> Arc<Self> {
        Arc::new(Self {
            settings: "/opt/lanlord/config/settings.yml".into(),
            group: "/etc/group".into(),
            passwd: "/etc/passwd".into(),
        })
    }

    /// is `user` in the admins group now, as a member or by primary group? this reads
    /// `/etc/group` and `/etc/passwd` directly, where core asks `id -nG` (which sees NSS: LDAP,
    /// winbind), so under such accounts it finds nobody and fails closed.
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
    pub(super) async fn grants_of(self: &Arc<Self>, user: &str) -> Grants {
        let (roster, user) = (self.clone(), user.to_string());
        tokio::task::spawn_blocking(move || roster.grants(&user))
            .await
            .unwrap_or_default()
    }
}

/// the route of `/api/session`, which also says what the account may do, read now.
pub fn route(roster: Arc<Roster>) -> MethodRouter<Arc<Auth>> {
    get(session).layer(Extension(roster))
}

/// GET /api/session: who is logged in, whether they are an admin, and their grants.
async fn session(
    State(auth): State<Arc<Auth>>,
    Extension(roster): Extension<Arc<Roster>>,
    headers: HeaderMap,
) -> Response {
    match require(&auth, &headers) {
        Ok((user, admin)) => {
            let grants = roster.grants_of(&user).await;
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
    use crate::webui::auth::{Check, Limits, Verifier};

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
        let roster = Arc::new(Roster {
            settings: folder.join("settings.yml"),
            group: folder.join("group"),
            passwd: folder.join("passwd"),
        });
        let auth = Auth::with_clock(
            Box::new(Fake),
            Limits {
                fail_delay: Duration::ZERO,
                ..Limits::default()
            },
            Box::new(Instant::now),
        );
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
