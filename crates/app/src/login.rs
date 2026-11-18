//! the logon box: the first screen, before any gateway is reached. it asks for a user name and a
//! host name and signs you in with your own ssh credentials. nothing is stored here but the list
//! of servers; a password you type lives in memory for one attempt and is wiped after it.

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

#[cfg(feature = "native")]
use lanlord_core::operate::{is_this_server, offered_at, this_server_target, LOCAL_SOCKET};
#[cfg(feature = "native")]
use lanlord_core::signin::{local_identity, Attempt, Signin};
use lanlord_model::signin::{Identity, SigninError};
use zeroize::Zeroizing;

use crate::message::MessageBox;
use crate::view::{Button, Dialog, Node, Size};
use crate::wake::Wake;

/// how the logon box reaches a server; the real one uses the system ssh, tests use a fake.
pub trait Auth: Send + Sync {
    fn attempt(
        &self,
        target: &str,
        password: Option<&str>,
        trust_new_host: bool,
    ) -> Result<Identity, SigninError>;
    /// the host key the server shows, for the user to check before trusting it.
    fn fingerprint(&self, target: &str) -> Result<String, SigninError>;
    /// close the connection kept open for `target`.
    fn sign_out(&self, target: &str);
    /// the target that means "this server, signed in as whoever runs this program", when the
    /// gateway's operations socket is here to carry it; `None` on any other machine.
    fn this_server(&self) -> Option<String> {
        None
    }
}

/// a future that runs on the page's one thread.
pub type LocalFuture<T> = Pin<Box<dyn Future<Output = T>>>;

/// how the browser's logon box reaches the web ui: the gateway's own accounts, one server.
/// nothing here is `Send`: in the browser there is one thread, and requests are awaited.
pub trait WebSession {
    /// log on with a user name and password; the answer is who the server says you are.
    fn login(
        &self,
        user: String,
        password: Zeroizing<String>,
    ) -> LocalFuture<Result<Identity, SigninError>>;
    /// who is logged on already (the cookie is still good): `Ok(None)` when the server says
    /// nobody is, an error when it could not be asked.
    fn current(&self) -> LocalFuture<anyhow::Result<Option<Identity>>>;
    /// end the session on the server.
    fn logout(&self) -> LocalFuture<()>;
}

/// what the web ui's `/api/login` answered, as the logon box shows it.
pub fn login_status(status: u16) -> Result<(), SigninError> {
    match status {
        200 => Ok(()),
        401 => Err(SigninError::WrongPassword),
        503 => Err(SigninError::Other(
            "The logon service on the server is not available.".into(),
        )),
        other => Err(SigninError::Other(format!(
            "The server refused the logon ({other})."
        ))),
    }
}

/// run `work` on the page's thread. the browser has an event loop for it; elsewhere (tests) it
/// is polled once, which is enough for the ready futures the fakes return.
pub fn spawn_local(work: impl Future<Output = ()> + 'static) {
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(work);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut work = Box::pin(work);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        let _ = work.as_mut().poll(&mut cx);
    }
}

#[cfg(feature = "native")]
pub struct SystemAuth(Signin);

#[cfg(feature = "native")]
impl Default for SystemAuth {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "native")]
impl SystemAuth {
    /// sign in with the system ssh.
    pub fn new() -> Self {
        Self(Signin::system())
    }
}

#[cfg(feature = "native")]
impl Auth for SystemAuth {
    fn attempt(
        &self,
        target: &str,
        password: Option<&str>,
        trust_new_host: bool,
    ) -> Result<Identity, SigninError> {
        // this machine: the account is the one running this program, no ssh
        if is_this_server(target) {
            return local_identity();
        }
        self.0.attempt(
            target,
            &Attempt {
                password,
                trust_new_host,
            },
        )
    }

    fn fingerprint(&self, target: &str) -> Result<String, SigninError> {
        self.0.fingerprint(target)
    }

