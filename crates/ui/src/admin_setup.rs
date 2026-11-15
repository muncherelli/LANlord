//! the screen root lands on after logging on: create the first administrator. root is allowed
//! to sign in, but day-to-day work belongs to a personal account that is in `lanlord-admins`
//! and has sudo, preferably signed in with an ssh key.

use eframe::egui::{self, Align2, Area, Frame, Id, Margin, Order, Vec2};
use lanlord_core::signin;
use zeroize::Zeroizing;

use crate::controls;
use crate::gateway::NewAdmin;
use crate::icons;
use crate::login::Message;
use crate::services::Services;
use crate::tasks::Job;
use crate::theme::{self, GRAY, NAVY, WHITE};

/// what the screen decided.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// the administrator `name` was created; the next logon should use it.
    Created(String),
    /// carry on as root for now.
    Skipped,
}

/// the warning before passwordless sudo, which says how sudo's timeout normally protects you.
const PASSWORDLESS_WARNING: &str = "Passwordless sudo is dangerous.\n\n\
Anything that gets into this account becomes root on the gateway with no further check: a stolen \
key, a laptop left unlocked with your ssh agent open, or any program running as you.\n\n\
You do not need it. With a password, sudo asks once and then remembers it for about 15 minutes \
before it asks again, and LANlord asks once per task and never stores the password. That is \
enough for normal use, and the gateway locks itself again when the time runs out.\n\n\
Allow passwordless sudo anyway?";

pub struct AdminSetup {
    pub name: String,
    password: Zeroizing<String>,
    confirm: Zeroizing<String>,
    pub key: String,
    passwordless: bool,
    warning: Option<Message>,
    /// the keys your ssh agent offers, read once when the screen opens.
    agent_keys: Vec<String>,
    job: Option<(Job<()>, String)>,
    status: String,
    status_is_error: bool,
    focus: bool,
}

impl AdminSetup {
    pub fn new() -> Self {
        Self::with_keys(signin::agent_keys())
    }

    fn with_keys(agent_keys: Vec<String>) -> Self {
        Self {
            name: String::new(),
            password: Zeroizing::new(String::new()),
            confirm: Zeroizing::new(String::new()),
            key: String::new(),
            passwordless: false,
            warning: None,
            agent_keys,
            job: None,
            status: String::new(),
            status_is_error: false,
            focus: false,
        }
    }

    #[cfg(test)]
    pub fn with_test_keys(keys: &[&str]) -> Self {
        Self::with_keys(keys.iter().map(|k| k.to_string()).collect())
    }

    #[cfg(test)]
    pub fn set_passwords(&mut self, password: &str, confirm: &str) {
        self.password = Zeroizing::new(password.to_string());
        self.confirm = Zeroizing::new(confirm.to_string());
    }

    #[cfg(test)]
    pub fn status(&self) -> &str {
        &self.status
    }

    #[cfg(test)]
    pub fn passwordless(&self) -> bool {
        self.passwordless
    }

    #[cfg(test)]
    pub fn warning_open(&self) -> bool {
        self.warning.is_some()
    }

    #[cfg(test)]
    pub fn working(&self) -> bool {
        self.job.is_some()
    }

    fn say(&mut self, text: &str, error: bool) {
        self.status = text.into();
        self.status_is_error = error;
    }

