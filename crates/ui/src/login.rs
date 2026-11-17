//! the logon box: the first screen, before any gateway is reached. its contents and the sign-in
//! itself live in `lanlord_app::login`; this draws them in a box in the middle of the screen.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use eframe::egui::{self, Align2, Area, Frame, Id, Margin, Order, Vec2};

#[cfg(test)]
pub use lanlord_app::login::fake;
#[cfg(test)]
pub use lanlord_app::login::Stage;
#[cfg(feature = "native")]
pub use lanlord_app::login::SystemAuth;
pub use lanlord_app::login::{Auth, Outcome, SignedIn};

use crate::theme::{self, GRAY, NAVY, WHITE};
use crate::view;

pub struct LoginBox(lanlord_app::login::LoginBox);

impl Deref for LoginBox {
    type Target = lanlord_app::login::LoginBox;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for LoginBox {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl LoginBox {
    /// the browser's box: user name and password for the gateway the page came from.
    pub fn web(
        session: std::rc::Rc<dyn lanlord_app::login::WebSession>,
        ctx: &egui::Context,
        last: Option<&str>,
    ) -> Self {
        Self(lanlord_app::login::LoginBox::web(
            session,
            crate::wake(ctx),
            last,
        ))
    }

    /// a box for `auth`, with the fields filled from `last` (`user@host`) when there is one.
    pub fn new(auth: Arc<dyn Auth>, ctx: &egui::Context, last: Option<&str>) -> Self {
        Self(lanlord_app::login::LoginBox::new(
            auth,
            crate::wake(ctx),
            last,
        ))
    }

    /// draw the box over the desktop colour; `known` are the servers used before.
    pub fn show(&mut self, ctx: &egui::Context, known: &[String]) -> Option<Outcome> {
        if let Some(signed_in) = self.0.poll() {
            return Some(Outcome::SignedIn(signed_in));
        }
        let mut outcome = None;
        Area::new(Id::unique("logon"))
            .order(Order::Middle)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                let frame = Frame::new().fill(GRAY).inner_margin(Margin::same(3));
                let shown = frame.show(ui, |ui| {
                    ui.set_width(420.0);
                    title_bar(ui, self.0.title());
                    ui.add_space(6.0);
                    Frame::new()
                        .inner_margin(Margin::symmetric(10, 4))
                        .show(ui, |ui| {
                            let nodes = self.0.view(known);
                            if let Some(msg) = view::show(ui, &nodes) {
                                outcome = self.0.update(msg, known);
                            }
                        });
                });
                theme::bevel(ui.painter(), shown.response.rect, true);
            });
        if let Some(dialog) = self.0.overlay() {
            if let Some(button) = view::dialog(ctx, &dialog) {
                self.0.answer_overlay(button);
            }
        }
        outcome
    }
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
mod tests {
    use std::time::{Duration, Instant};

    use super::fake::{id, FakeAuth};
    use super::*;
    use crate::testkit;
    use lanlord_core::signin::SigninError;

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
            (rig(None).login.user.clone(), rig(None).login.host.clone()),
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
        rig.login.refocus();
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
}