    fn sign_out(&self, target: &str) {
        if !is_this_server(target) {
            self.0.sign_out(target);
        }
    }

    fn this_server(&self) -> Option<String> {
        if !offered_at(std::path::Path::new(LOCAL_SOCKET)) {
            return None;
        }
        local_identity()
            .ok()
            .map(|identity| this_server_target(&identity.user))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedIn {
    pub target: String,
    pub identity: Identity,
}

/// what the box asks the desktop to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    SignedIn(SignedIn),
    ShutDown,
}

/// a signed-in server and who you are on it.
/// what a click or a keystroke in the box means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    /// the recent server at this index.
    Recent(usize),
    /// where to sign in: 0 is this server, 1 another one over ssh.
    Where(usize),
    User(String),
    Host(String),
    Password(Zeroizing<String>),
    /// Enter in a box, with the box's final text.
    SubmitUser(String),
    SubmitHost(String),
    SubmitPassword(Zeroizing<String>),
    ShutDown,
    Cancel,
    Ok,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// user name and host name.
    Credentials,
    /// keys did not work and the server takes a password.
    Password,
}

/// what a background attempt or fingerprint lookup found.
enum Work {
    Attempt {
        target: String,
        result: Result<Identity, SigninError>,
    },
    Fingerprint {
        target: String,
        result: Result<String, SigninError>,
    },
}

/// a message over the box and what its buttons mean.
enum Overlay {
    Error(MessageBox),
    /// the server's key is new: trust it (button 0) or not.
    Trust(MessageBox, String),
}

/// where logons go: ssh to a chosen server, or the web ui the page came from.
#[derive(Clone)]
enum Backend {
    Ssh(Arc<dyn Auth>),
    Web(Rc<dyn WebSession>),
}

pub struct LoginBox {
    backend: Backend,
    wake: Wake,
    pub user: String,
    pub host: String,
    password: Zeroizing<String>,
    stage: Stage,
    work: Option<Receiver<Work>>,
    overlay: Option<Overlay>,
    status: String,
    status_is_error: bool,
    focus: bool,
    /// `user@this-server` when the operations socket is here, and whether it is the choice.
    here: Option<String>,
    local: bool,
}

impl LoginBox {
    /// a box for `auth`, with the fields filled from `last` (`user@host`) when there is one.
    pub fn new(auth: Arc<dyn Auth>, wake: Wake, last: Option<&str>) -> Self {
        let (user, host) = last
            .and_then(|l| l.split_once('@'))
            .map_or((String::new(), String::new()), |(u, h)| {
                (u.to_string(), h.to_string())
            });
        let here = auth.this_server();
        Self {
            local: here.is_some(),
            here,
            backend: Backend::Ssh(auth),
            wake,
            user,
            host,
            password: Zeroizing::new(String::new()),
            stage: Stage::Credentials,
            work: None,
            overlay: None,
            status: String::new(),
            status_is_error: false,
            focus: false,
        }
    }

    /// the browser's box: a user name and password for the gateway the page came from, no
    /// host. `last` is the user name to fill in.
    pub fn web(session: Rc<dyn WebSession>, wake: Wake, last: Option<&str>) -> Self {
        let mut login = Self::new(Arc::new(NoSsh), wake, None);
        login.backend = Backend::Web(session);
        login.here = None;
        login.local = false;
        login.user = last.unwrap_or_default().to_string();
        login.stage = Stage::Password;
        login
    }

    /// whether the choice is this server, signed in as the account running this program.
    pub fn is_local(&self) -> bool {
        self.local && self.here.is_some()
    }

    fn is_web(&self) -> bool {
        matches!(self.backend, Backend::Web(_))
    }