    /// check the form and start creating the account on `host`.
    fn create(&mut self, services: &Services, host: &str) {
        if self.job.is_some() {
            return;
        }
        let name = self.name.trim().to_string();
        if name.is_empty() {
            self.say("Type a user name for the new administrator.", true);
            return;
        }
        if self.password.is_empty() {
            self.say("Type a password for the new account.", true);
            return;
        }
        if *self.password != *self.confirm {
            self.say("The two passwords are not the same.", true);
            return;
        }
        let key = self.key.trim();
        let new = NewAdmin {
            name: name.clone(),
            // the field is emptied now; the task owns the only copy
            password: Zeroizing::new(std::mem::take(&mut *self.password)),
            ssh_key: (!key.is_empty()).then(|| key.to_string()),
            passwordless_sudo: self.passwordless,
        };
        self.confirm = Zeroizing::new(String::new());
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| {
            gateway.create_admin(&target, &new, ask)
        });
        match job {
            Some(job) => {
                self.job = Some((job, name));
                self.say("Creating the account...", false);
            }
            None => self.say("The gateway is busy. Try again in a moment.", true),
        }
    }

    /// pick up the result of the task.
    fn poll(&mut self) -> Option<Outcome> {
        let (job, name) = self.job.as_ref()?;
        let result = job.poll()?;
        let name = name.clone();
        self.job = None;
        match result {
            Ok(()) => Some(Outcome::Created(name)),
            Err(error) => {
                // the password was spent; it has to be typed again
                self.say(&error, true);
                None
            }
        }
    }

    /// draw the screen; `Some` once it is over. `host` is the signed-in server.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        services: &Services,
        host: &str,
    ) -> Option<Outcome> {
        if let Some(done) = self.poll() {
            return Some(done);
        }
        let mut outcome = None;
        Area::new(Id::unique("admin-setup"))
            .order(Order::Middle)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                let frame = Frame::new().fill(GRAY).inner_margin(Margin::same(3));
                let shown = frame.show(ui, |ui| {
                    ui.set_width(500.0);
                    let (bar, _) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), 20.0),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect_filled(bar, 0.0, NAVY);
                    ui.painter().text(
                        bar.left_center() + Vec2::new(6.0, 0.0),
                        Align2::LEFT_CENTER,
                        "Create Administrator Account",
                        theme::font(),
                        WHITE,
                    );
                    ui.add_space(6.0);
                    Frame::new()
                        .inner_margin(Margin::symmetric(10, 4))
                        .show(ui, |ui| outcome = self.body(ui, services, host));
                });
                theme::bevel(ui.painter(), shown.response.rect, true);
            });
        self.warning_box(ctx);
        outcome
    }

    fn body(&mut self, ui: &mut egui::Ui, services: &Services, host: &str) -> Option<Outcome> {
        let busy = self.job.is_some() || self.warning.is_some();
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(40.0), egui::Sense::hover());
            icons::draw(
                ui.painter(),
                rect.min + Vec2::new(4.0, 4.0),
                icons::art("gateway"),
                2.0,
            );
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    "You are logged on as root. That is allowed, but root should not be your \
                     everyday account. Create an administrator for yourself: a login that is \
                     in lanlord-admins and can use sudo.",
                );
            });
        });
        ui.add_space(8.0);
        let first = !self.focus;
        controls::row(ui, "User name:", |ui| {
            let response = controls::edit(ui, &mut self.name, 260.0);
            if first {
                response.request_focus();
            }
        });
        ui.add_space(4.0);
        secret_box(ui, "Password:", &mut self.password);
        ui.add_space(4.0);
        secret_box(ui, "Confirm:", &mut self.confirm);
        ui.add_space(4.0);
        self.focus = true;
        controls::group(ui, "SSH public key (the preferred way to sign in)", |ui| {
            controls::field(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.key)
                        .desired_rows(3)
                        .desired_width(ui.available_width() - 8.0)
                        .font(egui::TextStyle::Monospace)
                        .hint_text("ssh-ed25519 AAAA... paste your public key here")
                        .frame(Frame::new())
                        .margin(Margin::ZERO),
                )
            });
            if !self.agent_keys.is_empty() {
                ui.add_space(4.0);
                controls::row(ui, "From your agent:", |ui| {
                    let keys = self.agent_keys.clone();
                    controls::dropdown(ui, "admin-agent-keys", "choose a key", 300.0, |ui| {
                        for key in &keys {
                            if ui.selectable_label(false, key_label(key)).clicked() {
                                self.key = key.clone();
                            }
                        }
                    });
                });
            }
        });
        ui.add_space(6.0);
        let before = self.passwordless;
        let mut ticked = self.passwordless;
        controls::check(ui, &mut ticked, "Allow sudo without a password (dangerous)");
        if ticked != before {
            if ticked {
                // not on until the warning is accepted
                self.warning = Some(Message::new(
                    "admin-passwordless",
                    "Passwordless sudo",
                    PASSWORDLESS_WARNING,
                    &["Allow it", "Keep the password"],
                ));
            } else {
                self.passwordless = false;
            }
        }
        ui.add_space(10.0);
        let mut outcome = None;
        controls::buttons(ui, |ui| {
            if theme::action(ui, "Skip for now", !busy).clicked() {
                outcome = Some(Outcome::Skipped);
            }
            if theme::action(ui, "Create", !busy).clicked() {
                self.create(services, host);
            }
        });
        ui.add_space(6.0);
        controls::status_bar(ui, &self.status, self.status_is_error);
        outcome
    }

    fn warning_box(&mut self, ctx: &egui::Context) {
        let Some(message) = self.warning.take() else {
            return;
        };
        match message.show(ctx) {
            None => self.warning = Some(message),
            Some(0) => self.passwordless = true,
            Some(_) => {}
        }
    }
}

