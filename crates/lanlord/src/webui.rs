//! `lanlord webui`: the web server that runs on a gateway. it listens on a unix socket that caddy
//! proxies to (https on port 8443), serves the browser build of the gui from a folder and, later,
//! carries the typed operations the browser sends. it never runs commands from raw text.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use lanlord_core::log::{self, Kind};
use tower_http::services::{ServeDir, ServeFile};

/// how `lanlord webui` was asked to run.
pub struct Options {
    pub socket: PathBuf,
    /// a tcp address instead of the socket, for trying the server out on a test machine
    pub listen: Option<SocketAddr>,
    /// the browser build; `web` next to the binary's folder when not given
    pub static_dir: Option<PathBuf>,
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
/// endpoint.
pub fn router(static_dir: Option<&Path>) -> Router {
    let routes = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/op", post(op));
    match static_dir.filter(|dir| dir.join("index.html").is_file()) {
        // unknown paths get the app too, so a reload on any address still opens it
        Some(dir) => routes
            .fallback_service(ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html")))),
        None => routes.fallback(placeholder),
    }
}

/// the operations endpoint. todo: hand the typed operation (json) to the operations executor
/// that `lanlord op` runs, once it exists; never a command line.
async fn op() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"error":"operations are not wired up yet"}"#,
    )
        .into_response()
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
    let dir = static_dir(options.static_dir);
    let app = router(dir.as_deref());
    match dir.as_deref().filter(|d| d.join("index.html").is_file()) {
        Some(d) => log::emit(
            Kind::Debug,
            format!("serving the web ui from {}", d.display()),
        ),
        None => log::emit(
            Kind::Info,
            "no web ui build found; serving a placeholder page",
        ),
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        match options.listen {
            Some(addr) => {
                let listener = tokio::net::TcpListener::bind(addr)
                    .await
                    .with_context(|| format!("listening on {addr}"))?;
                log::emit(Kind::Info, format!("web ui listening on http://{addr}"));
                axum::serve(listener, app)
                    .with_graceful_shutdown(stopped())
                    .await?;
            }
            None => serve_socket(&options.socket, app).await?,
        }
        Ok(())
    })
}

#[cfg(unix)]
async fn serve_socket(path: &Path, app: Router) -> Result<()> {
    let listener = bind_socket(path)?;
    log::emit(
        Kind::Info,
        format!("web ui listening on {}", path.display()),
    );
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(stopped())
        .await;
    let _ = std::fs::remove_file(path);
    Ok(served?)
}

#[cfg(not(unix))]
async fn serve_socket(_path: &Path, _app: Router) -> Result<()> {
    bail!("unix sockets need a unix system; use --listen to try the server out")
}

/// make the socket's folder, clear a socket left by an earlier run and listen, readable and
/// writable by the owner and group only (caddy reaches it through the group or as root).
#[cfg(unix)]
pub fn bind_socket(path: &Path) -> Result<tokio::net::UnixListener> {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("making {}", parent.display()))?;
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => std::fs::remove_file(path)
            .with_context(|| format!("removing the old socket {}", path.display()))?,
        Ok(_) => bail!("{} exists and is not a socket", path.display()),
        Err(_) => {}
    }
    let listener = tokio::net::UnixListener::bind(path)
        .with_context(|| format!("listening on {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))?;
    Ok(listener)
}

/// resolves when the service is asked to stop: ctrl-c, or systemd's sigterm.
async fn stopped() {
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
    fn operations_are_not_wired_up_yet() {
        let (status, kind, _) = runtime().block_on(call(router(None), "POST", "/api/op"));
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
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
            let first = bind_socket(&path).unwrap();
            drop(first);
            // the first run's socket file is still there; a second run takes its place
            assert!(std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_socket());
            let _second = bind_socket(&path).unwrap();
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
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
        let err = runtime().block_on(async { bind_socket(&path).map(|_| ()) });
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
            let listener = bind_socket(&path).unwrap();
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
}
