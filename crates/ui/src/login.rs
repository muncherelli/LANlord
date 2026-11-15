//! the logon box: the first screen, before any gateway is reached. it asks for a user name and a
//! host name and signs you in with your own ssh credentials. nothing is stored here but the list
//! of servers; a password you type lives in memory for one attempt and is wiped after it.

use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

use eframe::egui::{self, Align2, Area, Color32, Frame, Id, Key, Margin, Order, Sense, Vec2};
use lanlord_core::signin::{Attempt, Identity, Signin, SigninError};
use zeroize::Zeroizing;

use crate::controls;
use crate::icons;
use crate::theme::{self, BLACK, GRAY, NAVY, WHITE};

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

pub struct SystemAuth(Signin);

impl SystemAuth {
    pub fn new() -> Self {
        Self(Signin::system())
    }
}

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

/// a signed-in server and who you are on it.
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

/// a box with a message and a row of buttons; the last button is the way out.
pub struct Message {
    id: &'static str,
    title: String,
    text: String,
    buttons: Vec<String>,
}

impl Message {
    pub fn new(id: &'static str, title: &str, text: &str, buttons: &[&str]) -> Self {
        Self {
            id,
            title: title.into(),
            text: text.into(),
            buttons: buttons.iter().map(ToString::to_string).collect(),
        }
    }

    /// draw the box; `Some(n)` once button `n` was pressed (Escape presses the last one).
    pub fn show(&self, ctx: &egui::Context) -> Option<usize> {
        let mut pressed = None;
        crate::dialogs::modal(ctx, self.id, &self.title, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
                warning(ui, rect);
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width());
                    ui.label(&self.text);
                });
            });
            ui.add_space(8.0);
            controls::buttons(ui, |ui| {
                // right to left, so the first button ends up leftmost
                for (n, label) in self.buttons.iter().enumerate().rev() {
                    if theme::button(ui, label, false).clicked() {
                        pressed = Some(n);
                    }
                }
                if ui.input(|i| i.key_pressed(Key::Escape)) {
                    pressed = Some(self.buttons.len() - 1);
                }
            });
        });
        pressed
    }
}

/// the yellow warning triangle.
fn warning(ui: &egui::Ui, rect: egui::Rect) {
    let c = rect.center();
    let points = vec![
        c + Vec2::new(0.0, -14.0),
        c + Vec2::new(15.0, 13.0),
        c + Vec2::new(-15.0, 13.0),
    ];
    let yellow = Color32::from_rgb(255, 255, 0);
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        yellow,
        egui::Stroke::new(1.5, BLACK),
    ));
    ui.painter().text(
        c + Vec2::new(0.0, 4.0),
        Align2::CENTER_CENTER,
        "!",
        egui::FontId::proportional(16.0),
        BLACK,
    );
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
    Error(Message),
    /// the server's key is new: trust it (button 0) or not.
    Trust(Message, String),
}