    /// in the browser: look for a session that is still good, and skip the box when there is one.
    pub fn resume_web(&mut self) {
        let Backend::Web(session) = self.backend.clone() else {
            return;
        };
        let (tx, rx) = channel();
        let wake = self.wake.clone();
        spawn_local(async move {
            let result = match session.current().await {
                Ok(Some(identity)) => Ok(identity),
                Ok(None) | Err(_) => Err(SigninError::NeedsPassword),
            };
            let _ = tx.send(Work::Attempt {
                target: String::new(),
                result,
            });
            wake.wake();
        });
        self.work = Some(rx);
        self.say("Checking for a session...", false);
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    pub fn busy(&self) -> bool {
        self.work.is_some()
    }

    /// the text of the message over the box, if one is open.
    pub fn overlay_text(&self) -> Option<&str> {
        match &self.overlay {
            Some(Overlay::Error(m) | Overlay::Trust(m, _)) => Some(m.text()),
            None => None,
        }
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn set_password(&mut self, text: &str) {
        self.password = Zeroizing::new(text.to_string());
    }

    pub fn password_is_empty(&self) -> bool {
        self.password.is_empty()
    }

    /// `user@host` from the two fields, or what is missing. a host typed as `user@host` with
    /// no user name filled in is accepted as it is.
    pub fn target(&self) -> Result<String, String> {
        if self.is_web() {
            return match self.user.trim() {
                "" => Err("Type a username.".into()),
                user => Ok(user.to_string()),
            };
        }
        if let Some(target) = self.here.as_ref().filter(|_| self.local) {
            return Ok(target.clone());
        }
        let (user, host) = (self.user.trim(), self.host.trim());
        if user.is_empty() {
            return match host.split_once('@') {
                Some((u, h)) if !u.is_empty() && !h.is_empty() => Ok(host.to_string()),
                _ => Err("Type a username.".into()),
            };
        }
        if host.is_empty() {
            return Err("Type a hostname.".into());
        }
        Ok(format!("{user}@{host}"))
    }

    /// show `text` in the status bar, for example after an account was created.
    pub fn note(&mut self, text: &str) {
        self.say(text, false);
    }

    fn say(&mut self, text: &str, error: bool) {
        self.status = text.into();
        self.status_is_error = error;
    }

    fn fail(&mut self, text: &str) {
        self.overlay = Some(Overlay::Error(MessageBox::new(
            "logon-message",
            "Logon Message",
            text,
            &["OK"],
        )));
        self.say("", false);
    }

    /// check the fields and start signing in.
    pub fn submit(&mut self) {
        if self.busy() || self.overlay.is_some() {
            return;
        }
        let target = match self.target() {
            Ok(target) => target,
            Err(why) => {
                self.say(&why, true);
                return;
            }
        };
        if self.is_web() && self.password.is_empty() {
            self.say("Type a password.", true);
            return;
        }
        let password = (self.stage == Stage::Password && !self.is_local()).then(|| {
            // the field is emptied now; the attempt owns the only copy
            Zeroizing::new(std::mem::take(&mut *self.password))
        });
        self.start_attempt(target, password, false);
    }

    fn start_attempt(&mut self, target: String, password: Option<Zeroizing<String>>, trust: bool) {
        let wake = self.wake.clone();
        let (tx, rx) = channel();
        let auth = match &self.backend {
            Backend::Ssh(auth) => auth.clone(),
            Backend::Web(session) => {
                let session = session.clone();
                spawn_local(async move {
                    let password = password.unwrap_or_default();
                    let result = session.login(target.clone(), password).await;
                    let _ = tx.send(Work::Attempt { target, result });
                    wake.wake();
                });
                self.work = Some(rx);
                self.say("Logging on...", false);
                return;
            }
        };
        std::thread::spawn(move || {
            let result = auth.attempt(&target, password.as_ref().map(|p| p.as_str()), trust);
            // dropping the password wipes it
            drop(password);
            let _ = tx.send(Work::Attempt { target, result });
            wake.wake();
        });
        self.work = Some(rx);
        self.say("Logging on...", false);
    }

    fn start_fingerprint(&mut self, target: String) {
        let Backend::Ssh(auth) = self.backend.clone() else {
            return;
        };
        let wake = self.wake.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let result = auth.fingerprint(&target);
            let _ = tx.send(Work::Fingerprint { target, result });
            wake.wake();
        });
        self.work = Some(rx);
        self.say("Checking the server's identity...", false);
    }

