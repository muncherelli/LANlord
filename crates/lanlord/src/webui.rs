//! `lanlord webui`: the web server that runs on a gateway. it listens on a unix socket that caddy
//! proxies to (https on port 8443), serves the browser build of the gui from a folder and, later,
//! carries the typed operations the browser sends. it never runs commands from raw text.

mod auth;
mod monitor;

use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use axum::extract::{DefaultBodyLimit, FromRef, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use base64::Engine as _;
use lanlord_core::log::{self, Kind};
use lanlord_core::ops::{Op, Request as OpRequest, Response as OpResponse, UserChange};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use tower_http::services::ServeFile;
use tower_http::timeout::TimeoutLayer;

use crate::operations_service;

/// how `lanlord webui` was asked to run.
pub struct Options {
    pub socket: PathBuf,
    /// a tcp address instead of the socket, for trying the server out on a test machine
    pub listen: Option<SocketAddr>,
    /// the browser build; `web` next to the binary's folder when not given
    pub static_dir: Option<PathBuf>,
    /// the operations service's socket
    pub operations_socket: PathBuf,
}

/// the largest operation the browser may send.
const OP_LIMIT: usize = 64 * 1024;

/// how long a request may take before it is answered with a timeout.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// how long an operation request may take: a little more than the service lets one run, so the
/// service's own answer comes first.
const OPERATIONS_TIMEOUT: Duration =
    Duration::from_secs(operations_service::RUN_TIMEOUT.as_secs() + 60);

/// the content security policy, with the hashes of the inline scripts a page may run (the build's
/// loader). caddy sets the same headers for anything that arrives without them.
fn csp(script_hashes: &[String]) -> String {
    let hashes: String = script_hashes
        .iter()
        .map(|h| format!(" 'sha256-{h}'"))
        .collect();
    format!(
        "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'{hashes}; \
         style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; \
         frame-ancestors 'none'"
    )
}

/// the folder the browser build is read from: the one given, or `../web` from the binary, so
/// `/opt/lanlord/bin/lanlord` serves `/opt/lanlord/web`.
pub fn static_dir(given: Option<PathBuf>) -> Option<PathBuf> {
    given.or_else(|| {
        let exe = std::env::current_exe().ok()?;
        Some(exe.parent()?.parent()?.join("web"))
    })
}

/// the routes: the browser build (or a placeholder page), a health check and the operations
/// endpoint. the build is looked for on every request, so installing or replacing it needs no
/// restart. every response carries the security headers.
#[cfg(test)]
fn router(static_dir: Option<&Path>) -> Router {
    router_at(
        static_dir,
        Path::new("/run/lanlord-operations/operations.sock"),
    )
}

/// the same, sending operations to the service on `operations_socket`.
pub fn router_at(static_dir: Option<&Path>, operations_socket: &Path) -> Router {
    router_with(
        static_dir,
        auth::Auth::system(),
        service_runner(operations_socket.to_path_buf()),
    )
}

/// the same, with the logins checked and the sessions kept by `auth` and the operations run by
/// `run`.
fn router_with(static_dir: Option<&Path>, auth: Arc<auth::Auth>, run: Runner) -> Router {
    let dir = static_dir.map(Path::to_path_buf);
    let quick = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/api/login",
            post(auth::login).layer(DefaultBodyLimit::max(auth::LOGIN_LIMIT)),
        )
        .route("/api/logout", post(auth::logout))
        .route("/api/session", get(auth::session))
        .route("/api/monitor", get(monitor::sample))
        .route("/api/monitor/end", post(monitor::end))
        .fallback(move |request: Request| web(dir.clone(), request))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ));
    // an operation may run for minutes (an apply, a purge): its route has its own, longer limit
    let long = Router::new()
        .route(
            "/api/operations",
            post(operations).layer(DefaultBodyLimit::max(OP_LIMIT)),
        )
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            OPERATIONS_TIMEOUT,
        ));
    quick
        .merge(long)
        .with_state(App { auth, run })
        .layer(axum::middleware::map_response(secure))
}

