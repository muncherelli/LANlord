//! `lanlord operations-service`: the root service that runs the operations the web ui forwards.
//! the web ui itself is sandboxed (no network, no privileges), so it sends each operation here
//! over a unix socket as one line of json, `{"account": ..., "op": ...}`, and gets one line back,
//! the same `Response` `lanlord op` prints.
//!
//! trust boundary: this service believes the web ui about which account is signed in; it does
//! not believe it about what that account may do. it reads the account's group membership again
//! and runs core's permission check itself before anything runs. what keeps others from claiming
//! to be the web ui is the socket's group and a uid check on every connection (`SO_PEERCRED`):
//! only the web ui's account and root are served.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use lanlord_core::access::{Caller, Denied, Refused};
use lanlord_core::log::{self, Kind};
use lanlord_core::ops::{execute_in, Op, Reply, Response};
use lanlord_core::ssh::Session;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Semaphore;

/// the largest request line.
pub const REQUEST_LIMIT: usize = 128 * 1024;

/// the largest answer the web ui reads.
pub const ANSWER_LIMIT: u64 = 16 * 1024 * 1024;

/// how long a client may take to send its request.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// how long an operation may run: the longest ones (an apply, a purge) take minutes.
pub const RUN_TIMEOUT: Duration = Duration::from_secs(16 * 60);

/// how many operations run at the same time.
const WORKERS: usize = 4;

/// the longest error text sent back to the web ui.
const MESSAGE_LIMIT: usize = 200;

/// what the web ui sends: the signed-in account and what it asks for. there is no sudo password
/// in it: the service runs as root and never prompts.
pub struct Forwarded {
    pub account: String,
    pub op: Op,
}

impl Forwarded {
    fn to_line(&self) -> Option<Vec<u8>> {
        let mut line =
            serde_json::to_vec(&serde_json::json!({ "account": self.account, "op": self.op }))
                .ok()?;
        line.push(b'\n');
        Some(line)
    }

    /// the request in `line`, when it is exactly an account name and an operation.
    fn from_line(line: &[u8]) -> Option<Self> {
        let mut value: serde_json::Value = serde_json::from_slice(line).ok()?;
        let fields = value.as_object_mut().filter(|o| o.len() == 2)?;
        let account = fields.remove("account")?.as_str()?.to_string();
        let op = serde_json::from_value(fields.remove("op")?).ok()?;
        Some(Self { account, op })
    }
}

/// how the service is run.
pub struct Options {
    pub socket: PathBuf,
    /// the uid of the web ui's account; root is always served too
    pub allowed_uid: u32,
}

/// whether a peer with this uid is served.
fn served(peer: u32, allowed_uid: u32) -> bool {
    peer == 0 || peer == allowed_uid
}

/// the short text of an error: its outermost message on one line, never the chain of causes
/// (paths, command output). the whole chain goes to the log.
fn short(error: &anyhow::Error) -> String {
    let text = error.to_string();
    let line = text.lines().next().unwrap_or_default().trim();
    line.chars().take(MESSAGE_LIMIT).collect()
}

/// answer one request line: look the account up again, run the op if core's check allows it,
/// and say what happened. a refusal is a `Denied` answer, anything else that goes wrong a short
/// `Error`.
fn answer_line(
    line: &[u8],
    lookup: &dyn Fn(&str) -> Result<Caller>,
    run: &dyn Fn(&Caller, &Op) -> Result<Reply>,
) -> Response {
    let Some(forwarded) = Forwarded::from_line(line) else {
        return Response::Error("not an operation".into());
    };
    let caller = match lookup(&forwarded.account) {
        Ok(caller) => caller,
        Err(e) => {
            log::emit(
                Kind::Err,
                format!("operation for {:?} refused: {e:#}", forwarded.account),
            );
            return Response::Denied("this account is not known here".into());
        }
    };
    match run(&caller, &forwarded.op) {
        Ok(reply) => Response::Ok(reply),
        Err(e) => {
            let refused =
                e.downcast_ref::<Denied>().is_some() || e.downcast_ref::<Refused>().is_some();
            log::emit(
                Kind::Err,
                format!("operation for {} failed: {e:#}", caller.user),
            );
            if refused {
                Response::Denied(short(&e))
            } else {
                Response::Error(short(&e))
            }
        }
    }
}

/// the real thing: the account is read from this machine's groups, and the op runs locally as
/// root through core's `execute_in`, which checks `Op::requires`.
fn system_answer(line: &[u8]) -> Response {
    let session = Session::local();
    answer_line(
        line,
        &|account| Caller::of_account(&session, account),
        &|caller, op| {
            let mut ask =
                |_: &str| -> Result<String> { bail!("no sudo prompt: the service is root") };
            execute_in(&session, "localhost", caller, op, &mut ask)
        },
    )
}