    /// pick up what the background work found.
    pub fn poll(&mut self) -> Option<SignedIn> {
        let received = self.work.as_ref()?.try_recv();
        let work = match received {
            Ok(work) => work,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                self.work = None;
                self.fail("The logon was interrupted.");
                return None;
            }
        };
        self.work = None;
        self.say("", false);
        match work {
            Work::Attempt { target, result } => self.attempted(target, result),
            Work::Fingerprint { target, result } => {
                match result {
                    Ok(key) => {
                        let text = format!(
                            "The identity of {target} is not known yet.\n\n\
                             Its host key is:\n{key}\n\n\
                             Trust this server only if you recognise that key."
                        );
                        self.overlay = Some(Overlay::Trust(
                            MessageBox::new(
                                "logon-trust",
                                "Unknown Server",
                                &text,
                                &["Trust", "Cancel"],
                            ),
                            target,
                        ));
                    }
                    Err(error) => self.fail(&error.to_string()),
                }
                None
            }
        }
    }

    fn attempted(
        &mut self,
        target: String,
        result: Result<Identity, SigninError>,
    ) -> Option<SignedIn> {
        match result {
            Ok(identity) => {
                // the session answered with the account's name, whatever was typed
                let target = if self.is_web() {
                    identity.user.clone()
                } else {
                    target
                };
                return Some(SignedIn { target, identity });
            }
            // the browser has no session yet: just show the box
            Err(SigninError::NeedsPassword) if self.is_web() => {
                self.focus = false;
            }
            Err(error) if self.is_web() => {
                self.password = Zeroizing::new(String::new());
                self.focus = false;
                self.fail(&error.to_string());
            }
            Err(SigninError::NeedsPassword) => {
                self.stage = Stage::Password;
                self.focus = false;
                self.say("This server asks for a password.", false);
            }
            Err(SigninError::HostKeyUnknown) => self.start_fingerprint(target),
            Err(SigninError::WrongPassword) => {
                self.password = Zeroizing::new(String::new());
                self.focus = false;
                self.fail(&SigninError::WrongPassword.to_string());
            }
            Err(error) => {
                self.stage = Stage::Credentials;
                self.fail(&error.to_string());
            }
        }
        None
    }

    /// Cancel: back out of the password step, or empty the boxes.
    pub fn cancel(&mut self) {
        if self.busy() {
            return;
        }
        if self.is_web() {
            self.password = Zeroizing::new(String::new());
            self.say("", false);
            self.focus = false;
            return;
        }
        if self.stage == Stage::Password {
            self.stage = Stage::Credentials;
        } else {
            self.user.clear();
            self.host.clear();
        }
        self.password = Zeroizing::new(String::new());
        self.say("", false);
        self.focus = false;
    }

    /// put the cursor back in the first empty field on the next frame.
    pub fn refocus(&mut self) {
        self.focus = false;
    }

    /// the title of the box.
    pub fn title(&self) -> &'static str {
        "Logon Information"
    }

    /// the box's contents; `known` are the servers used before.
    pub fn view(&mut self, known: &[String]) -> Vec<Node<Msg>> {
        let busy = self.busy() || self.overlay.is_some();
        let first = !self.focus;
        self.focus = true;
        let web = self.is_web();
        let local = self.is_local();
        let password_stage = self.stage == Stage::Password;
        let mut nodes = vec![
            Node::Header {
                icon: "gateway",
                title: String::new(),
                lines: vec![if web {
                    "Type your name and password for this gateway.".into()
                } else if local {
                    "You are signed in to this server with the account you are using.".into()
                } else {
                    "Type the name you use on the gateway and its hostname or address.".into()
                }],
            },
            Node::Gap(8.0),
        ];
        if let Some(target) = &self.here {
            let user = target
                .split_once('@')
                .map_or(target.as_str(), |(user, _)| user);
            let options = vec![
                (0, format!("This server (signed in as {user})")),
                (1, "Another server (ssh)".to_string()),
            ];
            nodes.push(Node::Labelled {
                label: "Sign in to:".into(),
                body: vec![Node::Choice {
                    id: "logon-where",
                    current: options[usize::from(!local)].1.clone(),
                    width: 240.0,
                    options,
                    pick: Msg::Where,
                }],
            });
            nodes.push(Node::Gap(4.0));
        }
        if local {
            return self.finish_view(busy, nodes);
        }
        if !known.is_empty() && !web {
            nodes.push(Node::Labelled {
                label: "Server:".into(),
                body: vec![Node::Choice {
                    id: "logon-known",
                    current: "Recent servers".into(),
                    width: 240.0,
                    options: known.iter().cloned().enumerate().collect(),
                    pick: Msg::Recent,
                }],
            });
            nodes.push(Node::Gap(4.0));
        }
        let field = |label: &str,
                     value: &str,
                     focus: bool,
                     edit: fn(String) -> Msg,
                     enter: fn(String) -> Msg| Node::Labelled {
            label: label.into(),
            body: vec![Node::Input {
                value: value.into(),
                size: Size::Points(240.0),
                focus,
                hint: String::new(),
                edit: Box::new(edit),
                enter: (!busy).then(|| Box::new(enter) as Box<dyn Fn(String) -> Msg>),
            }],
        };
        let empty = self.user.is_empty();
        nodes.push(field(
            "Username:",
            &self.user,
            first && (web || !password_stage) && empty,
            Msg::User,
            Msg::SubmitUser,
        ));
        if !web {
            nodes.push(Node::Gap(4.0));
            nodes.push(field(
                "Hostname:",
                &self.host,
                first && !password_stage && !empty,
                Msg::Host,
                Msg::SubmitHost,
            ));
        }
        if password_stage {
            nodes.push(Node::Gap(4.0));
            nodes.push(Node::Labelled {
                label: "Password:".into(),
                body: vec![Node::Secret {
                    value: self.password.clone(),
                    size: Size::Points(240.0),
                    shown: false,
                    focus: first && (!web || !empty),
                    edit: Box::new(Msg::Password),
                    enter: (!busy).then(|| {
                        Box::new(Msg::SubmitPassword) as Box<dyn Fn(Zeroizing<String>) -> Msg>
                    }),
                }],
            });
        }
        self.finish_view(busy, nodes)
    }

    /// the buttons and the status line under the fields.
    fn finish_view(&self, busy: bool, mut nodes: Vec<Node<Msg>>) -> Vec<Node<Msg>> {
        let web = self.is_web();
        // the browser has nothing to shut down
        let mut buttons = vec![
            Button::new("Cancel", !busy, Msg::Cancel),
            Button::new("OK", !busy, Msg::Ok),
        ];
        if !web {
            buttons.insert(0, Button::new("Shut Down", true, Msg::ShutDown));
        }
        nodes.extend([
            Node::Gap(10.0),
            Node::Buttons(buttons),
            Node::Gap(6.0),
            Node::Status {
                text: self.status.clone(),
                error: self.status_is_error,
            },
        ]);
        nodes
    }

    /// act on a click or keystroke; `known` are the servers the recent list showed.
    pub fn update(&mut self, msg: Msg, known: &[String]) -> Option<Outcome> {
        match msg {
            Msg::Recent(i) => {
                if let Some(server) = known.get(i) {
                    self.choose(server);
                }
            }
            Msg::Where(index) => {
                self.local = index == 0 && self.here.is_some();
                self.stage = Stage::Credentials;
                self.password = Zeroizing::new(String::new());
                self.focus = false;
            }
            Msg::User(text) => self.user = text,
            Msg::Host(text) => self.host = text,
            Msg::Password(text) => self.password = text,
            Msg::SubmitUser(text) => {
                self.user = text;
                self.submit();
            }
            Msg::SubmitHost(text) => {
                self.host = text;
                self.submit();
            }
            Msg::SubmitPassword(text) => {
                self.password = text;
                self.submit();
            }
            Msg::ShutDown => return Some(Outcome::ShutDown),
            Msg::Cancel => self.cancel(),
            Msg::Ok => self.submit(),
        }
        None
    }

    /// the message over the box, while one is open.
    pub fn overlay(&self) -> Option<Dialog<usize>> {
        match &self.overlay {
            Some(Overlay::Error(m) | Overlay::Trust(m, _)) => Some(m.view()),
            None => None,
        }
    }

    /// the button pressed in the message: on an unknown server, the first one trusts it.
    pub fn answer_overlay(&mut self, button: usize) {
        if let Some(Overlay::Trust(_, target)) = self.overlay.take() {
            if button == 0 {
                self.start_attempt(target, None, true);
            }
        }
    }

    /// pick a recent server: it fills both boxes.
    pub fn choose(&mut self, server: &str) {
        self.local = false;
        if let Some((user, host)) = server.split_once('@') {
            self.user = user.to_string();
            self.host = host.to_string();
        }
        self.stage = Stage::Credentials;
        self.password = Zeroizing::new(String::new());
    }
}