/// add the security headers a response does not set itself.
async fn secure(mut response: Response) -> Response {
    let headers = response.headers_mut();
    if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
        if let Ok(value) = HeaderValue::from_str(&csp(&[])) {
            headers.insert(header::CONTENT_SECURITY_POLICY, value);
        }
    }
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

/// a file from the build. a path that names a file (`.wasm`, `.js`, `.css`...) is that file or a
/// 404; any other path is an address inside the app, so it gets the app's page (or the
/// placeholder page while there is no build). dot files and anything outside the build, through
/// `..` or a link, are a 404.
async fn web(dir: Option<PathBuf>, request: Request) -> Response {
    let Some(path) = decoded(request.uri().path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if path.split('/').any(|segment| segment.starts_with('.')) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let asset = is_asset(&path) && path != "/index.html";
    let build = match dir {
        Some(dir) => built(&dir).await,
        None => None,
    };
    let Some((root, index)) = build else {
        if asset {
            return StatusCode::NOT_FOUND.into_response();
        }
        return placeholder().await.into_response();
    };
    if !asset {
        return page(&index).await;
    }
    let Some(file) = inside(&root, &path).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match ServeFile::new(file).oneshot(request).await {
        Ok(response) => response.into_response(),
        Err(never) => match never {},
    }
}

/// `path` with its %-escapes undone, or none when that is not text or hides a backslash.
fn decoded(path: &str) -> Option<String> {
    let text = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .ok()?
        .into_owned();
    (!text.contains('\\') && !text.contains('\0')).then_some(text)
}

/// whether the last part of `path` looks like a file name, with an extension.
fn is_asset(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty())
}

/// the build's real folder and its index.html, when `dir` holds a build whose index.html is
/// really inside it.
async fn built(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let root = tokio::fs::canonicalize(dir).await.ok()?;
    let index = tokio::fs::canonicalize(root.join("index.html"))
        .await
        .ok()?;
    let is_file = tokio::fs::metadata(&index).await.is_ok_and(|m| m.is_file());
    (is_file && index.starts_with(&root)).then_some((root, index))
}

/// the real file `path` names inside the build `root`, or none when it is missing or a link
/// leads out of the build.
async fn inside(root: &Path, path: &str) -> Option<PathBuf> {
    let file = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .fold(root.to_path_buf(), |file, segment| file.join(segment));
    let real = tokio::fs::canonicalize(file).await.ok()?;
    real.starts_with(root).then_some(real)
}

