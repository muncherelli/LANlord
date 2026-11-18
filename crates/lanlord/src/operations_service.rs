//! `lanlord operations-service`: the root service that runs the operations the web ui forwards.
//! the web ui itself is sandboxed (no network, no privileges), so it sends each operation here
//! over a unix socket as one line of json, `{"account": ..., "op": ...}`. an apply sends progress
//! first, one `{"event": ...}` line per `ApplyEvent`; the last line is always
//! `{"response": ...}`, the same `Response` `lanlord op` prints.
//!
//! trust boundary: this service believes the web ui about which account is signed in; it does
//! not believe it about what that account may do. it reads the account's group membership again
//! and runs core's permission check itself before anything runs. what keeps others from claiming
//! to be the web ui is the socket's group and a uid check on every connection (`SO_PEERCRED`):
//! only the web ui's account and root are served.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::Duration;

use anyhow::{bail, Result};
use lanlord_core::access::{Caller, Denied, Refused};
use lanlord_core::apply::ApplyEvent;
use lanlord_core::log::{self, Kind};
use lanlord_core::ops::{execute_in, Op, Reply, Response, NO_KEY};
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

/// the answer for an op that reads secrets when the gateway has no settings key.
const NO_GATEWAY_KEY: &str =
    "the gateway has no settings key; operations that read secrets need one";

/// the answer for a second apply while one still runs.
const APPLY_RUNNING: &str = "an apply is already running";

/// held for as long as an apply runs. it is taken on the blocking thread that does the work, so
/// an apply the service stopped waiting for still holds it until it really ends.
static APPLY: Mutex<()> = Mutex::new(());

/// how the service is run.
pub struct Options {
    pub socket: PathBuf,
    /// the uid of the web ui's account; root is always served too
    pub allowed_uid: u32,
    /// the settings key; the service has no home folder to look in
    pub identity: PathBuf,
}

/// whether a peer with this uid is served.
fn served(peer: u32, allowed_uid: u32) -> bool {
    peer == 0 || peer == allowed_uid
}

/// the short text of an error: its outermost message on one line, never the chain of causes
/// (paths, command output). the whole chain goes to the log.
fn short(error: &anyhow::Error) -> String {
    let text = error.to_string();
    // the gateway keeps its key in its own place, not behind a terminal command
    let text = if text == NO_KEY {
        NO_GATEWAY_KEY.to_string()
    } else {
        text
    };
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

/// run `work` as the only apply when `op` is one: a second one is refused while the first runs,
/// and any other op is not held up.
fn single_apply<T>(lock: &Mutex<()>, op: &Op, work: impl FnOnce() -> Result<T>) -> Result<T> {
    if !matches!(op, Op::Apply { .. }) {
        return work();
    }
    let _running = match lock.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::Poisoned(guard)) => guard.into_inner(),
        Err(TryLockError::WouldBlock) => bail!(APPLY_RUNNING),
    };
    work()
}

/// sets the flag when dropped, whether the work ended or unwound.
struct Stop<'a>(&'a AtomicBool);

impl Drop for Stop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// run `work` and pass the apply events it publishes to `emit` as they happen. the log reaches
/// every thread's lines, so only events are taken; the rest still goes to the journal.
fn with_events<T>(emit: Emit, work: impl FnOnce() -> T) -> T {
    let lines = log::subscribe();
    let done = AtomicBool::new(false);
    let forward = |line: log::Line| match line.kind {
        Kind::Event => {
            if let Ok(event) = serde_json::from_str::<ApplyEvent>(&line.text) {
                emit(&event);
            }
        }
        Kind::Err | Kind::Info => eprintln!("{}", line.text),
        _ => {}
    };
    std::thread::scope(|scope| {
        let (done, forward) = (&done, &forward);
        let forwarder = scope.spawn(move || {
            while !done.load(Ordering::SeqCst) {
                if let Ok(line) = lines.recv_timeout(Duration::from_millis(50)) {
                    forward(line);
                }
            }
            lines.try_iter().for_each(forward);
        });
        // set even when the work panics, or the forwarder would never stop and the scope never end
        let result = {
            let _stop = Stop(done);
            work()
        };
        let _ = forwarder.join();
        result
    })
}