/// the ssh side of a box that only logs on through the web ui: never asked.
struct NoSsh;

impl Auth for NoSsh {
    fn attempt(&self, _: &str, _: Option<&str>, _: bool) -> Result<Identity, SigninError> {
        Err(SigninError::Other("no ssh in the browser".into()))
    }
    fn fingerprint(&self, _: &str) -> Result<String, SigninError> {
        Err(SigninError::Other("no ssh in the browser".into()))
    }
    fn sign_out(&self, _: &str) {}
}

#[cfg(any(test, feature = "fake"))]
pub mod fake {
    //! a scripted `Auth` for headless tests.

    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::{Auth, Identity, LocalFuture, SigninError, WebSession, Zeroizing};

    #[derive(Default)]
    pub struct FakeAuth {
        pub attempts: Mutex<VecDeque<Result<Identity, SigninError>>>,
        pub fingerprints: Mutex<VecDeque<Result<String, SigninError>>>,
        /// every call, as `attempt target password-or-none trust`.
        pub calls: Mutex<Vec<String>>,
        /// the target for "this server", when the machine offers it.
        pub here: Mutex<Option<String>>,
    }

    impl FakeAuth {
        pub fn push(&self, result: Result<Identity, SigninError>) {
            self.attempts.lock().unwrap().push_back(result);
        }

        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    /// a scripted web session: what `current` finds, what the next logons answer, every call.
    #[derive(Default)]
    pub struct FakeWeb {
        pub current: Mutex<Option<Identity>>,
        pub logins: Mutex<VecDeque<Result<Identity, SigninError>>>,
        /// every call, as `login user password`, `current` or `logout`.
        pub calls: Mutex<Vec<String>>,
        /// the server keeps the session when asked to end it.
        pub keeps_session: Mutex<bool>,
        /// `current` cannot reach the server.
        pub unreachable: Mutex<bool>,
    }

