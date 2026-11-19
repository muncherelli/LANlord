//! how a client reaches the operations that touch secrets without holding a key. the gateway
//! keeps the key (root only) and decrypts only inside the root operations service, so a client
//! sends such an operation where the key is and gets back a reply that has no secret in it:
//!
//! - on the gateway itself, through the service's local unix socket. the kernel says who is
//!   asking (`SO_PEERCRED`), so there is no ssh and no sudo, and the request names no account;
//! - from another machine, through `sudo lanlord operate` over the ssh session that is already
//!   signed in. sudo tells `operate` which account asked.
//!
//! both ends speak what the service's socket speaks: one request line of json, then zero or more
//! `{"event": ...}` lines (apply progress) and a last `{"response": ...}` line.

use std::path::Path;

use anyhow::{anyhow, bail, Result};
use lanlord_model::apply::ApplyEvent;
use lanlord_model::operations::{Operation, Reply, Request, Response};

use crate::access::Refused;
use crate::bootstrap::{root_argv, AskPassword};
use crate::bridge::BRIDGE_BIN;
use crate::log::{emit, Kind};
use crate::ssh::Session;

/// where the operations service listens for the accounts of this machine. the folder can be
/// searched by anyone and the socket is open to connect to: the service decides by the kernel's
/// word on who connected, and serves only ordinary login accounts, each as itself.
pub const LOCAL_SOCKET: &str = "/run/lanlord-operations/local.sock";

pub use lanlord_model::signin::{is_this_server, THIS_SERVER};

/// the `user@host` target a local logon is kept under in the list of servers.
pub fn this_server_target(user: &str) -> String {
    format!("{user}@{THIS_SERVER}")
}

/// does the operations service of this machine listen on `socket`? a socket file is only there
/// while the service runs, so this is all the logon box needs to offer "This server".
pub fn offered_at(socket: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        std::fs::symlink_metadata(socket).is_ok_and(|meta| meta.file_type().is_socket())
    }
    #[cfg(not(unix))]
    {
        let _ = socket;
        false
    }
}

/// the one request line for `operation`: the operation alone. a local request never names an
/// account.
fn request_line(operation: &Operation) -> Result<zeroize::Zeroizing<Vec<u8>>> {
    let mut line = serde_json::to_vec(&serde_json::json!({ "op": operation }))?;
    line.push(b'\n');
    Ok(zeroize::Zeroizing::new(line))
}

/// what has been read of an answer.
#[derive(Default)]
struct Answered {
    response: Option<Response>,
}

impl Answered {
    /// take one line of an answer: an event goes to the log as the apply progress display reads
    /// it, the response is kept, and anything else (what sudo or the shell said) is ignored.
    fn take(&mut self, line: &str) {
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(line) else {
            return;
        };
        let Some(fields) = value.as_object_mut() else {
            return;
        };
        if let Some(event) = fields.remove("event") {
            if let Ok(event) = serde_json::from_value::<ApplyEvent>(event) {
                if let Ok(text) = serde_json::to_string(&event) {
                    emit(Kind::Event, text);
                }
            }
        } else if let Some(response) = fields.remove("response") {
            self.response = serde_json::from_value(response).ok();
        }
    }

    fn finish(self) -> Result<Reply> {
        match self.response {
            Some(Response::Ok(reply)) => Ok(reply),
            Some(Response::Denied(why)) => Err(Refused(why).into()),
            Some(Response::Error(why)) => Err(anyhow!(why)),
            None => bail!("the gateway gave no answer"),
        }
    }
}

/// run `operation` through the service on `socket`. the service takes the account from the kernel.
#[cfg(unix)]
pub fn through_socket(socket: &Path, operation: &Operation) -> Result<Reply> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let mut stream = UnixStream::connect(socket).map_err(|err| {
        anyhow!(
            "the operations service is not reachable at {}: {err}",
            socket.display()
        )
    })?;
    stream.set_read_timeout(Some(
        crate::operations::RUN_LIMIT + std::time::Duration::from_secs(30),
    ))?;
    stream.write_all(&request_line(operation)?)?;
    let mut answered = Answered::default();
    for line in BufReader::new(stream).lines() {
        answered.take(&line?);
    }
    answered.finish()
}

#[cfg(not(unix))]
pub fn through_socket(_socket: &Path, _operation: &Operation) -> Result<Reply> {
    bail!("the operations service is only on the gateway")
}

/// run `operation` with `sudo lanlord operate` over `session`. sudo's password, when the server
/// wants one, is the first line of what is sent and the request follows (see `bridge`).
pub fn through_sudo(session: &Session, operation: &Operation, ask: AskPassword) -> Result<Reply> {
    let admin = crate::config::admin(session)?;
    let (remote, first) = root_argv(session, &admin, &[BRIDGE_BIN, "operate"], ask)?;
    let mut input = zeroize::Zeroizing::new(first.into_bytes());
    input.extend_from_slice(&request_line(operation)?);
    let mut answered = Answered::default();
    session.run_with_stdin_lines(&remote, &input, &mut |line| answered.take(line))?;
    answered.finish()
}

