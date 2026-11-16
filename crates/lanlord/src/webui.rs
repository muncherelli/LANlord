//! `lanlord webui`: the web server that runs on a gateway. it listens on a unix socket that caddy
//! proxies to (https on port 8443), serves the browser build of the gui from a folder and, later,
//! carries the typed operations the browser sends. it never runs commands from raw text.

use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use lanlord_core::log::{self, Kind};
use tower::ServiceExt;
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
/// endpoint. the build is looked for on every request, so installing or replacing it needs no
/// restart.
pub fn router(static_dir: Option<&Path>) -> Router {
    let dir = static_dir.map(Path::to_path_buf);
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/op", post(op))
        .fallback(move |request: Request| web(dir.clone(), request))
}

/// a file from the build. a path that names a file (`.wasm`, `.js`, `.css`...) is that file or a
/// 404; any other path is an address inside the app, so it gets the app's index.html (or the
/// placeholder page while there is no build).
async fn web(dir: Option<PathBuf>, request: Request) -> Response {
    let asset = is_asset(request.uri().path());
    let built = match dir {
        Some(dir) if is_build(&dir).await => dir,
        _ if asset => return StatusCode::NOT_FOUND.into_response(),
        _ => return placeholder().await.into_response(),
    };
    let files = ServeDir::new(&built);
    let served = if asset {
        files.oneshot(request).await
    } else {
        files
            .fallback(ServeFile::new(built.join("index.html")))
            .oneshot(request)
            .await
    };
    match served {
        Ok(response) => response.into_response(),
        Err(never) => match never {},
    }
}

/// whether the last part of `path` looks like a file name, with an extension.
fn is_asset(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty())
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
    let dir = match static_dir(options.static_dir) {
        Some(d) if is_build(&d).await => Some(d),
        _ => None,
    };
    match &dir {
        Some(d) => log::emit(
            Kind::Debug,
            format!("serving the web ui from {}", d.display()),
        ),
        None => log::emit(
            Kind::Info,
            "no web ui build found; serving a placeholder page",
        ),
    }
    let app = router(dir.as_deref());
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

/// whether `dir` holds a browser build.
async fn is_build(dir: &Path) -> bool {
    tokio::fs::metadata(dir.join("index.html"))
        .await
        .is_ok_and(|m| m.is_file())
}

#[cfg(unix)]
async fn serve_socket(
    path: &Path,
    app: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let listener = bind_socket(path).await?;
    log::emit(
        Kind::Info,
        format!("web ui listening on {}", path.display()),
    );
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await;
    let _ = tokio::fs::remove_file(path).await;
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

/// make the socket's folder, clear a socket left by an earlier run and listen, readable and
/// writable by the owner and group only (caddy reaches it through the group or as root).
#[cfg(unix)]
pub async fn bind_socket(path: &Path) -> Result<tokio::net::UnixListener> {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};

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
    let listener = tokio::net::UnixListener::bind(path)
        .with_context(|| format!("listening on {}", path.display()))?;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660)).await?;
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
            let first = bind_socket(&path).await.unwrap();
            drop(first);
            // the first run's socket file is still there; a second run takes its place
            assert!(tokio::fs::symlink_metadata(&path)
                .await
                .unwrap()
                .file_type()
                .is_socket());
            let _second = bind_socket(&path).await.unwrap();
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
            let listener = bind_socket(&path).await.unwrap();
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
                },
                async {},
            ))
            .unwrap_err();
        assert!(err.to_string().contains("making"), "{err}");
        let _ = std::fs::remove_dir_all(root);
    }
}