pub struct LoginBox {
    auth: Arc<dyn Auth>,
    ctx: egui::Context,
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
    pub fn new(auth: Arc<dyn Auth>, ctx: &egui::Context, last: Option<&str>) -> Self {
        let (user, host) = last
            .and_then(|l| l.split_once('@'))
            .map_or((String::new(), String::new()), |(u, h)| {
                (u.to_string(), h.to_string())
            });
        Self {
            auth,
            ctx: ctx.clone(),
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

    #[cfg(test)]
    pub fn stage(&self) -> Stage {
        self.stage
    }

    pub fn busy(&self) -> bool {
        self.work.is_some()
    }

    /// the text of the message over the box, if one is open.
    #[cfg(test)]
    pub fn overlay_text(&self) -> Option<&str> {
        match &self.overlay {
            Some(Overlay::Error(m) | Overlay::Trust(m, _)) => Some(&m.text),
            None => None,
        }
    }

    #[cfg(test)]
    pub fn status(&self) -> &str {
        &self.status
    }

    #[cfg(test)]
    pub fn set_password(&mut self, text: &str) {
        self.password = Zeroizing::new(text.to_string());
    }

    #[cfg(test)]
    pub fn password_is_empty(&self) -> bool {
        self.password.is_empty()
    }

    /// `user@host` from the two fields, or what is missing. a host typed as `user@host` with
    /// no user name filled in is accepted as it is.
    fn target(&self) -> Result<String, String> {
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
        self.overlay = Some(Overlay::Error(Message::new(
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
        let (auth, ctx) = (self.auth.clone(), self.ctx.clone());
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let result = auth.attempt(&target, password.as_ref().map(|p| p.as_str()), trust);
            // dropping the password wipes it
            drop(password);
            let _ = tx.send(Work::Attempt { target, result });
            ctx.request_repaint();
        });
        self.work = Some(rx);
        self.say("Logging on...", false);
    }

    fn start_fingerprint(&mut self, target: String) {
        let (auth, ctx) = (self.auth.clone(), self.ctx.clone());
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let result = auth.fingerprint(&target);
            let _ = tx.send(Work::Fingerprint { target, result });
            ctx.request_repaint();
        });
        self.work = Some(rx);
        self.say("Checking the server's identity...", false);
    }

    /// pick up what the background work found.
    fn poll(&mut self) -> Option<SignedIn> {
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
                            Message::new(
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
    fn cancel(&mut self) {
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

    /// draw the box over the desktop colour; `known` are the servers used before.
    pub fn show(&mut self, ctx: &egui::Context, known: &[String]) -> Option<Outcome> {
        if let Some(signed_in) = self.poll() {
            return Some(Outcome::SignedIn(signed_in));
        }
        let mut shut_down = false;
        Area::new(Id::unique("logon"))
            .order(Order::Middle)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                let frame = Frame::new().fill(GRAY).inner_margin(Margin::same(3));
                let shown = frame.show(ui, |ui| {
                    ui.set_width(420.0);
                    title_bar(ui, "Logon Information");
                    ui.add_space(6.0);
                    Frame::new()
                        .inner_margin(Margin::symmetric(10, 4))
                        .show(ui, |ui| shut_down = self.body(ui, known));
                });
                theme::bevel(ui.painter(), shown.response.rect, true);
            });
        self.overlay_buttons(ctx);
        shut_down.then_some(Outcome::ShutDown)
    }

    fn body(&mut self, ui: &mut egui::Ui, known: &[String]) -> bool {
        let busy = self.busy() || self.overlay.is_some();
        intro(ui);
        let first = !self.focus;
        self.known_row(ui, known);
        let mut enter = self.name_rows(ui, first);
        if self.stage == Stage::Password {
            ui.add_space(4.0);
            enter |= self.secret_row(ui, first);
        }
        self.focus = true;
        ui.add_space(10.0);
        let shut_down = self.button_row(ui, busy, enter);
        ui.add_space(6.0);
        controls::status_bar(ui, &self.status, self.status_is_error);
        shut_down
    }

    /// the drop-down of servers used before; choosing one fills both boxes.
    fn known_row(&mut self, ui: &mut egui::Ui, known: &[String]) {
        if known.is_empty() {
            return;
        }
        controls::row(ui, "Server:", |ui| {
            controls::dropdown(ui, "logon-known", "Recent servers", 240.0, |ui| {
                for server in known {
                    if ui.selectable_label(false, server).clicked() {
                        self.choose(server);
                    }
                }
            });
        });
        ui.add_space(4.0);
    }

    /// the user name and host name boxes; true when enter was pressed in one of them.
    fn name_rows(&mut self, ui: &mut egui::Ui, first: bool) -> bool {
        let password_stage = self.stage == Stage::Password;
        let mut enter = false;
        controls::row(ui, "Username:", |ui| {
            let response = controls::edit(ui, &mut self.user, 240.0);
            if first && !password_stage && self.user.is_empty() {
                response.request_focus();
            }
            enter |= pressed_enter(ui, &response);
        });
        ui.add_space(4.0);
        controls::row(ui, "Hostname:", |ui| {
            let response = controls::edit(ui, &mut self.host, 240.0);
            if first && !password_stage && !self.user.is_empty() {
                response.request_focus();
            }
            enter |= pressed_enter(ui, &response);
        });
        enter
    }

    /// the masked box for the one attempt that needs it; true when enter was pressed in it.
    fn secret_row(&mut self, ui: &mut egui::Ui, first: bool) -> bool {
        let mut enter = false;
        controls::row(ui, "Password:", |ui| {
            let response = controls::field(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut *self.password)
                        .password(true)
                        .desired_width(240.0)
                        .frame(Frame::new())
                        .margin(Margin::ZERO),
                )
            });
            if first {
                response.request_focus();
            }
            enter = pressed_enter(ui, &response);
        });
        enter
    }

    /// OK, Cancel and Shut Down; true when Shut Down was pressed.
    fn button_row(&mut self, ui: &mut egui::Ui, busy: bool, enter: bool) -> bool {
        let mut shut_down = false;
        controls::buttons(ui, |ui| {
            if theme::button(ui, "Shut Down", false).clicked() {
                shut_down = true;
            }
            if theme::action(ui, "Cancel", !busy).clicked() {
                self.cancel();
            }
            if theme::action(ui, "OK", !busy).clicked() || (enter && !busy) {
                self.submit();
            }
        });
        shut_down
    }

    /// pick a recent server: it fills both boxes.
    fn choose(&mut self, server: &str) {
        if let Some((user, host)) = server.split_once('@') {
            self.user = user.to_string();
            self.host = host.to_string();
        }
        self.stage = Stage::Credentials;
        self.password = Zeroizing::new(String::new());
    }

    fn overlay_buttons(&mut self, ctx: &egui::Context) {
        let Some(overlay) = self.overlay.take() else {
            return;
        };
        match overlay {
            Overlay::Error(message) => {
                if message.show(ctx).is_none() {
                    self.overlay = Some(Overlay::Error(message));
                }
            }
            Overlay::Trust(message, target) => match message.show(ctx) {
                None => self.overlay = Some(Overlay::Trust(message, target)),
                Some(0) => self.start_attempt(target, None, true),
                Some(_) => {}
            },
        }
    }
}

