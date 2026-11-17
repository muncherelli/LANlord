//! the logon box: the first screen, before any gateway is reached. it asks for a user name and a
//! host name and signs you in with your own ssh credentials. nothing is stored here but the list
//! of servers; a password you type lives in memory for one attempt and is wiped after it.

use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

#[cfg(feature = "native")]
use lanlord_core::signin::{Attempt, Signin};
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
        self.0.sign_out(target);
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

pub struct LoginBox {
    auth: Arc<dyn Auth>,
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
}

impl LoginBox {
    /// a box for `auth`, with the fields filled from `last` (`user@host`) when there is one.
    pub fn new(auth: Arc<dyn Auth>, wake: Wake, last: Option<&str>) -> Self {
        let (user, host) = last
            .and_then(|l| l.split_once('@'))
            .map_or((String::new(), String::new()), |(u, h)| {
                (u.to_string(), h.to_string())
            });
        Self {
            auth,
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
        let password = (self.stage == Stage::Password).then(|| {
            // the field is emptied now; the attempt owns the only copy
            Zeroizing::new(std::mem::take(&mut *self.password))
        });
        self.start_attempt(target, password, false);
    }

    fn start_attempt(&mut self, target: String, password: Option<Zeroizing<String>>, trust: bool) {
        let (auth, wake) = (self.auth.clone(), self.wake.clone());
        let (tx, rx) = channel();
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
        let (auth, wake) = (self.auth.clone(), self.wake.clone());
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
            Ok(identity) => return Some(SignedIn { target, identity }),
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
        let password_stage = self.stage == Stage::Password;
        let mut nodes = vec![
            Node::Header {
                icon: "gateway",
                title: String::new(),
                lines: vec![
                    "Type the name you use on the gateway and its hostname or address.".into(),
                ],
            },
            Node::Gap(8.0),
        ];
        if !known.is_empty() {
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
            first && !password_stage && empty,
            Msg::User,
            Msg::SubmitUser,
        ));
        nodes.push(Node::Gap(4.0));
        nodes.push(field(
            "Hostname:",
            &self.host,
            first && !password_stage && !empty,
            Msg::Host,
            Msg::SubmitHost,
        ));
        if password_stage {
            nodes.push(Node::Gap(4.0));
            nodes.push(Node::Labelled {
                label: "Password:".into(),
                body: vec![Node::Secret {
                    value: self.password.clone(),
                    size: Size::Points(240.0),
                    shown: false,
                    focus: first,
                    edit: Box::new(Msg::Password),
                    enter: (!busy).then(|| {
                        Box::new(Msg::SubmitPassword) as Box<dyn Fn(Zeroizing<String>) -> Msg>
                    }),
                }],
            });
        }
        nodes.extend([
            Node::Gap(10.0),
            Node::Buttons(vec![
                Button::new("Shut Down", true, Msg::ShutDown),
                Button::new("Cancel", !busy, Msg::Cancel),
                Button::new("OK", !busy, Msg::Ok),
            ]),
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
        if let Some((user, host)) = server.split_once('@') {
            self.user = user.to_string();
            self.host = host.to_string();
        }
        self.stage = Stage::Credentials;
        self.password = Zeroizing::new(String::new());
    }
}

#[cfg(any(test, feature = "fake"))]
pub mod fake {
    //! a scripted `Auth` for headless tests.

    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    pub struct FakeAuth {
        pub attempts: Mutex<VecDeque<Result<Identity, SigninError>>>,
        pub fingerprints: Mutex<VecDeque<Result<String, SigninError>>>,
        /// every call, as `attempt target password-or-none trust`.
        pub calls: Mutex<Vec<String>>,
    }

    impl FakeAuth {
        pub fn push(&self, result: Result<Identity, SigninError>) {
            self.attempts.lock().unwrap().push_back(result);
        }

        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    pub fn id(user: &str, uid: u32) -> Identity {
        Identity {
            user: user.into(),
            uid,
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
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeAuth;
    use super::*;

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
}