/// read one request line of at most `REQUEST_LIMIT` bytes, or say why not.
async fn read_line<R: tokio::io::AsyncRead + Unpin>(stream: R) -> Result<Vec<u8>, &'static str> {
    let mut reader = BufReader::new(stream.take(REQUEST_LIMIT as u64 + 1));
    let mut line = Vec::new();
    let read = tokio::time::timeout(READ_TIMEOUT, reader.read_until(b'\n', &mut line)).await;
    match read {
        Err(_) => Err("the request took too long"),
        Ok(Err(_)) => Err("the request could not be read"),
        Ok(Ok(_)) if line.len() > REQUEST_LIMIT => Err("the request is too large"),
        Ok(Ok(_)) if line.last() != Some(&b'\n') => Err("the request is not a whole line"),
        Ok(Ok(_)) => Ok(line),
    }
}

/// how an answer is produced from a request line; blocking, so it runs off the async threads.
type Answer = Arc<dyn Fn(&[u8]) -> Response + Send + Sync>;

async fn handle(mut stream: tokio::net::UnixStream, allowed_uid: u32, answer: Answer) {
    match stream.peer_cred() {
        Ok(cred) if served(cred.uid(), allowed_uid) => {}
        Ok(cred) => {
            log::emit(
                Kind::Err,
                format!("refused a connection from uid {}", cred.uid()),
            );
            return;
        }
        Err(_) => return,
    }
    let (read, mut write) = stream.split();
    let response = match read_line(read).await {
        Err(reason) => Response::Error(reason.into()),
        Ok(line) => {
            let work = tokio::task::spawn_blocking(move || answer(&line));
            match tokio::time::timeout(RUN_TIMEOUT, work).await {
                Ok(Ok(response)) => response,
                Ok(Err(_)) => Response::Error("the operation stopped".into()),
                Err(_) => Response::Error("the operation took too long".into()),
            }
        }
    };
    if let Ok(mut text) = serde_json::to_vec(&response) {
        text.push(b'\n');
        let _ = write.write_all(&text).await;
    }
}

/// serve connections on `listener` until `shutdown` resolves.
async fn accept_loop(
    listener: tokio::net::UnixListener,
    allowed_uid: u32,
    answer: Answer,
    shutdown: impl Future<Output = ()>,
) {
    let workers = Arc::new(Semaphore::new(WORKERS));
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => return,
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                // a small fixed pool: further connections wait their turn
                let Ok(permit) = workers.clone().acquire_owned().await else { return };
                let answer = answer.clone();
                tokio::spawn(async move {
                    handle(stream, allowed_uid, answer).await;
                    drop(permit);
                });
            }
        }
    }
}

/// run the service until it is stopped.
pub fn run(options: Options) -> Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(serve(options))
}

async fn serve(options: Options) -> Result<()> {
    let (listener, ours) = crate::webui::bind_socket(&options.socket).await?;
    log::emit(
        Kind::Info,
        format!(
            "operations service listening on {}",
            options.socket.display()
        ),
    );
    accept_loop(
        listener,
        options.allowed_uid,
        Arc::new(system_answer),
        crate::webui::stopped(),
    )
    .await;
    crate::webui::remove_if_ours(&options.socket, &ours).await;
    Ok(())
}

/// why the service could not be asked.
#[derive(Debug, PartialEq, Eq)]
pub enum Unreachable {
    /// nothing listens on the socket: the service is not running.
    Down,
    /// it was reached, but no whole answer came back; the operation may or may not have run.
    NoAnswer,
}

/// the future an ask returns.
pub type Asking = Pin<Box<dyn Future<Output = Result<Response, Unreachable>> + Send>>;