/// the app's page, with a policy that lets exactly its own inline scripts run.
async fn page(index: &Path) -> Response {
    match tokio::fs::read_to_string(index).await {
        Ok(html) => {
            let policy = csp(&inline_script_hashes(&html));
            let mut response = Html(html).into_response();
            if let Ok(value) = HeaderValue::from_str(&policy) {
                response
                    .headers_mut()
                    .insert(header::CONTENT_SECURITY_POLICY, value);
            }
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
            response
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// the sha-256 (base64) of every inline `<script>` in `html`, the way a browser hashes them for
/// the content security policy. scripts loaded with `src=` are left out.
fn inline_script_hashes(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut hashes = Vec::new();
    let mut from = 0;
    while let Some(start) = lower[from..].find("<script").map(|i| from + i) {
        let Some(open_end) = lower[start..].find('>').map(|i| start + i + 1) else {
            break;
        };
        let Some(close) = lower[open_end..].find("</script").map(|i| open_end + i) else {
            break;
        };
        if !lower[start..open_end].contains("src=") {
            let digest = Sha256::digest(&html.as_bytes()[open_end..close]);
            hashes.push(base64::engine::general_purpose::STANDARD.encode(digest));
        }
        from = close;
    }
    hashes
}

/// sends one operation for an account to whatever runs it; the apply events it reports on the
/// way go to the `Events` it is given.
type Runner =
    Arc<dyn Fn(String, Op, operations_service::Events) -> operations_service::Asking + Send + Sync>;

/// the real runner: the root operations service on its unix socket, which checks what the
/// account may do itself.
fn service_runner(socket: PathBuf) -> Runner {
    Arc::new(move |account, op, events| operations_service::ask(&socket, account, op, events))
}

/// where the apply events of an operation go. the progress stream (`publish_apply`) connects
/// here once it is merged; until then they are dropped.
fn apply_events() -> operations_service::Events {
    Arc::new(|_event| {})
}

/// what the handlers share: the logins and the way operations are run.
#[derive(Clone)]
struct App {
    auth: Arc<auth::Auth>,
    run: Runner,
}

impl FromRef<App> for Arc<auth::Auth> {
    fn from_ref(app: &App) -> Self {
        app.auth.clone()
    }
}

/// an error answer: `{"error": message}`.
fn refusal(status: StatusCode, message: &str) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        serde_json::json!({ "error": message }).to_string(),
    )
        .into_response()
}

/// the accounts whose sessions an op leaves out of date, and whether the caller's own session
/// may stay: a password change keeps the window it was made in, a permission change does not.
fn affected(op: &Op, caller: &str) -> Vec<(String, bool)> {
    let own = |name: &str| name == caller;
    match op {
        Op::ChangePassword { .. } => vec![(caller.to_string(), true)],
        Op::ResetPassword { user, .. } => vec![(user.clone(), own(user))],
        Op::ChangeUsers { changes } => changes
            .iter()
            .map(|change| match change {
                UserChange::SetPassword { name, .. } => (name.clone(), own(name)),
                UserChange::SetAdmin { name, .. }
                | UserChange::SetSoftwareAdmin { name, .. }
                | UserChange::Remove { name } => (name.clone(), false),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// the operations endpoint. it needs a page-origin json request and a login, and takes the same
/// `Request` body the browser sends. the account's admin state is read again on every request,
/// and the op goes to the operations service, which checks `Op::requires` for the account itself:
/// a refusal is a 403, a body that is not an operation a 400, a service that is not there a 503,
/// and anything else the `Response` json of `lanlord op`. after an op that changes a password or
/// a user's permissions, even one that failed half way, the affected users' sessions end (see
/// `Auth::revoke_user`). no sudo password travels: the service is root. live streams are not
/// served yet. the body is read already so the size limit applies.
async fn operations(
    State(app): State<App>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if let Err(refused) = auth::same_origin_json(&headers) {
        return refused;
    }
    let (user, _) = match auth::require_current(&app.auth, &headers).await {
        Ok(who) => who,
        Err(unauthorized) => return unauthorized,
    };
    let Ok(request) = serde_json::from_slice::<OpRequest>(&body) else {
        return refusal(StatusCode::BAD_REQUEST, "not an operation");
    };
    drop(body);
    if matches!(request.op, Op::Subscribe { .. }) {
        return refusal(
            StatusCode::NOT_IMPLEMENTED,
            "live streams are not available yet",
        );
    }
    let changed = affected(&request.op, &user);
    let answer = (app.run)(user, request.op, apply_events()).await;
    let revoke = |app: &App| {
        for (name, keep_own) in &changed {
            if *keep_own {
                app.auth.revoke_others(name, &headers);
            } else {
                app.auth.revoke_user(name);
            }
        }
    };
    let response = match answer {
        Ok(OpResponse::Denied(message)) => return refusal(StatusCode::FORBIDDEN, &message),
        Ok(response) => {
            // a failed batch may have changed some of it: end the sessions either way
            revoke(&app);
            response
        }
        Err(operations_service::Unreachable::Down) => {
            return refusal(
                StatusCode::SERVICE_UNAVAILABLE,
                "the operations service is not running",
            )
        }
        Err(operations_service::Unreachable::NoAnswer) => {
            // it may have run
            revoke(&app);
            return refusal(
                StatusCode::SERVICE_UNAVAILABLE,
                "the operations service did not answer",
            );
        }
    };
    match serde_json::to_string(&response) {
        Ok(text) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            text,
        )
            .into_response(),
        Err(_) => refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not encode the answer",
        ),
    }
}

/// shown until the browser build is installed.
async fn placeholder() -> Html<String> {
    Html(format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>LANlord Server v{v}</title></head>\
         <body><h1>LANlord Server v{v}</h1><p>The web UI is not built yet.</p></body></html>",
        v = env!("CARGO_PKG_VERSION")
    ))
}

/// run the server until it is stopped.
pub fn run(options: Options) -> Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(serve(options, stopped()))
}

/// serve until `shutdown` resolves, then remove the socket.
pub async fn serve(
    options: Options,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let dir = static_dir(options.static_dir);
    let found = match &dir {
        Some(d) => built(d).await.is_some(),
        None => false,
    };
    match &dir {
        Some(d) if found => log::emit(
            Kind::Debug,
            format!("serving the web ui from {}", d.display()),
        ),
        _ => log::emit(
            Kind::Info,
            "no web ui build found yet; serving a placeholder page until there is one",
        ),
    }
    let app = router_at(dir.as_deref(), &options.operations_socket);
    match options.listen {
        Some(addr) => {
            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .with_context(|| format!("listening on {addr}"))?;
            log::emit(Kind::Info, format!("web ui listening on http://{addr}"));
            Ok(axum::serve(listener, app)
                .with_graceful_shutdown(shutdown)
                .await?)
        }
        None => serve_socket(&options.socket, app, shutdown).await,
    }
}

#[cfg(unix)]
async fn serve_socket(
    path: &Path,
    app: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let (listener, ours) = bind_socket(path).await?;
    log::emit(
        Kind::Info,
        format!("web ui listening on {}", path.display()),
    );
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await;
    remove_if_ours(path, &ours).await;
    Ok(served?)
}

#[cfg(not(unix))]
async fn serve_socket(
    _path: &Path,
    _app: Router,
    _shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    bail!("unix sockets need a unix system; use --listen to try the server out")
}

/// the socket this run made: its device and inode, and a second handle on it. the handle keeps
/// the socket alive until the cleanup has looked, so its inode cannot be handed to a newer
/// run's socket in the meantime and mistaken for ours.
#[cfg(unix)]
pub struct Ours {
    id: (u64, u64),
    _held: std::os::unix::net::UnixListener,
}

/// make the socket's folder, clear a socket left by an earlier run and listen, readable and
/// writable by the owner and group only (caddy reaches it through the group or as root). also
/// says which socket is ours, so only that one is removed later.
#[cfg(unix)]
pub async fn bind_socket(path: &Path) -> Result<(tokio::net::UnixListener, Ours)> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("making {}", parent.display()))?;
    }
    match tokio::fs::symlink_metadata(path).await {
        Ok(meta) if meta.file_type().is_socket() => tokio::fs::remove_file(path)
            .await
            .with_context(|| format!("removing the old socket {}", path.display()))?,
        Ok(_) => bail!("{} exists and is not a socket", path.display()),
        Err(_) => {}
    }
    let listener = std::os::unix::net::UnixListener::bind(path)
        .with_context(|| format!("listening on {}", path.display()))?;
    let held = listener.try_clone()?;
    listener.set_nonblocking(true)?;
    let listener = tokio::net::UnixListener::from_std(listener)?;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660)).await?;
    let meta = tokio::fs::symlink_metadata(path).await?;
    let ours = Ours {
        id: (meta.dev(), meta.ino()),
        _held: held,
    };
    Ok((listener, ours))
}