/// the request `lanlord operate` reads: one line holding the operation (a password field, as a
/// bridge request may carry, is ignored: sudo asked for it already).
pub fn read_request(line: &str) -> Result<Operation> {
    Ok(serde_json::from_str::<Request>(line)?.operation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_servers_own_name_is_the_local_target() {
        assert!(is_this_server(&this_server_target("alice")));
        assert!(is_this_server(THIS_SERVER));
        assert!(!is_this_server("alice@gw.example.net"));
        assert!(!is_this_server("alice@localhost"));
        assert!(!is_this_server("this-server.localhost@gw"));
    }

    #[test]
    fn this_server_is_a_name_ssh_would_take_and_nobody_else_owns() {
        assert!(Session::new(&this_server_target("alice")).is_ok());
        assert!(THIS_SERVER.ends_with(".localhost"));
    }

    #[test]
    fn a_local_request_is_the_operation_alone() {
        let line = request_line(&Operation::LoadDocument).unwrap();
        let text = String::from_utf8(line.to_vec()).unwrap();
        assert_eq!(text, "{\"op\":{\"op\":\"load_document\"}}\n");
        assert!(!text.contains("account"));
        assert!(matches!(
            read_request(&text).unwrap(),
            Operation::LoadDocument
        ));
    }

    #[test]
    fn the_answer_is_the_last_response_and_noise_is_ignored() {
        let mut answered = Answered::default();
        answered.take("sudo: a password is required");
        answered.take(r#"{"event":{"event":"log","text":"hi"}}"#);
        answered.take(r#"{"response":{"ok":{"reply":"done"}}}"#);
        assert_eq!(answered.finish().unwrap(), Reply::Done);
    }

    #[test]
    fn a_refusal_is_a_refusal_and_no_answer_is_an_error() {
        let mut answered = Answered::default();
        answered.take(r#"{"response":{"denied":"permission denied"}}"#);
        let error = answered.finish().unwrap_err();
        assert!(error.downcast_ref::<Refused>().is_some(), "{error:#}");
        assert!(Answered::default().finish().is_err());
    }

    #[test]
    fn the_logon_box_offers_this_server_only_when_the_socket_exists() {
        let dir = std::env::temp_dir().join(format!("lanlord-offer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("local.sock");
        assert!(!offered_at(&socket));
        // a plain file of that name is not the service
        std::fs::write(&socket, "").unwrap();
        assert!(!offered_at(&socket));
        std::fs::remove_file(&socket).unwrap();
        #[cfg(unix)]
        {
            let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
            assert!(offered_at(&socket));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// a service on a socket of its own that reads one line and answers with `lines`.
    #[cfg(unix)]
    fn serve_once(
        name: &str,
        lines: &'static [&'static str],
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        use std::io::{BufRead, BufReader, Write};
        let dir = tempfile::Builder::new()
            .prefix(&format!("lanlord-operation-{name}-"))
            .tempdir()
            .unwrap();
        let socket = dir.path().join("local.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut asked = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut asked)
                .unwrap();
            assert!(asked.contains("load_document"), "{asked}");
            for line in lines {
                writeln!(stream, "{line}").unwrap();
            }
        });
        (dir, socket)
    }

    #[cfg(unix)]
    #[test]
    fn a_request_through_the_socket_gets_the_response_after_the_events() {
        let (_dir, socket) = serve_once(
            "ok",
            &[
                r#"{"event":{"event":"log","text":"hi"}}"#,
                "noise",
                r#"{"response":{"ok":{"reply":"done"}}}"#,
            ],
        );
        assert!(matches!(
            through_socket(&socket, &Operation::LoadDocument).unwrap(),
            Reply::Done
        ));
    }

    #[cfg(unix)]
    #[test]
    fn the_socket_says_denied_as_a_refusal_and_silence_as_an_error() {
        let (_dir, socket) = serve_once("denied", &[r#"{"response":{"denied":"no"}}"#]);
        let error = through_socket(&socket, &Operation::LoadDocument).unwrap_err();
        assert!(error.downcast_ref::<Refused>().is_some());
        let (_dir, socket) = serve_once("silent", &[]);
        let error = through_socket(&socket, &Operation::LoadDocument).unwrap_err();
        assert!(error.to_string().contains("no answer"), "{error}");
        let (_dir, socket) = serve_once("failed", &[r#"{"response":{"error":"broke"}}"#]);
        let error = through_socket(&socket, &Operation::LoadDocument).unwrap_err();
        assert_eq!(error.to_string(), "broke");
    }

    #[cfg(unix)]
    #[test]
    fn no_service_is_said_with_where_it_looked() {
        let socket = std::env::temp_dir().join("lanlord-operation-nothing-here.sock");
        let error = through_socket(&socket, &Operation::LoadDocument).unwrap_err();
        assert!(error.to_string().contains("not reachable"), "{error}");
    }
}