/// send `account`'s `op` to the service listening on `socket`, and read its answer.
pub fn ask(socket: &Path, account: String, op: Op) -> Asking {
    let socket = socket.to_path_buf();
    Box::pin(async move {
        let mut stream = tokio::net::UnixStream::connect(&socket)
            .await
            .map_err(|_| Unreachable::Down)?;
        let line = Forwarded { account, op }
            .to_line()
            .ok_or(Unreachable::NoAnswer)?;
        let exchange = async {
            stream.write_all(&line).await.ok()?;
            let mut text = Vec::new();
            (&mut stream)
                .take(ANSWER_LIMIT)
                .read_to_end(&mut text)
                .await
                .ok()?;
            serde_json::from_slice::<Response>(&text).ok()
        };
        tokio::time::timeout(RUN_TIMEOUT + Duration::from_secs(30), exchange)
            .await
            .ok()
            .flatten()
            .ok_or(Unreachable::NoAnswer)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanlord_core::access::{authorize, Grants};

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    /// the service's real check, with the account's group read replaced by `admin`.
    fn answer_as(admin: bool, line: &str) -> Response {
        answer_line(
            line.as_bytes(),
            &|account| {
                Ok(Caller {
                    user: account.to_string(),
                    admin,
                })
            },
            &|caller, op| {
                authorize(&Grants::fallback(caller.admin), op)?;
                Ok(Reply::Names(vec![caller.user.clone()]))
            },
        )
    }

    const RESET: &str =
        r#"{"account":"carol","op":{"op":"reset_password","user":"dave","new":"x"}}"#;
    const FACTS: &str = r#"{"account":"carol","op":{"op":"server_facts"}}"#;

    #[test]
    fn only_root_and_the_web_account_are_served() {
        assert!(served(0, 990));
        assert!(served(990, 990));
        assert!(!served(1000, 990));
        assert!(!served(991, 990));
    }

    #[test]
    fn the_service_checks_the_account_itself() {
        // whatever the web ui checked, an account without the right is refused here
        assert!(
            matches!(answer_as(false, RESET), Response::Denied(m) if m.contains("permission denied"))
        );
        assert!(matches!(answer_as(true, RESET), Response::Ok(_)));
        assert!(matches!(answer_as(false, FACTS), Response::Ok(_)));
    }

    #[test]
    fn an_unknown_account_is_refused() {
        let answer = answer_line(FACTS.as_bytes(), &|_| bail!("unknown account"), &|_, _| {
            Ok(Reply::Done)
        });
        assert!(matches!(answer, Response::Denied(_)));
    }

    #[test]
    fn a_garbled_request_is_an_error_and_runs_nothing() {
        for junk in [
            "",
            "nope",
            "{}",
            r#"{"account":"carol"}"#,
            r#"{"account":"carol","op":{"op":"zzz"}}"#,
        ] {
            assert!(
                matches!(answer_as(true, junk), Response::Error(m) if m == "not an operation"),
                "{junk}"
            );
        }
    }

    #[test]
    fn an_error_shows_one_short_line_not_the_chain() {
        let answer = answer_line(
            FACTS.as_bytes(),
            &|account| {
                Ok(Caller {
                    user: account.into(),
                    admin: true,
                })
            },
            &|_, _| {
                Err(anyhow::anyhow!("/etc/secret/path: stderr noise")
                    .context("reading the settings\nsecond line"))
            },
        );
        assert_eq!(answer, Response::Error("reading the settings".into()));
    }

    #[test]
    fn an_oversize_or_unfinished_request_is_refused() {
        runtime().block_on(async {
            let big = vec![b'a'; REQUEST_LIMIT + 10];
            assert_eq!(
                read_line(&big[..]).await.unwrap_err(),
                "the request is too large"
            );
            let mut huge = vec![b'a'; REQUEST_LIMIT + 10];
            huge.push(b'\n');
            assert!(read_line(&huge[..]).await.is_err());
            assert_eq!(
                read_line(&b"{}"[..]).await.unwrap_err(),
                "the request is not a whole line"
            );
            assert!(read_line(&b"{}\n"[..]).await.is_ok());
        });
    }

    fn our_uid() -> u32 {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata("/proc/self").unwrap().uid()
    }

    /// serve one request on a fresh socket, as a client with this process's uid.
    async fn serve_one(allowed_uid: u32, line: &str) -> Option<Response> {
        let dir =
            std::env::temp_dir().join(format!("lanlord-ops-{}-{allowed_uid}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("operations.sock");
        let (listener, _ours) = crate::webui::bind_socket(&socket).await.unwrap();
        let answer: Answer = Arc::new(|line| {
            answer_line(
                line,
                &|account| {
                    Ok(Caller {
                        user: account.into(),
                        admin: false,
                    })
                },
                &|_, _| Ok(Reply::Done),
            )
        });
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(accept_loop(listener, allowed_uid, answer, async {
            let _ = stopped.await;
        }));
        let mut stream = tokio::net::UnixStream::connect(&socket).await.unwrap();
        stream
            .write_all(format!("{line}\n").as_bytes())
            .await
            .unwrap();
        let mut text = Vec::new();
        let _ = stream.read_to_end(&mut text).await;
        let _ = stop.send(());
        server.await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        serde_json::from_slice(&text).ok()
    }

    #[test]
    fn a_connection_from_another_uid_is_dropped_without_an_answer() {
        if our_uid() == 0 {
            return; // root is always served
        }
        runtime().block_on(async {
            assert!(serve_one(our_uid() + 1, FACTS).await.is_none());
            // the same request from the allowed uid is answered
            assert!(matches!(
                serve_one(our_uid(), FACTS).await,
                Some(Response::Ok(Reply::Done))
            ));
        });
    }

    #[test]
    fn a_missing_socket_means_the_service_is_down() {
        runtime().block_on(async {
            let missing = Path::new("/nonexistent-lanlord/operations.sock");
            let asked = ask(missing, "carol".into(), Op::ServerFacts).await;
            assert_eq!(asked.unwrap_err(), Unreachable::Down);
        });
    }
}