/// remove the socket on the way out, unless another run has put its own socket there since.
#[cfg(unix)]
pub async fn remove_if_ours(path: &Path, ours: &Ours) {
    use std::os::unix::fs::MetadataExt;

    if let Ok(meta) = tokio::fs::symlink_metadata(path).await {
        if (meta.dev(), meta.ino()) == ours.id {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
}

/// resolves when the service is asked to stop: ctrl-c, or systemd's sigterm.
pub(crate) async fn stopped() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    /// a fresh folder under the system temp dir.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lanlord-webui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    async fn call(app: Router, method: &str, uri: &str) -> (StatusCode, String, Vec<u8>) {
        let response = app
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let kind = response
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        (status, kind, body)
    }

    #[test]
    fn the_health_check_answers_ok() {
        let (status, _, body) = runtime().block_on(call(router(None), "GET", "/healthz"));
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, b"ok");
    }

    #[test]
    fn operations_are_refused_without_the_pages_origin() {
        // (with a session they still get their answer from the operations service; see the session tests in auth.rs)
        let (status, kind, _) = runtime().block_on(call(router(None), "POST", "/api/operations"));
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(kind, "application/json");
    }

    #[test]
    fn without_a_build_every_page_is_the_placeholder() {
        let missing = scratch("missing").join("nothing-here");
        for path in ["/", "/some/page"] {
            let (status, kind, body) =
                runtime().block_on(call(router(Some(&missing)), "GET", path));
            assert_eq!(status, StatusCode::OK, "{path}");
            assert!(kind.starts_with("text/html"), "{kind}");
            let text = String::from_utf8(body).unwrap();
            assert!(text.contains(&format!("LANlord Server v{}", env!("CARGO_PKG_VERSION"))));
            assert!(text.contains("not built yet"));
        }
        // a file is never answered with a page
        for path in ["/lanlord_bg.wasm", "/lanlord.js", "/style.css"] {
            let (status, _, _) = runtime().block_on(call(router(Some(&missing)), "GET", path));
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        }
        let (status, _, _) = runtime().block_on(call(router(None), "GET", "/lanlord.js"));
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn a_build_installed_after_start_is_served_without_a_restart() {
        let dir = scratch("late");
        let app = router(Some(&dir));
        let rt = runtime();
        let (_, _, body) = rt.block_on(call(app.clone(), "GET", "/"));
        assert!(String::from_utf8(body).unwrap().contains("not built yet"));
        let (status, _, _) = rt.block_on(call(app.clone(), "GET", "/lanlord_bg.wasm"));
        assert_eq!(status, StatusCode::NOT_FOUND);

        std::fs::write(dir.join("index.html"), "<title>late app</title>").unwrap();
        std::fs::write(dir.join("lanlord_bg.wasm"), b"\0asm\x01\0\0\0").unwrap();
        let (_, _, body) = rt.block_on(call(app.clone(), "GET", "/"));
        assert!(String::from_utf8(body).unwrap().contains("late app"));
        let (status, kind, _) = rt.block_on(call(app, "GET", "/lanlord_bg.wasm"));
        assert_eq!(status, StatusCode::OK);
        assert_eq!(kind, "application/wasm");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn file_names_are_told_apart_from_app_addresses() {
        for asset in [
            "/lanlord_bg.wasm",
            "/pkg/lanlord.js",
            "/a.b/style.css",
            "/favicon.ico",
        ] {
            assert!(is_asset(asset), "{asset}");
        }
        for page in [
            "/",
            "/windows/dns",
            "/a.b/settings",
            "/.hidden",
            "/trailing.",
        ] {
            assert!(!is_asset(page), "{page}");
        }
    }

    #[test]
    fn the_build_is_served_with_the_right_types() {
        let dir = scratch("build");
        std::fs::write(dir.join("index.html"), "<!doctype html><title>app</title>").unwrap();
        std::fs::write(dir.join("lanlord_bg.wasm"), b"\0asm\x01\0\0\0").unwrap();
        std::fs::write(dir.join("lanlord.js"), "export {};").unwrap();
        let rt = runtime();

        let (status, kind, body) = rt.block_on(call(router(Some(&dir)), "GET", "/"));
        assert_eq!(status, StatusCode::OK);
        assert!(kind.starts_with("text/html"), "{kind}");
        assert!(String::from_utf8(body)
            .unwrap()
            .contains("<title>app</title>"));

        let (status, kind, body) = rt.block_on(call(router(Some(&dir)), "GET", "/lanlord_bg.wasm"));
        assert_eq!(status, StatusCode::OK);
        assert_eq!(kind, "application/wasm");
        assert_eq!(body, b"\0asm\x01\0\0\0");

        let (_, kind, _) = rt.block_on(call(router(Some(&dir)), "GET", "/lanlord.js"));
        assert!(kind.contains("javascript"), "{kind}");

        // a missing file is a 404, not the app
        let (status, _, _) = rt.block_on(call(router(Some(&dir)), "GET", "/missing.wasm"));
        assert_eq!(status, StatusCode::NOT_FOUND);

        // an address the app handles itself still gets the app
        let (status, kind, _) = rt.block_on(call(router(Some(&dir)), "GET", "/windows/dns"));
        assert_eq!(status, StatusCode::OK);
        assert!(kind.starts_with("text/html"), "{kind}");

        // the health check still wins over the files
        let (_, _, body) = rt.block_on(call(router(Some(&dir)), "GET", "/healthz"));
        assert_eq!(body, b"ok");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_build_folder_defaults_to_web_beside_the_bin_folder() {
        let given = PathBuf::from("/srv/elsewhere");
        assert_eq!(static_dir(Some(given.clone())), Some(given));
        let exe = std::env::current_exe().unwrap();
        let expected = exe.parent().unwrap().parent().unwrap().join("web");
        assert_eq!(static_dir(None), Some(expected));
    }

    #[cfg(unix)]
    #[test]
    fn the_socket_is_made_in_a_new_folder_and_replaces_an_old_one() {
        use std::os::unix::fs::{FileTypeExt, PermissionsExt};

        let root = scratch("socket");
        let path = root.join("run/lanlord/web.sock");
        let rt = runtime();
        rt.block_on(async {
            let first = bind_socket(&path).await.unwrap().0;
            drop(first);
            // the first run's socket file is still there; a second run takes its place
            assert!(tokio::fs::symlink_metadata(&path)
                .await
                .unwrap()
                .file_type()
                .is_socket());
            let _second = bind_socket(&path).await.unwrap().0;
            let mode = tokio::fs::metadata(&path)
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o660);
        });
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_in_the_sockets_place_is_left_alone() {
        let root = scratch("not-a-socket");
        let path = root.join("web.sock");
        std::fs::write(&path, "keep me").unwrap();
        let err = runtime().block_on(async { bind_socket(&path).await.map(|_| ()) });
        assert!(err.unwrap_err().to_string().contains("is not a socket"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep me");
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn the_server_answers_over_the_socket() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let root = scratch("serve");
        let path = root.join("web.sock");
        let rt = runtime();
        let reply = rt.block_on(async {
            let listener = bind_socket(&path).await.unwrap().0;
            let server = tokio::spawn(async move { axum::serve(listener, router(None)).await });
            let mut stream = tokio::net::UnixStream::connect(&path).await.unwrap();
            stream
                .write_all(b"GET /healthz HTTP/1.1\r\nHost: lanlord\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            let mut reply = String::new();
            stream.read_to_string(&mut reply).await.unwrap();
            server.abort();
            reply
        });
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        assert!(reply.ends_with("ok"), "{reply}");
        let _ = std::fs::remove_dir_all(root);
    }

    /// a tcp port that was free a moment ago.
    fn free_port() -> SocketAddr {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
    }

    #[cfg(unix)]
    async fn get_over_socket(path: &Path, uri: &str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut stream = tokio::net::UnixStream::connect(path).await.unwrap();
        let request = format!("GET {uri} HTTP/1.1\r\nHost: lanlord\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).await.unwrap();
        reply
    }

    /// wait until `path` exists, for a server that is starting.
    async fn appears(path: &Path) {
        for _ in 0..200 {
            if tokio::fs::symlink_metadata(path).await.is_ok() {
                return;
            }
            tokio::task::yield_now().await;
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("{} never appeared", path.display());
    }

    #[cfg(unix)]
    #[test]
    fn serve_runs_on_the_socket_with_the_build_and_cleans_up_when_stopped() {
        let root = scratch("serve-build");
        let build = root.join("web");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::write(build.join("index.html"), "<title>built app</title>").unwrap();
        let path = root.join("run/web.sock");
        let rt = runtime();
        let (reply, result) = rt.block_on(async {
            let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
            let server = tokio::spawn(serve(
                Options {
                    socket: path.clone(),
                    listen: None,
                    static_dir: Some(build.clone()),
                    operations_socket: path.with_extension("operations"),
                },
                async {
                    let _ = stopped.await;
                },
            ));
            appears(&path).await;
            let reply = get_over_socket(&path, "/").await;
            stop.send(()).unwrap();
            (reply, server.await.unwrap())
        });
        result.unwrap();
        assert!(reply.contains("built app"), "{reply}");
        assert!(
            !path.exists(),
            "the socket is removed when the server stops"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn serve_can_listen_on_tcp_to_try_it_out() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let addr = free_port();
        let rt = runtime();
        let (reply, result) = rt.block_on(async {
            let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
            let server = tokio::spawn(serve(
                Options {
                    socket: PathBuf::from("/nonexistent/unused.sock"),
                    listen: Some(addr),
                    static_dir: Some(PathBuf::from("/nonexistent/web")),
                    operations_socket: PathBuf::from("/nonexistent/operations.sock"),
                },
                async {
                    let _ = stopped.await;
                },
            ));
            let mut stream = loop {
                match tokio::net::TcpStream::connect(addr).await {
                    Ok(stream) => break stream,
                    Err(_) => tokio::time::sleep(std::time::Duration::from_millis(5)).await,
                }
            };
            stream
                .write_all(b"GET / HTTP/1.1\r\nHost: lanlord\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            let mut reply = String::new();
            stream.read_to_string(&mut reply).await.unwrap();
            stop.send(()).unwrap();
            (reply, server.await.unwrap())
        });
        result.unwrap();
        assert!(reply.contains("not built yet"), "{reply}");
    }

    #[test]
    fn a_taken_tcp_address_is_an_error() {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = taken.local_addr().unwrap();
        let err = runtime()
            .block_on(serve(
                Options {
                    socket: PathBuf::from("/nonexistent/unused.sock"),
                    listen: Some(addr),
                    static_dir: None,
                    operations_socket: PathBuf::from("/nonexistent/operations.sock"),
                },
                async {},
            ))
            .unwrap_err();
        assert!(err.to_string().contains("listening on"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_socket_that_cannot_be_made_is_an_error() {
        let root = scratch("blocked");
        // a file where the socket's folder should be
        std::fs::write(root.join("run"), "").unwrap();
        let err = runtime()
            .block_on(serve(
                Options {
                    socket: root.join("run/web.sock"),
                    listen: None,
                    static_dir: None,
                    operations_socket: PathBuf::from("/nonexistent/operations.sock"),
                },
                async {},
            ))
            .unwrap_err();
        assert!(err.to_string().contains("making"), "{err}");
        let _ = std::fs::remove_dir_all(root);
    }

    async fn headers(
        app: Router,
        method: &str,
        uri: &str,
        body: Body,
    ) -> (StatusCode, axum::http::HeaderMap) {
        let response = app
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(body)
                    .unwrap(),
            )
            .await
            .unwrap();
        (response.status(), response.headers().clone())
    }

    fn assert_secure(headers: &axum::http::HeaderMap, what: &str) {
        let csp = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(
            csp.starts_with("default-src 'self'; script-src 'self' 'wasm-unsafe-eval'"),
            "{what}: {csp}"
        );
        assert!(csp.contains("frame-ancestors 'none'"), "{what}: {csp}");
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff", "{what}");
        assert_eq!(headers[header::REFERRER_POLICY], "no-referrer", "{what}");
        assert!(!headers.contains_key(header::SERVER), "{what}");
    }

    #[test]
    fn every_response_carries_the_security_headers() {
        let dir = scratch("headers");
        std::fs::write(dir.join("index.html"), "<title>app</title>").unwrap();
        std::fs::write(dir.join("lanlord.js"), "export {};").unwrap();
        let rt = runtime();
        for (app, method, uri) in [
            (router(None), "GET", "/"),
            (router(None), "GET", "/healthz"),
            (router(None), "POST", "/api/operations"),
            (router(None), "GET", "/missing.js"),
            (router(Some(&dir)), "GET", "/"),
            (router(Some(&dir)), "GET", "/lanlord.js"),
        ] {
            let (_, h) = rt.block_on(headers(app, method, uri, Body::empty()));
            assert_secure(&h, uri);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_page_lets_exactly_its_own_inline_scripts_run() {
        let loader = "import init from '/lanlord-1a2b.js'; await init('/lanlord-1a2b_bg.wasm');";
        let dir = scratch("csp");
        std::fs::write(
            dir.join("index.html"),
            format!(
                "<html><head><script type=\"module\">{loader}</script>\
                 <SCRIPT src=\"/other.js\"></SCRIPT></head><body></body></html>"
            ),
        )
        .unwrap();
        let expected = base64::engine::general_purpose::STANDARD.encode(Sha256::digest(loader));
        let rt = runtime();
        for uri in ["/", "/index.html", "/windows/dns"] {
            let (status, h) = rt.block_on(headers(router(Some(&dir)), "GET", uri, Body::empty()));
            assert_eq!(status, StatusCode::OK, "{uri}");
            assert_secure(&h, uri);
            let csp = h[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
            assert!(
                csp.contains(&format!("'sha256-{expected}'")),
                "{uri}: {csp}"
            );
            assert_eq!(
                csp.matches("'sha256-").count(),
                1,
                "only the inline one: {csp}"
            );
        }
        assert_eq!(
            inline_script_hashes("<p>no scripts</p>"),
            Vec::<String>::new()
        );
        assert_eq!(
            inline_script_hashes("<script>unclosed"),
            Vec::<String>::new()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dot_files_are_never_served() {
        let dir = scratch("dots");
        std::fs::write(dir.join("index.html"), "<title>app</title>").unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git/config"), "secret").unwrap();
        std::fs::write(dir.join(".env"), "secret").unwrap();
        let rt = runtime();
        for app_dir in [Some(dir.as_path()), None] {
            for uri in [
                "/.git/config",
                "/%2egit/config",
                "/.env",
                "/pkg/.hidden.js",
                "/.well-known",
                "/a/%2E%2E/index.html",
            ] {
                let (status, _) = rt.block_on(headers(router(app_dir), "GET", uri, Body::empty()));
                assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
            }
        }
        // escapes that are not text, or hide a backslash
        for uri in ["/%ff.js", "/a%5c..%5cindex.html"] {
            let (status, _) = rt.block_on(headers(router(Some(&dir)), "GET", uri, Body::empty()));
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn links_out_of_the_build_are_refused() {
        use std::os::unix::fs::symlink;

        let root = scratch("links");
        let build = root.join("web");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::write(root.join("secret.txt"), "secret").unwrap();
        std::fs::write(build.join("index.html"), "<title>app</title>").unwrap();
        std::fs::write(build.join("lanlord.js"), "export {};").unwrap();
        symlink(root.join("secret.txt"), build.join("leak.txt")).unwrap();
        symlink(build.join("lanlord.js"), build.join("alias.js")).unwrap();
        let rt = runtime();

        let (status, _, _) = rt.block_on(call(router(Some(&build)), "GET", "/leak.txt"));
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _, body) = rt.block_on(call(router(Some(&build)), "GET", "/alias.js"));
        assert_eq!(status, StatusCode::OK, "a link inside the build is fine");
        assert_eq!(body, b"export {};");

        // an index.html that leads out of the build is no build at all
        let other = root.join("other");
        std::fs::create_dir_all(&other).unwrap();
        symlink(root.join("secret.txt"), other.join("index.html")).unwrap();
        let (_, _, body) = rt.block_on(call(router(Some(&other)), "GET", "/"));
        assert!(String::from_utf8(body).unwrap().contains("not built yet"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_operation_is_at_most_64_kib() {
        let rt = runtime();
        let (status, h) = rt.block_on(headers(
            router(None),
            "POST",
            "/api/operations",
            Body::from(vec![b'x'; OP_LIMIT + 1]),
        ));
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_secure(&h, "too large");
        let (status, _) = rt.block_on(headers(
            router(None),
            "POST",
            "/api/operations",
            Body::from(vec![b'x'; 100]),
        ));
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[cfg(unix)]
    #[test]
    fn only_our_own_socket_is_removed_on_the_way_out() {
        let root = scratch("ours");
        let path = root.join("web.sock");
        let rt = runtime();
        rt.block_on(async {
            // the first run's listener is gone, but it holds its socket until its cleanup
            let (first, first_ours) = bind_socket(&path).await.unwrap();
            drop(first);
            // a newer run took the path over
            let (_second, second_ours) = bind_socket(&path).await.unwrap();
            assert_ne!(first_ours.id, second_ours.id);
            remove_if_ours(&path, &first_ours).await;
            assert!(path.exists(), "the newer run's socket stays");
            remove_if_ours(&path, &second_ours).await;
            assert!(!path.exists());
            // nothing there is fine too
            remove_if_ours(&path, &second_ours).await;
        });
        let _ = std::fs::remove_dir_all(root);
    }
}
