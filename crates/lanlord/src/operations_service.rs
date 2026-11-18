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
//! only the web ui's account and root are served there.
//!
//! a second socket serves the accounts of this machine (the desktop and the tui on the gateway).
//! its folder can be searched by anyone and the socket can be connected to by anyone, because
//! the kernel's word on who connected is the whole check: a peer whose uid is not an ordinary
//! login account (`login_uids`) is dropped before it can take a slot, and one that is, is
//! served as the account that uid belongs to. the account is never read from the request: a
//! request that names a different account is refused. what the account may do is checked as
//! for everyone.
//!
//! the gateway's settings key is created here on first start (root only) and used only here (and
//! in `lanlord operate`): secrets are decrypted inside this process, for operations the account
//! may run, and never leave it.

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
use lanlord_core::ops::{execute_in, login_uids, Op, Reply, Response};
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
pub const RUN_TIMEOUT: Duration = lanlord_core::ops::RUN_LIMIT;

/// how many operations run at the same time.
const WORKERS: usize = 4;
/// operations the accounts of this machine may run at once, in a pool of their own so the web
/// ui always has its own slots.
const LOCAL_WORKERS: usize = 2;
/// connections held open at once on a socket, and by one local uid.
const CONNECTIONS: usize = 64;
const PER_UID_CONNECTIONS: usize = 4;
/// the pause after a failed accept (out of file descriptors, say), so a failure never spins.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// what a socket may use up: its own pool of operation slots, a cap on open connections, and a
/// cap per local uid. connections over a cap are dropped at once, before any work is done for
/// them (before the account lookup, which forks).
#[derive(Clone)]
struct Limits {
    workers: Arc<Semaphore>,
    connections: Arc<Semaphore>,
    per_uid: usize,
    held: Arc<Mutex<std::collections::HashMap<u32, usize>>>,
}

impl Limits {
    fn new(workers: usize, connections: usize, per_uid: usize) -> Self {
        Self {
            workers: Arc::new(Semaphore::new(workers)),
            connections: Arc::new(Semaphore::new(connections)),
            per_uid,
            held: Arc::default(),
        }
    }

    fn web() -> Self {
        Self::new(WORKERS, CONNECTIONS, usize::MAX)
    }

    fn local() -> Self {
        Self::new(LOCAL_WORKERS, CONNECTIONS, PER_UID_CONNECTIONS)
    }

    /// a place for one more connection from `uid`, or `None` when it has its share.
    fn hold(&self, uid: u32) -> Option<UidHold> {
        let mut held = self
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let count = held.entry(uid).or_default();
        if *count >= self.per_uid {
            return None;
        }
        *count += 1;
        Some(UidHold {
            uid,
            held: self.held.clone(),
        })
    }
}

/// one connection's place in a uid's share; freed on drop.
struct UidHold {
    uid: u32,
    held: Arc<Mutex<std::collections::HashMap<u32, usize>>>,
}

impl Drop for UidHold {
    fn drop(&mut self) {
        let mut held = self
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(count) = held.get_mut(&self.uid) {
            *count -= 1;
            if *count == 0 {
                held.remove(&self.uid);
            }
        }
    }
}

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

/// the answer for a second apply while one still runs.
const APPLY_RUNNING: &str = "an apply is already running";

/// held for as long as an apply runs. it is taken on the blocking thread that does the work, so
/// an apply the service stopped waiting for still holds it until it really ends.
static APPLY: Mutex<()> = Mutex::new(());

/// how the service is run.
pub struct Options {
    pub socket: PathBuf,
    /// where the accounts of this machine connect, each served as itself
    pub local_socket: PathBuf,
    /// the uid of the web ui's account; root is always served too
    pub allowed_uid: u32,
    /// the gateway's settings key, made on first start; the service has no home folder
    pub identity: PathBuf,
}