/// the real thing: the account is read from this machine's groups, and the op runs locally as
/// root through core's `execute_in`, which checks `Op::requires`.
fn system_answer(line: &[u8], emit: Emit) -> Response {
    let session = Session::local();
    answer_line(
        line,
        &|account| Caller::of_account(&session, account),
        &|caller, op| {
            let mut ask =
                |_: &str| -> Result<String> { bail!("no sudo prompt: the service is root") };
            let mut run = || execute_in(&session, "localhost", caller, op, &mut ask);
            single_apply(&APPLY, op, || match op {
                Op::Apply { .. } => with_events(emit, &mut run),
                _ => run(),
            })
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

/// how an answer is produced from a request line, handing apply events to the second argument as
/// they happen; blocking, so it runs off the async threads.
type Answer = Arc<dyn Fn(&[u8], Emit) -> Response + Send + Sync>;

/// where an op's apply events are handed as they happen.
type Emit<'a> = &'a (dyn Fn(&ApplyEvent) + Sync);

/// one line of the service's answer: an apply event.
fn event_line(event: &ApplyEvent) -> Option<Vec<u8>> {
    let mut line = serde_json::to_vec(&serde_json::json!({ "event": event })).ok()?;
    line.push(b'\n');
    Some(line)
}

/// the last line of the service's answer.
fn response_line(response: &Response) -> Option<Vec<u8>> {
    let mut line = serde_json::to_vec(&serde_json::json!({ "response": response })).ok()?;
    line.push(b'\n');
    Some(line)
}

/// serve one connection: a peer that is not ours is dropped at once, before it can take a slot.
/// the work holds its slot (`workers`) for as long as its thread runs, even when we stop
/// waiting for it.
async fn handle(
    mut stream: tokio::net::UnixStream,
    allowed_uid: u32,
    answer: Answer,
    workers: Arc<Semaphore>,
) {
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
            // a small fixed pool: further requests wait their turn
            let Ok(permit) = workers.acquire_owned().await else {
                return;
            };
            let (events, mut seen) = tokio::sync::mpsc::unbounded_channel::<ApplyEvent>();
            // the thread is not stopped when we stop waiting: it ends with the work, and holds
            // whatever it took (its slot, the apply lock) until then
            let mut work = tokio::task::spawn_blocking(move || {
                let _slot = permit;
                answer(&line, &|event| {
                    let _ = events.send(event.clone());
                })
            });
            let deadline = tokio::time::sleep(RUN_TIMEOUT);
            tokio::pin!(deadline);
            let response = loop {
                tokio::select! {
                    biased;
                    Some(event) = seen.recv() => {
                        if let Some(line) = event_line(&event) {
                            let _ = write.write_all(&line).await;
                        }
                    }
                    done = &mut work => {
                        break match done {
                            Ok(response) => response,
                            Err(_) => Response::Error("the operation stopped".into()),
                        };
                    }
                    () = &mut deadline => {
                        break Response::Error("the operation took too long".into());
                    }
                }
            };
            // what was published just before the work ended still goes out first
            while let Ok(event) = seen.try_recv() {
                if let Some(line) = event_line(&event) {
                    let _ = write.write_all(&line).await;
                }
            }
            response
        }
    };
    if let Some(line) = response_line(&response) {
        let _ = write.write_all(&line).await;
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
                // waiting for a slot happens in the connection's own task, so a full pool
                // neither stops accepting nor delays shutdown
                tokio::spawn(handle(stream, allowed_uid, answer.clone(), workers.clone()));
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
    lanlord_core::crypto::set_default_path(options.identity.clone());
    if !options.identity.exists() {
        log::emit(
            Kind::Info,
            format!(
                "no settings key at {}: operations that read secrets will say so",
                options.identity.display()
            ),
        );
    }
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

/// where the apply events of an op go.
pub type Events = Arc<dyn Fn(ApplyEvent) + Send + Sync>;

/// read a service answer: each event goes to `on_event` in order, and the response ends it. none
/// when the stream ends or breaks before a whole response.
async fn read_answer<R: tokio::io::AsyncRead + Unpin>(
    stream: R,
    on_event: &(dyn Fn(ApplyEvent) + Sync),
) -> Option<Response> {
    let mut lines = BufReader::new(stream.take(ANSWER_LIMIT)).lines();
    while let Some(line) = lines.next_line().await.ok()? {
        let mut value: serde_json::Value = serde_json::from_str(&line).ok()?;
        let fields = value.as_object_mut()?;
        if let Some(event) = fields.remove("event") {
            on_event(serde_json::from_value(event).ok()?);
        } else {
            return serde_json::from_value(fields.remove("response")?).ok();
        }
    }
    None
}

/// send `account`'s `op` to the service listening on `socket`, pass the apply events on to
/// `events` as they come, and read its answer.
pub fn ask(socket: &Path, account: String, op: Op, events: Events) -> Asking {
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
            read_answer(&mut stream, &|event| events(event)).await
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

    /// serve one request on a fresh socket, as a client with this process's uid, and return
    /// everything the service wrote.
    async fn serve_text(allowed_uid: u32, line: &str, answer: Answer) -> Vec<u8> {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "lanlord-ops-{}-{allowed_uid}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("operations.sock");
        let (listener, _ours) = crate::webui::bind_socket(&socket).await.unwrap();
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
        text
    }

    async fn serve_one(allowed_uid: u32, line: &str) -> Option<Response> {
        let answer: Answer = Arc::new(|line, _| {
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
        let text = serve_text(allowed_uid, line, answer).await;
        read_answer(&text[..], &|_| {}).await
    }

    #[test]
    fn apply_events_arrive_in_order_before_the_response() {
        runtime().block_on(async {
            let sent = [
                ApplyEvent::Started,
                ApplyEvent::Total { n: 2 },
                ApplyEvent::TaskStart { name: "one".into() },
            ];
            let to_send = sent.clone();
            let answer: Answer = Arc::new(move |_, emit| {
                for event in &to_send {
                    emit(event);
                }
                Response::Ok(Reply::Done)
            });
            let text = serve_text(our_uid(), FACTS, answer).await;
            // on the wire: the events, then the one response line
            let lines: Vec<&str> = std::str::from_utf8(&text).unwrap().lines().collect();
            assert_eq!(lines.len(), 4, "{lines:?}");
            assert!(lines[..3].iter().all(|l| l.starts_with(r#"{"event":"#)));
            assert!(lines[3].starts_with(r#"{"response":"#));
            // and read back the way the web ui does
            let got = Mutex::new(Vec::new());
            let response = read_answer(&text[..], &|e| got.lock().unwrap().push(e)).await;
            assert_eq!(*got.lock().unwrap(), sent);
            assert_eq!(response, Some(Response::Ok(Reply::Done)));
        });
    }

    #[test]
    fn a_response_before_an_event_ends_the_answer() {
        runtime().block_on(async {
            let mut text = response_line(&Response::Ok(Reply::Done)).unwrap();
            text.extend(event_line(&ApplyEvent::Started).unwrap());
            let got = Mutex::new(0);
            let response = read_answer(&text[..], &|_| *got.lock().unwrap() += 1).await;
            assert!(response.is_some());
            assert_eq!(*got.lock().unwrap(), 0);
            // no response at all is no answer
            assert!(
                read_answer(&b"{\"event\":{\"event\":\"started\"}}\n"[..], &|_| {})
                    .await
                    .is_none()
            );
        });
    }

    #[test]
    fn a_panicking_apply_ends_cleanly_and_frees_the_lock() {
        let lock = Mutex::new(());
        let apply = Op::Apply { modules: vec![] };
        // on its own thread, so a forwarder that never stops fails the test instead of hanging it
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let panicked = std::panic::catch_unwind(|| {
                    let _ = single_apply(&lock, &apply, || {
                        with_events(&|_| {}, || -> Result<()> { panic!("the apply broke") })
                    });
                });
                done.send(panicked.is_err()).unwrap();
            });
            let panicked = finished
                .recv_timeout(Duration::from_secs(10))
                .expect("the panicking apply never ended");
            assert!(panicked);
        });
        // the next apply is not "already running"
        assert!(single_apply(&lock, &apply, || Ok(())).is_ok());
    }

    #[test]
    fn a_second_apply_is_refused_while_one_runs() {
        let lock = Arc::new(Mutex::new(()));
        let apply = Op::Apply { modules: vec![] };
        let (started, wait_started) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        // the first apply, on its own thread as the service runs it
        let first = {
            let (lock, apply) = (lock.clone(), apply.clone());
            std::thread::spawn(move || {
                single_apply(&lock, &apply, || {
                    started.send(()).unwrap();
                    released.recv().unwrap();
                    Ok(())
                })
            })
        };
        wait_started.recv().unwrap();
        // whoever waited for it has gone (a timeout), but the thread still runs: still refused
        let second = single_apply(&lock, &apply, || Ok(())).unwrap_err();
        assert_eq!(second.to_string(), "an apply is already running");
        // other operations are not held up
        assert!(single_apply(&lock, &Op::ServerFacts, || Ok(())).is_ok());
        release.send(()).unwrap();
        first.join().unwrap().unwrap();
        assert!(single_apply(&lock, &apply, || Ok(())).is_ok());
    }

    #[test]
    fn a_missing_settings_key_says_so_and_other_operations_run() {
        let lookup = |account: &str| {
            Ok(Caller {
                user: account.into(),
                admin: true,
            })
        };
        let no_key = answer_line(FACTS.as_bytes(), &lookup, &|_, _| bail!(NO_KEY));
        assert_eq!(
            no_key,
            Response::Error(
                "the gateway has no settings key; operations that read secrets need one".into()
            )
        );
        let without_secrets = answer_line(FACTS.as_bytes(), &lookup, &|_, _| Ok(Reply::Done));
        assert_eq!(without_secrets, Response::Ok(Reply::Done));
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
            let asked = ask(missing, "carol".into(), Op::ServerFacts, Arc::new(|_| {})).await;
            assert_eq!(asked.unwrap_err(), Unreachable::Down);
        });
    }
}
