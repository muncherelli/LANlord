//! the screen root lands on after logging on: create the first administrator. root is allowed
//! to sign in, but day-to-day work belongs to a personal account that is in `admins` and has
//! sudo, preferably signed in with an ssh key.

use lanlord_core::signin;
use zeroize::Zeroizing;

use crate::gateway::NewAdmin;
use crate::message::MessageBox;
use crate::services::Services;
use crate::tasks::Job;
use crate::view::{Button, Dialog, Node};

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

/// what a click or a keystroke on the screen means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    Name(String),
    Password(String),
    Confirm(String),
    Key(String),
    /// the key at this index of the agent's keys.
    AgentKey(usize),
    TogglePasswordless,
    Skip,
    Create,
}

pub struct AdminSetup {
    pub name: String,
    password: Zeroizing<String>,
    confirm: Zeroizing<String>,
    pub key: String,
    passwordless: bool,
    warning: Option<MessageBox>,
    /// the keys your ssh agent offers, read once when the screen opens.
    agent_keys: Vec<String>,
    job: Option<(Job<()>, String)>,
    status: String,
    status_is_error: bool,
    focus: bool,
}

impl Default for AdminSetup {
    fn default() -> Self {
        Self::new()
    }
}

impl AdminSetup {
    /// the screen, with the keys your ssh agent offers.
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

    pub fn with_test_keys(keys: &[&str]) -> Self {
        Self::with_keys(keys.iter().map(|k| k.to_string()).collect())
    }

    pub fn set_passwords(&mut self, password: &str, confirm: &str) {
        self.password = Zeroizing::new(password.to_string());
        self.confirm = Zeroizing::new(confirm.to_string());
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn passwordless(&self) -> bool {
        self.passwordless
    }

    pub fn warning_open(&self) -> bool {
        self.warning.is_some()
    }

    pub fn working(&self) -> bool {
        self.job.is_some()
    }

    fn say(&mut self, text: &str, error: bool) {
        self.status = text.into();
        self.status_is_error = error;
    }

    /// check the form and start creating the account on `host`.
    pub fn create(&mut self, services: &Services, host: &str) {
        if self.job.is_some() {
            return;
        }
        let name = self.name.trim().to_string();
        if name.is_empty() {
            self.say("Type a username for the new administrator.", true);
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
    pub fn poll(&mut self) -> Option<Outcome> {
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

    /// the title of the screen's box.
    pub fn title(&self) -> &'static str {
        "Create Administrator Account"
    }

    /// the screen's contents.
    pub fn view(&mut self) -> Vec<Node<Msg>> {
        let busy = self.job.is_some() || self.warning.is_some();
        let first = !self.focus;
        self.focus = true;
        let secret = |label: &str, value: &str, edit: fn(String) -> Msg| Node::Labelled {
            label: label.into(),
            body: vec![Node::Input {
                value: value.into(),
                width: 260.0,
                masked: true,
                focus: false,
                edit,
                enter: None,
            }],
        };
        let mut key = vec![Node::TextArea {
            id: "admin-key",
            value: self.key.clone(),
            height: 48.0,
            hint: "ssh-ed25519 AAAA... paste your public key here".into(),
            edit: Msg::Key,
        }];
        if !self.agent_keys.is_empty() {
            key.push(Node::Gap(4.0));
            key.push(Node::Labelled {
                label: "From your agent:".into(),
                body: vec![Node::Choice {
                    id: "admin-agent-keys",
                    current: "choose a key".into(),
                    width: 300.0,
                    options: self
                        .agent_keys
                        .iter()
                        .enumerate()
                        .map(|(i, k)| (i, key_label(k)))
                        .collect(),
                    pick: Msg::AgentKey,
                }],
            });
        }
        vec![
            Node::Header {
                icon: "gateway",
                title: String::new(),
                lines: vec![
                    "You are logged on as root. That is allowed, but root should not be your \
                     everyday account. Create an administrator for yourself: a login that is \
                     in admins and can use sudo."
                        .into(),
                ],
            },
            Node::Gap(8.0),
            Node::Labelled {
                label: "Username:".into(),
                body: vec![Node::Input {
                    value: self.name.clone(),
                    width: 260.0,
                    masked: false,
                    focus: first,
                    edit: Msg::Name,
                    enter: None,
                }],
            },
            Node::Gap(4.0),
            secret("Password:", &self.password, Msg::Password),
            Node::Gap(4.0),
            secret("Confirm:", &self.confirm, Msg::Confirm),
            Node::Gap(4.0),
            Node::Group {
                title: "SSH public key (the preferred way to sign in)".into(),
                body: key,
            },
            Node::Gap(6.0),
            Node::Check {
                label: "Allow sudo without a password (dangerous)".into(),
                checked: self.passwordless,
                enabled: true,
                toggle: Msg::TogglePasswordless,
            },
            Node::Gap(10.0),
            Node::Buttons(vec![
                Button::new("Skip for now", !busy, Msg::Skip),
                Button::new("Create", !busy, Msg::Create),
            ]),
            Node::Gap(6.0),
            Node::Status {
                text: self.status.clone(),
                error: self.status_is_error,
            },
        ]
    }

    /// act on a click or keystroke; `Some` once the screen is over.
    pub fn update(&mut self, msg: Msg, services: &Services, host: &str) -> Option<Outcome> {
        match msg {
            Msg::Name(text) => self.name = text,
            Msg::Password(text) => self.password = Zeroizing::new(text),
            Msg::Confirm(text) => self.confirm = Zeroizing::new(text),
            Msg::Key(text) => self.key = text,
            Msg::AgentKey(i) => {
                if let Some(key) = self.agent_keys.get(i) {
                    self.key.clone_from(key);
                }
            }
            Msg::TogglePasswordless => {
                if self.passwordless {
                    self.passwordless = false;
                } else {
                    // not on until the warning is accepted
                    self.warning = Some(MessageBox::new(
                        "admin-passwordless",
                        "Passwordless sudo",
                        PASSWORDLESS_WARNING,
                        &["Allow it", "Keep the password"],
                    ));
                }
            }
            Msg::Skip => return Some(Outcome::Skipped),
            Msg::Create => self.create(services, host),
        }
        None
    }

    /// the warning before passwordless sudo, while it is open.
    pub fn warning(&self) -> Option<Dialog<usize>> {
        self.warning.as_ref().map(MessageBox::view)
    }

    /// the button pressed in the warning: the first one allows passwordless sudo.
    pub fn answer_warning(&mut self, button: usize) {
        self.warning = None;
        if button == 0 {
            self.passwordless = true;
        }
    }
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
    use crate::wake::Wake;

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

        let fake = Arc::new(crate::gateway::Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
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

        let fake = Arc::new(crate::gateway::Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
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