/// who may connect to a socket.
#[derive(Clone, Debug)]
pub enum Audience {
    /// the web socket: root and the web ui's account, which name the account they forward.
    Forwarders { allowed_uid: u32 },
    /// the local socket: the ordinary login accounts of this machine, each as itself.
    LoginAccounts { uids: std::ops::RangeInclusive<u32> },
}

/// how a connected peer is served.
#[derive(Debug, PartialEq, Eq)]
enum Standing {
    /// it forwards a request that names the account.
    Forwarder,
    /// it is this uid's account, whatever the request says.
    Itself(u32),
}

/// how the uid of a peer is served by a socket, or `None` when it is not.
fn standing(audience: &Audience, peer: u32) -> Option<Standing> {
    match audience {
        Audience::Forwarders { allowed_uid } => {
            (peer == 0 || peer == *allowed_uid).then_some(Standing::Forwarder)
        }
        Audience::LoginAccounts { uids } => uids.contains(&peer).then_some(Standing::Itself(peer)),
    }
}

/// the account of a uid on this machine, or `None`.
type Names = Arc<dyn Fn(u32) -> Option<String> + Send + Sync>;

/// the account name of `uid`, from the machine's own account database.
fn account_of_uid(uid: u32) -> Option<String> {
    let found = Session::local()
        .capture(&["getent".into(), "passwd".into(), uid.to_string()])
        .ok()?;
    let name = found.lines().next()?.split(':').next()?.to_string();
    lanlord_core::access::plain_account(&name).then_some(name)
}