/// a labelled masked box.
fn secret_box(ui: &mut egui::Ui, label: &str, text: &mut String) {
    controls::row(ui, label, |ui| {
        controls::field(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(text)
                    .password(true)
                    .desired_width(260.0)
                    .frame(Frame::new())
                    .margin(Margin::ZERO),
            )
        });
    });
}

/// a short name for a key line: its comment, or its type.
fn key_label(key: &str) -> String {
    let mut words = key.split_whitespace();
    let kind = words.next().unwrap_or("");
    match words.nth(1) {
        Some(comment) => format!("{comment} ({kind})"),
        None => kind.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_named_by_their_comment() {
        assert_eq!(
            key_label("ssh-ed25519 AAAA me@laptop"),
            "me@laptop (ssh-ed25519)"
        );
        assert_eq!(key_label("ssh-ed25519 AAAA"), "ssh-ed25519");
        assert_eq!(key_label(""), "");
    }

    #[test]
    fn a_busy_gateway_is_reported_and_nothing_is_sent() {
        use std::sync::{mpsc, Arc};

        let ctx = egui::Context::default();
        let fake = Arc::new(crate::gateway::Fake::new());
        let services = Services::new(fake.clone(), &ctx);
        // a task that does not finish holds the host
        let (release, wait) = mpsc::channel::<()>();
        let held = services.spawn("root@gw", move |_, _| {
            let _ = wait.recv();
            Ok(())
        });
        assert!(held.is_some());
        let mut setup = AdminSetup::with_test_keys(&[]);
        setup.name = "alice".into();
        setup.set_passwords("pw", "pw");
        setup.create(&services, "root@gw");
        assert!(setup.status().contains("busy"), "{}", setup.status());
        assert!(!setup.working());
        assert!(fake.calls().is_empty());
        drop(release);
    }

    #[test]
    fn a_form_in_progress_ignores_a_second_create() {
        use std::sync::Arc;

        let ctx = egui::Context::default();
        let fake = Arc::new(crate::gateway::Fake::new());
        let services = Services::new(fake.clone(), &ctx);
        let mut setup = AdminSetup::with_test_keys(&[]);
        setup.name = "alice".into();
        setup.set_passwords("pw", "pw");
        setup.create(&services, "root@gw");
        assert!(setup.working());
        // typed again and pressed again while the first is running: nothing new starts
        setup.set_passwords("pw", "pw");
        setup.create(&services, "root@gw");
        let start = std::time::Instant::now();
        while setup.poll().is_none() {
            assert!(
                start.elapsed() < std::time::Duration::from_secs(5),
                "timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(fake.calls().len(), 1);
    }

    #[test]
    fn the_warning_explains_the_sudo_timeout() {
        assert!(PASSWORDLESS_WARNING.contains("15 minutes"));
        assert!(PASSWORDLESS_WARNING.contains("dangerous"));
    }
}
