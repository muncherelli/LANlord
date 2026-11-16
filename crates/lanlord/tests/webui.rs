//! `lanlord webui` as systemd runs it: listen on a socket, answer, and stop cleanly on sigterm.
#![cfg(all(unix, feature = "webui"))]

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    let start = Instant::now();
    while !ready() {
        assert!(start.elapsed() < Duration::from_secs(20), "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn get(socket: &Path, uri: &str) -> String {
    let mut stream = UnixStream::connect(socket).unwrap();
    write!(
        stream,
        "GET {uri} HTTP/1.1\r\nHost: lanlord\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    reply
}

#[test]
fn the_web_server_answers_on_its_socket_and_stops_on_sigterm() {
    let root = std::env::temp_dir().join(format!("lanlord-webui-bin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let socket = root.join("run/lanlord/web.sock");
    let mut child = Command::new(env!("CARGO_BIN_EXE_lanlord"))
        .args(["webui", "--socket"])
        .arg(&socket)
        .arg("--static")
        .arg(root.join("web"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    wait_for("the socket never appeared", || socket.exists());
    assert!(get(&socket, "/healthz").ends_with("ok"));
    assert!(get(&socket, "/").contains("not built yet"));

    let killed = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let mut status = None;
    wait_for("the server did not stop", || {
        status = child.try_wait().unwrap();
        status.is_some()
    });
    assert!(status.unwrap().success(), "{status:?}");
    assert!(!socket.exists(), "the socket is removed on the way out");
    let _ = std::fs::remove_dir_all(root);
}