    impl FakeWeb {
        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl WebSession for FakeWeb {
        fn login(
            &self,
            user: String,
            password: Zeroizing<String>,
        ) -> LocalFuture<Result<Identity, SigninError>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("login {user} {}", password.as_str()));
            let answer = self
                .logins
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err(SigninError::Other("no answer scripted".into())));
            Box::pin(async move { answer })
        }

        fn current(&self) -> LocalFuture<anyhow::Result<Option<Identity>>> {
            self.calls.lock().unwrap().push("current".into());
            let now = self.current.lock().unwrap().clone();
            let down = *self.unreachable.lock().unwrap();
            Box::pin(async move {
                if down {
                    anyhow::bail!("no answer from the server");
                }
                Ok(now)
            })
        }

        fn logout(&self) -> LocalFuture<()> {
            self.calls.lock().unwrap().push("logout".into());
            if !*self.keeps_session.lock().unwrap() {
                *self.current.lock().unwrap() = None;
            }
            Box::pin(async {})
        }
    }

    pub fn id(user: &str, uid: u32) -> Identity {
        Identity {
            user: user.into(),
            uid,
            admin: true,
            grants: None,
        }
    }

    impl Auth for FakeAuth {
        fn attempt(
            &self,
            target: &str,
            password: Option<&str>,
            trust_new_host: bool,
        ) -> Result<Identity, SigninError> {
            self.calls.lock().unwrap().push(format!(
                "attempt {target} {} {trust_new_host}",
                password.unwrap_or("-")
            ));
            self.attempts
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err(SigninError::Other("no answer scripted".into())))
        }

        fn fingerprint(&self, target: &str) -> Result<String, SigninError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("fingerprint {target}"));
            self.fingerprints
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err(SigninError::Other("no key scripted".into())))
        }

        fn sign_out(&self, target: &str) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("sign_out {target}"));
        }

        fn this_server(&self) -> Option<String> {
            self.here.lock().unwrap().clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{id, FakeAuth, FakeWeb};
    use super::*;

    fn web_box(web: &Rc<FakeWeb>) -> LoginBox {
        LoginBox::web(web.clone(), Wake::none(), None)
    }

    #[test]
    fn the_web_box_logs_on_with_a_user_and_a_password_and_no_host() {
        let web = Rc::new(FakeWeb::default());
        web.logins.lock().unwrap().push_back(Ok(id("ann", 1000)));
        let mut login = web_box(&web);
        login.user = "ann".into();
        login.set_password("s3cret");
        login.submit();
        let signed = login.poll().expect("signed in");
        assert_eq!(signed.target, "ann");
        assert_eq!(signed.identity.user, "ann");
        assert_eq!(web.calls(), ["login ann s3cret"]);
        assert!(login.password_is_empty(), "the field was emptied");
    }

    #[test]
    fn the_web_box_needs_a_user_and_a_password_before_asking() {
        let web = Rc::new(FakeWeb::default());
        let mut login = web_box(&web);
        login.submit();
        assert_eq!(login.status(), "Type a username.");
        login.user = "ann".into();
        login.submit();
        assert_eq!(login.status(), "Type a password.");
        assert!(web.calls().is_empty());
    }

    #[test]
    fn a_wrong_password_in_the_web_box_says_so_and_keeps_the_box() {
        let web = Rc::new(FakeWeb::default());
        web.logins
            .lock()
            .unwrap()
            .push_back(Err(SigninError::WrongPassword));
        let mut login = web_box(&web);
        login.user = "ann".into();
        login.set_password("nope");
        login.submit();
        assert!(login.poll().is_none());
        assert!(login.overlay_text().unwrap().contains("incorrect"));
        assert!(login.password_is_empty());
        assert_eq!(login.stage(), Stage::Password);
    }

    #[test]
    fn a_live_session_skips_the_web_box() {
        let web = Rc::new(FakeWeb::default());
        *web.current.lock().unwrap() = Some(id("ann", 1000));
        let mut login = web_box(&web);
        login.resume_web();
        assert_eq!(login.poll().expect("signed in").target, "ann");
        assert_eq!(web.calls(), ["current"]);
    }

    #[test]
    fn without_a_session_the_web_box_just_waits_quietly() {
        let web = Rc::new(FakeWeb::default());
        let mut login = web_box(&web);
        login.resume_web();
        assert!(login.poll().is_none());
        assert!(login.overlay_text().is_none());
        assert_eq!(login.status(), "");
        assert!(!login.busy());
    }

    #[test]
    fn the_web_box_has_no_host_field_or_shut_down() {
        let web = Rc::new(FakeWeb::default());
        let mut login = web_box(&web);
        let mut labels = Vec::new();
        let mut buttons = Vec::new();
        for node in login.view(&["x@y".into()]) {
            match node {
                Node::Labelled { label, .. } => labels.push(label),
                Node::Buttons(list) => buttons.extend(list.into_iter().map(|b| b.label)),
                _ => {}
            }
        }
        assert_eq!(labels, ["Username:", "Password:"]);
        assert_eq!(buttons, ["Cancel", "OK"]);
    }

    #[test]
    fn the_logon_answers_become_messages() {
        assert_eq!(login_status(200), Ok(()));
        assert_eq!(login_status(401), Err(SigninError::WrongPassword));
        for status in [403, 400, 503] {
            assert!(matches!(login_status(status), Err(SigninError::Other(_))));
        }
    }

    #[test]
    fn a_dropped_worker_is_reported() {
        let mut login = LoginBox::new(Arc::new(FakeAuth::default()), Wake::none(), None);
        let (tx, rx) = channel::<Work>();
        drop(tx);
        login.work = Some(rx);
        assert!(login.poll().is_none());
        assert!(login.overlay_text().unwrap().contains("interrupted"));
    }

    #[test]
    fn the_last_server_fills_the_boxes_and_a_recent_one_replaces_them() {
        let mut login = LoginBox::new(Arc::new(FakeAuth::default()), Wake::none(), Some("ann@gw"));
        assert_eq!((login.user.as_str(), login.host.as_str()), ("ann", "gw"));
        let known = ["bob@other".to_string()];
        assert_eq!(login.update(Msg::Recent(0), &known), None);
        assert_eq!(login.target().unwrap(), "bob@other");
        assert_eq!(login.update(Msg::ShutDown, &known), Some(Outcome::ShutDown));
    }

    fn choice_labels(login: &mut LoginBox) -> Vec<String> {
        login
            .view(&[])
            .iter()
            .filter_map(|node| match node {
                Node::Labelled { body, .. } => Some(body),
                _ => None,
            })
            .flatten()
            .filter_map(|node| match node {
                Node::Choice { options, .. } => Some(options.iter().map(|o| o.1.clone())),
                _ => None,
            })
            .flatten()
            .collect()
    }

    #[test]
    fn this_server_is_offered_only_when_the_machine_has_the_socket() {
        let auth = Arc::new(FakeAuth::default());
        let mut without = LoginBox::new(auth.clone(), Wake::none(), None);
        assert!(!without.is_local());
        assert!(choice_labels(&mut without).is_empty());
        *auth.here.lock().unwrap() = Some("ann@this-server.localhost".into());
        let mut with = LoginBox::new(auth, Wake::none(), None);
        assert!(with.is_local(), "the default when it is there");
        assert_eq!(
            choice_labels(&mut with),
            ["This server (signed in as ann)", "Another server (ssh)"]
        );
    }

    #[test]
    fn this_server_signs_in_without_a_host_or_a_password_and_ssh_stays_a_choice() {
        let auth = Arc::new(FakeAuth::default());
        *auth.here.lock().unwrap() = Some("ann@this-server.localhost".into());
        auth.push(Ok(id("ann", 1000)));
        let mut login = LoginBox::new(auth.clone(), Wake::none(), None);
        login.submit();
        // the attempt runs on its own thread
        let signed = (0..500)
            .find_map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(10));
                login.poll()
            })
            .expect("signed in");
        assert_eq!(signed.target, "ann@this-server.localhost");
        assert_eq!(auth.calls(), ["attempt ann@this-server.localhost - false"]);
        // choosing the other server brings the fields back
        login.update(Msg::Where(1), &[]);
        assert!(!login.is_local());
        assert!(login.target().is_err());
        login.user = "bob".into();
        login.host = "gw".into();
        assert_eq!(login.target().unwrap(), "bob@gw");
    }
}