/// the picture and the sentence at the top of the box.
fn intro(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
        icons::draw(
            ui.painter(),
            rect.min + Vec2::new(4.0, 4.0),
            icons::art("gateway"),
            2.0,
        );
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.set_width(ui.available_width());
            ui.label("Type the name you use on the gateway and its hostname or address.");
        });
    });
    ui.add_space(8.0);
}

/// enter was pressed while this box had the focus.
fn pressed_enter(ui: &egui::Ui, response: &egui::Response) -> bool {
    response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))
}

/// the navy strip with the box's name.
fn title_bar(ui: &mut egui::Ui, title: &str) {
    let (bar, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), egui::Sense::hover());
    ui.painter().rect_filled(bar, 0.0, NAVY);
    ui.painter().text(
        bar.left_center() + Vec2::new(6.0, 0.0),
        Align2::LEFT_CENTER,
        title,
        theme::font(),
        WHITE,
    );
}

#[cfg(test)]
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
    use std::time::{Duration, Instant};

    use super::fake::{id, FakeAuth};
    use super::*;
    use crate::testkit;

    struct Rig {
        ctx: egui::Context,
        login: LoginBox,
        auth: Arc<FakeAuth>,
        outcome: Option<Outcome>,
    }

    fn rig(last: Option<&str>) -> Rig {
        let ctx = egui::Context::default();
        let auth = Arc::new(FakeAuth::default());
        let login = LoginBox::new(auth.clone(), &ctx, last);
        Rig {
            ctx,
            login,
            auth,
            outcome: None,
        }
    }

    impl Rig {
        fn frame(&mut self, events: Vec<egui::Event>, known: &[String]) {
            let (login, outcome) = (&mut self.login, &mut self.outcome);
            testkit::run(&self.ctx, events, |ui| {
                theme::apply(ui.ctx());
                if let Some(done) = login.show(ui.ctx(), known) {
                    *outcome = Some(done);
                }
            });
        }

        fn frames(&mut self, n: usize) {
            for _ in 0..n {
                self.frame(Vec::new(), &[]);
            }
        }

        fn press(&mut self, label: &str) -> bool {
            self.frames(3);
            let Some((rect, true)) = theme::probe::find(&self.ctx, label) else {
                return false;
            };
            for event in testkit::click(rect.center()) {
                self.frame(vec![event], &[]);
            }
            self.frames(2);
            true
        }

        /// draw frames until nothing is running in the background.
        fn settle(&mut self) {
            let start = Instant::now();
            while self.login.busy() {
                assert!(start.elapsed() < Duration::from_secs(5), "timed out");
                self.frames(1);
                std::thread::sleep(Duration::from_millis(5));
            }
            self.frames(2);
        }

        fn fill(&mut self, user: &str, host: &str) {
            self.login.user = user.into();
            self.login.host = host.into();
        }
    }

    #[test]
    fn the_box_starts_empty_or_with_the_last_server() {
        assert_eq!(
            (rig(None).login.user, rig(None).login.host),
            (String::new(), String::new())
        );
        let last = rig(Some("alice@gw.lan")).login;
        assert_eq!(
            (last.user.as_str(), last.host.as_str()),
            ("alice", "gw.lan")
        );
        let odd = rig(Some("nonsense")).login;
        assert!(odd.user.is_empty() && odd.host.is_empty());
    }

    #[test]
    fn a_missing_name_is_asked_for_without_connecting() {
        let mut rig = rig(None);
        rig.frames(2);
        assert!(rig.press("OK"));
        assert_eq!(rig.login.status(), "Type a username.");
        rig.fill("alice", "");
        assert!(rig.press("OK"));
        assert_eq!(rig.login.status(), "Type a hostname.");
        assert!(rig.auth.calls().is_empty());
        rig.fill("", "bob@gw");
        assert_eq!(rig.login.target().unwrap(), "bob@gw");
        rig.fill("", "@gw");
        assert!(rig.login.target().is_err());
    }

    #[test]
    fn a_key_that_works_signs_you_in_without_a_password() {
        let mut rig = rig(None);
        rig.auth.push(Ok(id("alice", 1000)));
        rig.fill(" alice ", " gw.lan ");
        assert!(rig.press("OK"));
        rig.settle();
        assert_eq!(rig.auth.calls(), ["attempt alice@gw.lan - false"]);
        assert_eq!(
            rig.outcome,
            Some(Outcome::SignedIn(SignedIn {
                target: "alice@gw.lan".into(),
                identity: id("alice", 1000)
            }))
        );
    }

    #[test]
    fn enter_in_a_box_logs_on() {
        let mut rig = rig(None);
        rig.auth.push(Ok(id("alice", 1000)));
        rig.fill("alice", "gw");
        rig.frames(3);
        // focus the host box by clicking it, then press enter
        rig.login.focus = false;
        rig.frames(3);
        rig.frame(vec![testkit::key(egui::Key::Enter)], &[]);
        rig.settle();
        assert_eq!(rig.auth.calls().len(), 1);
    }

    #[test]
    fn a_server_with_passwords_asks_for_one_and_wipes_it() {
        let mut rig = rig(None);
        rig.auth.push(Err(SigninError::NeedsPassword));
        rig.auth.push(Ok(id("alice", 1000)));
        rig.fill("alice", "gw");
        assert!(rig.press("OK"));
        rig.settle();
        assert_eq!(rig.login.stage(), Stage::Password);
        assert_eq!(rig.login.status(), "This server asks for a password.");
        rig.login.set_password("hunter2");
        assert!(rig.press("OK"));
        assert!(rig.login.password_is_empty(), "the box is emptied at once");
        rig.settle();
        assert_eq!(
            rig.auth.calls(),
            ["attempt alice@gw - false", "attempt alice@gw hunter2 false"]
        );
        assert!(matches!(rig.outcome, Some(Outcome::SignedIn(_))));
    }

    #[test]
    fn a_wrong_password_says_so_and_stays_on_the_password_step() {
        let mut rig = rig(None);
        rig.auth.push(Err(SigninError::NeedsPassword));
        rig.auth.push(Err(SigninError::WrongPassword));
        rig.fill("alice", "gw");
        assert!(rig.press("OK"));
        rig.settle();
        rig.login.set_password("bad");
        assert!(rig.press("OK"));
        rig.settle();
        assert_eq!(rig.login.stage(), Stage::Password);
        assert!(rig.login.overlay_text().unwrap().contains("incorrect"));
        assert!(rig.login.password_is_empty());
        assert!(rig.press("OK"), "dismiss the message");
        assert!(rig.login.overlay_text().is_none());
    }

    #[test]
    fn a_keys_only_server_is_an_error_message() {
        let mut rig = rig(None);
        rig.auth
            .push(Err(SigninError::KeysOnly(vec!["publickey".into()])));
        rig.fill("alice", "gw");
        assert!(rig.press("OK"));
        rig.settle();
        assert_eq!(rig.login.stage(), Stage::Credentials);
        assert!(rig
            .login
            .overlay_text()
            .unwrap()
            .contains("only accepts keys"));
        assert!(rig.press("OK"));
        assert!(rig.outcome.is_none());
    }

    #[test]
    fn other_failures_show_their_own_message_and_keep_you_on_the_box() {
        for (error, word) in [
            (SigninError::Timeout, "did not answer"),
            (SigninError::Refused, "refused"),
            (SigninError::HostKeyChanged, "changed"),
            (SigninError::AgentLocked, "agent"),
            (
                SigninError::Unreachable("That hostname could not be found.".into()),
                "found",
            ),
        ] {
            let mut rig = rig(None);
            rig.auth.push(Err(error));
            rig.fill("alice", "gw");
            assert!(rig.press("OK"));
            rig.settle();
            assert!(rig.login.overlay_text().unwrap().contains(word), "{word}");
            assert!(rig.outcome.is_none());
        }
    }

    #[test]
    fn an_unknown_host_shows_its_key_and_is_only_trusted_on_request() {
        let mut rig = rig(None);
        rig.auth.push(Err(SigninError::HostKeyUnknown));
        rig.auth.push(Ok(id("alice", 1000)));
        rig.auth
            .fingerprints
            .lock()
            .unwrap()
            .push_back(Ok("ssh-ed25519 SHA256:AbCd".into()));
        rig.fill("alice", "gw");
        assert!(rig.press("OK"));
        rig.settle();
        assert!(rig
            .login
            .overlay_text()
            .unwrap()
            .contains("ssh-ed25519 SHA256:AbCd"));
        assert_eq!(
            rig.auth.calls(),
            ["attempt alice@gw - false", "fingerprint alice@gw"]
        );
        assert!(rig.press("Trust"));
        rig.settle();
        assert_eq!(rig.auth.calls().last().unwrap(), "attempt alice@gw - true");
        assert!(matches!(rig.outcome, Some(Outcome::SignedIn(_))));
    }

    #[test]
    fn declining_an_unknown_host_connects_nowhere() {
        let mut rig = rig(None);
        rig.auth.push(Err(SigninError::HostKeyUnknown));
        rig.auth
            .fingerprints
            .lock()
            .unwrap()
            .push_back(Ok("ssh-ed25519 SHA256:AbCd".into()));
        rig.fill("alice", "gw");
        assert!(rig.press("OK"));
        rig.settle();
        assert!(rig.press("Cancel"));
        rig.settle();
        assert_eq!(rig.auth.calls().len(), 2, "no trusted attempt");
        assert!(rig.login.overlay_text().is_none() && rig.outcome.is_none());
    }

    #[test]
    fn a_fingerprint_that_cannot_be_read_is_an_error() {
        let mut rig = rig(None);
        rig.auth.push(Err(SigninError::HostKeyUnknown));
        rig.auth
            .fingerprints
            .lock()
            .unwrap()
            .push_back(Err(SigninError::Refused));
        rig.fill("alice", "gw");
        assert!(rig.press("OK"));
        rig.settle();
        assert!(rig.login.overlay_text().unwrap().contains("refused"));
    }

    #[test]
    fn cancel_backs_out_of_the_password_step_and_then_empties_the_boxes() {
        let mut rig = rig(None);
        rig.auth.push(Err(SigninError::NeedsPassword));
        rig.fill("alice", "gw");
        assert!(rig.press("OK"));
        rig.settle();
        rig.login.set_password("half typed");
        assert!(rig.press("Cancel"));
        assert_eq!(rig.login.stage(), Stage::Credentials);
        assert!(rig.login.password_is_empty());
        assert_eq!(rig.login.user, "alice");
        assert!(rig.press("Cancel"));
        assert!(rig.login.user.is_empty() && rig.login.host.is_empty());
    }

    #[test]
    fn shut_down_asks_the_desktop_to_quit() {
        let mut rig = rig(None);
        assert!(rig.press("Shut Down"));
        assert_eq!(rig.outcome, Some(Outcome::ShutDown));
    }

    #[test]
    fn a_recent_server_fills_both_boxes() {
        let mut rig = rig(None);
        rig.login.choose("bob@other.lan");
        assert_eq!(
            (rig.login.user.as_str(), rig.login.host.as_str()),
            ("bob", "other.lan")
        );
        rig.login.choose("no-at-sign");
        let known = vec!["bob@other.lan".to_string()];
        rig.frame(Vec::new(), &known);
        rig.frame(Vec::new(), &known);
    }

    #[test]
    fn nothing_is_submitted_while_busy_or_over_a_message() {
        let mut rig = rig(None);
        rig.auth.push(Err(SigninError::Refused));
        rig.fill("alice", "gw");
        assert!(rig.press("OK"));
        rig.settle();
        assert!(rig.login.overlay_text().is_some());
        rig.login.submit();
        assert_eq!(
            rig.auth.calls().len(),
            1,
            "the message must be dismissed first"
        );
        rig.login.cancel();
        assert!(!rig.login.busy());
    }

    #[test]
    fn a_dropped_worker_is_reported() {
        let mut rig = rig(None);
        let (tx, rx) = channel::<Work>();
        drop(tx);
        rig.login.work = Some(rx);
        rig.frames(2);
        assert!(rig.login.overlay_text().unwrap().contains("interrupted"));
    }
}