/// the request of a peer that is `account` itself, in the form the service reads: the op,
/// with the account taken from the kernel. only exactly `{"op":..}` or `{"account":<self>,"op":..}`
/// is accepted; anything else, however it is wrong, is refused and never passed on.
fn bind_account(line: &[u8], account: &str) -> Result<Vec<u8>, String> {
    const NOT_AN_OPERATION: &str = "not an operation";
    let mut value: serde_json::Value =
        serde_json::from_slice(line).map_err(|_| NOT_AN_OPERATION.to_string())?;
    let fields = value
        .as_object_mut()
        .ok_or_else(|| NOT_AN_OPERATION.to_string())?;
    if let Some(named) = fields.remove("account") {
        if named.as_str() != Some(account) {
            return Err("a request here acts as your own account, not another".into());
        }
    }
    let op = fields
        .remove("op")
        .filter(|_| fields.is_empty())
        .and_then(|op| serde_json::from_value(op).ok())
        .ok_or_else(|| NOT_AN_OPERATION.to_string())?;
    Forwarded {
        account: account.to_string(),
        op,
    }
    .to_line()
    .ok_or_else(|| NOT_AN_OPERATION.to_string())
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
    audience: Audience,
    names: Names,
    answer: Answer,
    (limits, _connection): (Limits, tokio::sync::OwnedSemaphorePermit),
) {
    let standing = match stream.peer_cred() {
        Ok(cred) => match standing(&audience, cred.uid()) {
            Some(standing) => standing,
            None => {
                log::emit(
                    Kind::Err,
                    format!("refused a connection from uid {}", cred.uid()),
                );
                return;
            }
        },
        Err(_) => return,
    };
    // a login uid has its share of connections; over it, dropped before anything is looked up
    let _share = match standing {
        Standing::Itself(uid) => match limits.hold(uid) {
            Some(hold) => Some(hold),
            None => {
                log::emit(Kind::Err, format!("too many connections from uid {uid}"));
                return;
            }
        },
        Standing::Forwarder => None,
    };
    let workers = limits.workers.clone();
    // the account a login peer acts as: whoever the kernel says it is
    let account = match standing {
        Standing::Forwarder => None,
        Standing::Itself(uid) => {
            match tokio::task::spawn_blocking(move || names(uid))
                .await
                .ok()
                .flatten()
            {
                Some(account) => Some(account),
                None => {
                    log::emit(Kind::Err, format!("uid {uid} has no account name here"));
                    return;
                }
            }
        }
    };
    let (read, mut write) = stream.split();
    let response = match read_line(read).await {
        Err(reason) => Response::Error(reason.into()),
        Ok(line) => {
            let line = match &account {
                Some(account) => match bind_account(&line, account) {
                    Ok(line) => line,
                    Err(refused) => {
                        let _ = write_response(&mut write, &Response::Denied(refused)).await;
                        return;
                    }
                },
                None => line,
            };
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
    let _ = write_response(&mut write, &response).await;
}

/// write the last line of an answer.
async fn write_response<W: tokio::io::AsyncWrite + Unpin>(
    write: &mut W,
    response: &Response,
) -> std::io::Result<()> {
    match response_line(response) {
        Some(line) => write.write_all(&line).await,
        None => Ok(()),
    }
}

/// serve connections on `listener` until `shutdown` resolves, within `limits`.
async fn accept_loop(
    listener: tokio::net::UnixListener,
    (audience, names): (Audience, Names),
    answer: Answer,
    limits: Limits,
    shutdown: impl Future<Output = ()>,
) {
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => return,
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else {
                    // no descriptors left, or the like: wait, never spin
                    tokio::time::sleep(ACCEPT_BACKOFF).await;
                    continue;
                };
                // over the socket's connection cap, drop it at once
                let Ok(connection) = limits.connections.clone().try_acquire_owned() else {
                    continue;
                };
                // waiting for a slot happens in the connection's own task, so a full pool
                // neither stops accepting nor delays shutdown
                tokio::spawn(handle(
                    stream,
                    audience.clone(),
                    names.clone(),
                    answer.clone(),
                    (limits.clone(), connection),
                ));
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
    serve_until(options, crate::webui::stopped()).await
}

/// the service on its sockets until `stop` resolves; then the sockets are removed.
async fn serve_until(options: Options, stop_signal: impl Future<Output = ()>) -> Result<()> {
    // the gateway's own key: made here on first start, root only, and used only in this process
    if let Err(error) = lanlord_core::crypto::use_gateway_key(options.identity.clone()) {
        log::emit(
            Kind::Err,
            format!(
                "no settings key at {}: operations that read secrets will say so ({error:#})",
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
    // the accounts of this machine; the web socket works without it when it cannot be made
    let local = match crate::webui::bind_socket_mode(&options.local_socket, 0o666).await {
        Ok(bound) => Some(bound),
        Err(error) => {
            log::emit(Kind::Err, format!("no local socket: {error:#}"));
            None
        }
    };
    let answer: Answer = Arc::new(system_answer);
    let names: Names = Arc::new(account_of_uid);
    let (stop, stopping) = tokio::sync::watch::channel(false);
    let web = accept_loop(
        listener,
        (
            Audience::Forwarders {
                allowed_uid: options.allowed_uid,
            },
            names.clone(),
        ),
        answer.clone(),
        Limits::web(),
        wait_for(stopping.clone()),
    );
    let (local_listener, local_ours) = match local {
        Some((listener, ours)) => (Some(listener), Some(ours)),
        None => (None, None),
    };
    let here = async {
        if let Some(listener) = local_listener {
            let audience = Audience::LoginAccounts { uids: login_uids() };
            accept_loop(
                listener,
                (audience, names.clone()),
                answer.clone(),
                Limits::local(),
                wait_for(stopping.clone()),
            )
            .await;
        }
    };
    let stopper = async {
        stop_signal.await;
        let _ = stop.send(true);
    };
    tokio::join!(web, here, stopper);
    crate::webui::remove_if_ours(&options.socket, &ours).await;
    if let Some(ours) = &local_ours {
        crate::webui::remove_if_ours(&options.local_socket, ours).await;
    }
    Ok(())
}

/// resolves once the flag is set.
async fn wait_for(mut flag: tokio::sync::watch::Receiver<bool>) {
    let _ = flag.wait_for(|stopped| *stopped).await;
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
    use lanlord_core::ops::NO_KEY;

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
    fn only_root_and_the_web_account_are_served_on_the_web_socket() {
        let web = Audience::Forwarders { allowed_uid: 990 };
        assert_eq!(standing(&web, 0), Some(Standing::Forwarder));
        assert_eq!(standing(&web, 990), Some(Standing::Forwarder));
        assert_eq!(standing(&web, 1000), None);
        assert_eq!(standing(&web, 991), None);
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
        let web = Audience::Forwarders { allowed_uid };
        serve_audience((web, Arc::new(|_| None)), line, answer).await
    }

    /// as `serve_text`, on a socket for `audience`; `tag` makes the socket's folder its own.
    async fn serve_audience(
        (audience, names): (Audience, Names),
        line: &str,
        answer: Answer,
    ) -> Vec<u8> {
        let (dir, socket, listener) = test_socket().await;
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let workers = Limits::web();
        let server = tokio::spawn(accept_loop(
            listener,
            (audience, names),
            answer,
            workers,
            async {
                let _ = stopped.await;
            },
        ));
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

    async fn test_socket() -> (PathBuf, PathBuf, tokio::net::UnixListener) {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "lanlord-ops-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // searchable by all, as the service's own folder is
        std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let socket = dir.join("local.sock");
        let (listener, _ours) = crate::webui::bind_socket_mode(&socket, 0o666)
            .await
            .unwrap();
        (dir, socket, listener)
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

    /// an answer that says what request it was given, after the account was bound to it.
    fn echo() -> Answer {
        Arc::new(|line, _| Response::Error(String::from_utf8_lossy(line).trim().to_string()))
    }

    fn login_audience(name: &'static str) -> (Audience, Names) {
        let uid = our_uid();
        (
            Audience::LoginAccounts { uids: uid..=uid },
            Arc::new(move |_| Some(name.to_string())),
        )
    }

    const LOAD: &str = r#"{"op":{"op":"load_document"}}"#;

    #[test]
    fn a_login_peer_is_served_as_the_account_its_uid_belongs_to() {
        runtime().block_on(async {
            let text = serve_audience(login_audience("ann"), LOAD, echo()).await;
            let response = read_answer(&text[..], &|_| {}).await;
            let Some(Response::Error(seen)) = response else {
                panic!("{response:?}")
            };
            let seen = Forwarded::from_line(seen.as_bytes()).expect("a bound request");
            assert_eq!(seen.account, "ann");
            assert!(matches!(seen.op, Op::LoadDocument));
        });
    }

    #[test]
    fn a_login_peer_cannot_act_as_another_account() {
        runtime().block_on(async {
            for other in ["root", "bob"] {
                let line = format!(r#"{{"account":"{other}","op":{{"op":"load_document"}}}}"#);
                let text = serve_audience(login_audience("ann"), &line, echo()).await;
                let response = read_answer(&text[..], &|_| {}).await;
                assert!(matches!(response, Some(Response::Denied(_))), "{other}");
            }
            // naming itself is fine
            let line = r#"{"account":"ann","op":{"op":"load_document"}}"#;
            let text = serve_audience(login_audience("ann"), line, echo()).await;
            assert!(matches!(
                read_answer(&text[..], &|_| {}).await,
                Some(Response::Error(_))
            ));
        });
    }

    #[test]
    fn a_peer_outside_the_login_range_is_dropped_on_the_local_socket() {
        runtime().block_on(async {
            let outside = (
                Audience::LoginAccounts {
                    uids: our_uid() + 1..=our_uid() + 2,
                },
                Arc::new(|_| Some("ann".to_string())) as Names,
            );
            let text = serve_audience(outside, LOAD, echo()).await;
            assert!(text.is_empty());
        });
    }

    #[test]
    fn a_uid_without_an_account_name_is_dropped() {
        runtime().block_on(async {
            let uid = our_uid();
            let nameless = (
                Audience::LoginAccounts { uids: uid..=uid },
                Arc::new(|_| None) as Names,
            );
            assert!(serve_audience(nameless, LOAD, echo()).await.is_empty());
        });
    }

    #[test]
    fn the_account_binding_refuses_anything_but_an_op_alone_or_with_its_own_name() {
        for bad in [
            &b"not json\n"[..],
            br#"[1]"#,
            br#"{"op":{"op":"load_document"},"x":1}"#,
            br#"{"account":"ann"}"#,
            br#"{"op":{"op":"no_such_operation"}}"#,
            br#"{"account":"bob","op":{"op":"load_document"}}"#,
        ] {
            assert!(bind_account(bad, "ann").is_err(), "{bad:?}");
        }
        for good in [
            &br#"{"op":{"op":"load_document"}}"#[..],
            br#"{"account":"ann","op":{"op":"load_document"}}"#,
        ] {
            assert!(bind_account(good, "ann").is_ok());
        }
    }

    #[test]
    fn the_service_answers_on_both_sockets_and_removes_them_when_stopped() {
        runtime().block_on(async {
            let dir = std::env::temp_dir().join(format!("lanlord-serve-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let options = Options {
                socket: dir.join("operations.sock"),
                local_socket: dir.join("local.sock"),
                allowed_uid: our_uid(),
                // a key that cannot be made: the service says so and runs without one
                identity: "/proc/lanlord-no-such-folder/identity".into(),
            };
            let (socket, local) = (options.socket.clone(), options.local_socket.clone());
            let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
            let service = tokio::spawn(serve_until(options, async {
                let _ = stopped.await;
            }));
            for _ in 0..100 {
                if socket.exists() && local.exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            // the web socket serves its allowed uid: a garbled request is answered as such
            let mut web = tokio::net::UnixStream::connect(&socket).await.unwrap();
            web.write_all(b"garbled\n").await.unwrap();
            let mut text = Vec::new();
            web.read_to_end(&mut text).await.unwrap();
            assert!(String::from_utf8_lossy(&text).contains("response"));
            // the local socket takes the connection too (what it says depends on the account)
            let mut here = tokio::net::UnixStream::connect(&local).await.unwrap();
            here.write_all(b"garbled\n").await.unwrap();
            let mut text = Vec::new();
            let _ = here.read_to_end(&mut text).await;
            let _ = stop.send(());
            service.await.unwrap().unwrap();
            assert!(!socket.exists() && !local.exists());
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn a_uid_holds_only_its_share_of_connections_and_frees_it_on_drop() {
        let limits = Limits::new(1, 8, 2);
        let first = limits.hold(1000).unwrap();
        let _second = limits.hold(1000).unwrap();
        assert!(limits.hold(1000).is_none());
        // another uid is not held up by it
        assert!(limits.hold(1001).is_some());
        drop(first);
        assert!(limits.hold(1000).is_some());
    }

    #[test]
    fn the_web_and_local_sockets_have_pools_of_their_own() {
        let (web, local) = (Limits::web(), Limits::local());
        assert!(!Arc::ptr_eq(&web.workers, &local.workers));
        assert!(!Arc::ptr_eq(&web.connections, &local.connections));
        // using up the local pool leaves the web's untouched
        let _held: Vec<_> = (0..LOCAL_WORKERS)
            .map(|_| local.workers.clone().try_acquire_owned().unwrap())
            .collect();
        assert!(local.workers.clone().try_acquire_owned().is_err());
        assert!(web.workers.clone().try_acquire_owned().is_ok());
    }

    #[test]
    fn connections_over_the_cap_are_dropped_at_once_and_the_service_keeps_answering() {
        runtime().block_on(async {
            let (dir, socket, listener) = test_socket().await;
            let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
            let uid = our_uid();
            let audience = (
                Audience::LoginAccounts { uids: uid..=uid },
                Arc::new(|_| Some("ann".to_string())) as Names,
            );
            let server = tokio::spawn(accept_loop(
                listener,
                audience,
                echo(),
                Limits::new(1, 2, 1),
                async {
                    let _ = stopped.await;
                },
            ));
            // two idle connections take the whole share (one per uid), the rest are dropped
            let idle = tokio::net::UnixStream::connect(&socket).await.unwrap();
            let mut dropped = tokio::net::UnixStream::connect(&socket).await.unwrap();
            let mut text = Vec::new();
            let read =
                tokio::time::timeout(Duration::from_secs(5), dropped.read_to_end(&mut text)).await;
            assert!(read.is_ok(), "the extra connection was not dropped");
            assert!(text.is_empty());
            // once the idle one goes, a new connection is served
            drop(idle);
            tokio::time::sleep(Duration::from_millis(100)).await;
            let mut again = tokio::net::UnixStream::connect(&socket).await.unwrap();
            again
                .write_all(format!("{LOAD}\n").as_bytes())
                .await
                .unwrap();
            let mut text = Vec::new();
            let _ = again.read_to_end(&mut text).await;
            assert!(!text.is_empty());
            let _ = stop.send(());
            server.await.unwrap();
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// as root: make a login account, connect to the local socket as that account and check the
    /// service names it from its uid, and refuses a request that names root. the ci job runs it
    /// as root; elsewhere it is skipped.
    #[test]
    #[ignore = "needs root"]
    fn root_a_local_peer_is_served_as_its_own_account_and_cannot_name_root() {
        let local = Session::local();
        let sh = |args: &[&str]| {
            let argv: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            local.capture(&argv).unwrap()
        };
        let user = format!("lltest{}", std::process::id());
        sh(&["useradd", "-m", "-s", "/bin/sh", &user]);
        let ids = sh(&["id", "-u", &user]).trim().to_string();
        let gid = sh(&["id", "-g", &user]).trim().to_string();
        let uid: u32 = ids.parse().unwrap();
        // the real login range and the real account lookup
        assert!(login_uids().contains(&uid), "{uid} is not a login uid");
        let audience = (
            Audience::LoginAccounts { uids: login_uids() },
            Arc::new(account_of_uid) as Names,
        );
        let (as_named, as_root) = runtime().block_on(async {
            let (dir, socket, listener) = test_socket().await;
            let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
            let workers = Limits::web();
            let server = tokio::spawn(accept_loop(listener, audience, echo(), workers, async {
                let _ = stopped.await;
            }));
            // a client with the new account's uid, through python on the runner
            let talk = |request: String| {
                let (socket, ids, gid) = (socket.clone(), ids.clone(), gid.clone());
                tokio::task::spawn_blocking(move || {
                    let script = "import socket,sys;s=socket.socket(socket.AF_UNIX);\
                        s.connect(sys.argv[1]);s.sendall(sys.argv[2].encode()+b'\\n');\
                        print(s.makefile().read())";
                    let out = std::process::Command::new("setpriv")
                        .args(["--reuid", &ids, "--regid", &gid, "--clear-groups"])
                        .args(["python3", "-c", script])
                        .arg(&socket)
                        .arg(&request)
                        .output()
                        .unwrap();
                    String::from_utf8_lossy(&out.stdout).to_string()
                })
            };
            let named = talk(LOAD.to_string()).await.unwrap();
            let root = talk(r#"{"account":"root","op":{"op":"load_document"}}"#.to_string())
                .await
                .unwrap();
            let _ = stop.send(());
            server.await.unwrap();
            let _ = std::fs::remove_dir_all(&dir);
            (named, root)
        });
        sh(&["userdel", "-r", &user]);
        assert!(
            as_named.contains(&format!(r#"\"account\":\"{user}\""#)),
            "{as_named}"
        );
        assert!(as_root.contains("denied"), "{as_root}");
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
