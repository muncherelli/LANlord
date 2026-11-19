//! the screen root lands on after logging on: create the first administrator. root is allowed
//! to sign in, but day-to-day work belongs to a personal account that is in `admins` and has
//! sudo, preferably signed in with an ssh key.

use lanlord_model::operations::Operation;
use lanlord_model::rootssh::RootSsh;
use lanlord_model::secret::Secret;
use zeroize::Zeroizing;

use crate::gateway::NewAdmin;
use crate::message::MessageBox;
use crate::services::Services;
use crate::tasks::Job;
use crate::view::{Button, Dialog, Node, Size};

/// what the screen decided.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// the administrator `name` was created; the next logon should use it.
    Created(String),
    /// carry on as root for now.
    Skipped,
    /// the first-run setup is over: the administrator `name` exists and root's session has ended.
    /// `note` says what did not work when the account was made but root's ssh setting was not.
    FirstRun { name: String, note: Option<String> },
}

/// which page of the first-run setup is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Account,
    RootSsh,
}

/// what the first-run setup says about root's ssh access, and what each choice means.
const ROOT_SSH_INTRO: &str = "Choose how root may sign in to this server over ssh. This is \
separate from this page: it is about the ssh daemon on the server.";

/// the choices of the root ssh page, in the order they are listed; the last leaves sshd alone.
fn root_ssh_options() -> Vec<(usize, String)> {
    RootSsh::ALL
        .iter()
        .map(|level| level.label().to_string())
        .enumerate()
        .collect()
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
    Password(Zeroizing<String>),
    Confirm(Zeroizing<String>),
    Key(String),
    /// the key at this index of the agent's keys.
    AgentKey(usize),
    TogglePasswordless,
    Skip,
    Create,
    /// first run: on to the root ssh page, or back to the account.
    Next,
    Back,
    /// first run: the choice at this index of the root ssh page.
    RootSsh(usize),
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
    job: Option<(Job<Option<String>>, String)>,
    /// the first-run setup of the web ui: no skipping, two pages, one request at the end.
    first_run: bool,
    step: Step,
    /// root's ssh access as chosen; none leaves sshd as it is.
    root_ssh: Option<RootSsh>,
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
    #[cfg(feature = "native")]
    pub fn new() -> Self {
        Self::with_keys(lanlord_core::signin::agent_keys())
    }

    /// the screen; the browser has no ssh agent to offer keys from.
    #[cfg(not(feature = "native"))]
    pub fn new() -> Self {
        Self::with_keys(Vec::new())
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
            first_run: false,
            step: Step::Account,
            root_ssh: None,
            status: String::new(),
            status_is_error: false,
            focus: false,
        }
    }

    /// the first-run setup the web ui makes root go through: create the administrator and
    /// choose root's ssh access, with nothing else to do.
    pub fn first_run() -> Self {
        let mut setup = Self::with_keys(Vec::new());
        setup.first_run = true;
        setup
    }

    pub fn with_test_keys(keys: &[&str]) -> Self {
        Self::with_keys(keys.iter().map(ToString::to_string).collect())
    }

    pub fn set_passwords(&mut self, password: &str, confirm: &str) {
        self.password = Zeroizing::new(password.to_string());
        self.confirm = Zeroizing::new(confirm.to_string());
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn is_first_run(&self) -> bool {
        self.first_run
    }

    /// whether the root ssh page is showing (first run only).
    pub fn on_root_ssh_page(&self) -> bool {
        self.step == Step::RootSsh
    }

    pub fn root_ssh_choice(&self) -> Option<RootSsh> {
        self.root_ssh
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
        if !self.account_ok() {
            return;
        }
        let name = self.name.trim().to_string();
        if self.first_run {
            self.finish(services, host, name);
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
            gateway.create_admin(&target, &new, ask).map(|()| None)
        });
        match job {
            Some(job) => {
                self.job = Some((job, name));
                self.say("Creating the account...", false);
            }
            None => self.say("The gateway is busy. Try again in a moment.", true),
        }
    }

    /// check the account fields, saying what is wrong.
    fn account_ok(&mut self) -> bool {
        let name = self.name.trim().to_string();
        if name.is_empty() {
            self.say("Type a username for the new administrator.", true);
            return false;
        }
        if self.password.is_empty() {
            self.say("Type a password for the new account.", true);
            return false;
        }
        if *self.password != *self.confirm {
            self.say("The two passwords are not the same.", true);
            return false;
        }
        if self.first_run {
            // the same rules the server applies, so a bad name is caught before the last page
            let key = self.key.trim();
            let check = Operation::SetupFirstAdmin {
                name,
                password: Secret::new("x"),
                root_ssh: RootSsh::KeysOnly,
                ssh_key: (!key.is_empty()).then(|| key.to_string()),
            };
            if let Err(invalid) = check.validate() {
                let what = if invalid.field == "ssh_key" {
                    "The ssh public key is not valid"
                } else {
                    "The username is not allowed"
                };
                self.say(&format!("{what}: {}.", invalid.reason), true);
                return false;
            }
        }
        true
    }

    /// first run: send the account and the root ssh choice in one request. the password leaves
    /// the form for the task, which holds the only copy.
    fn finish(&mut self, services: &Services, host: &str, name: String) {
        // nothing is preselected: the step cannot finish until one level is picked
        let Some(root_ssh) = self.root_ssh else {
            self.say("Choose how root may sign in over ssh.", true);
            return;
        };
        let (account, password) = (
            name.clone(),
            Zeroizing::new(std::mem::take(&mut *self.password)),
        );
        self.confirm = Zeroizing::new(String::new());
        let key = self.key.trim().to_string();
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| {
            let key = (!key.is_empty()).then_some(key.as_str());
            gateway.setup_first_admin(&target, &account, password.as_str(), (root_ssh, key), ask)
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
            Ok(note) if self.first_run => Some(Outcome::FirstRun { name, note }),
            Ok(_) => Some(Outcome::Created(name)),
            Err(error) => {
                // the password was spent; it has to be typed again
                if self.first_run {
                    self.step = Step::Account;
                }
                self.say(&error, true);
                None
            }
        }
    }

    /// the title of the screen's box.
    pub fn title(&self) -> &'static str {
        if self.first_run {
            return "First-Run Setup";
        }
        "Create Administrator Account"
    }

    /// the screen's contents.
    pub fn view(&mut self) -> Vec<Node<Msg>> {
        if self.first_run {
            return self.first_run_view();
        }
        let busy = self.job.is_some() || self.warning.is_some();
        let first = !self.focus;
        self.focus = true;
        let secret =
            |label: &str, value: &Zeroizing<String>, edit: fn(Zeroizing<String>) -> Msg| {
                Node::Labelled {
                    label: label.into(),
                    body: vec![Node::Secret {
                        value: value.clone(),
                        size: Size::Points(260.0),
                        shown: false,
                        focus: false,
                        edit: Box::new(edit),
                        enter: None,
                    }],
                }
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
                    size: Size::Points(260.0),
                    hint: String::new(),
                    focus: first,
                    edit: Box::new(Msg::Name),
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

    /// the two pages of the first-run setup.
    fn first_run_view(&mut self) -> Vec<Node<Msg>> {
        let busy = self.job.is_some();
        let first = !self.focus;
        self.focus = true;
        let status = Node::Status {
            text: self.status.clone(),
            error: self.status_is_error,
        };
        if self.step == Step::RootSsh {
            let current = self
                .root_ssh
                .map_or("Choose one", RootSsh::label)
                .to_string();
            let mut nodes = vec![
                Node::Header {
                    icon: "gateway",
                    title: "Step 2 of 2: root ssh access".into(),
                    lines: vec![ROOT_SSH_INTRO.into()],
                },
                Node::Gap(8.0),
                // the label is short on purpose: the column is narrow and clips what is longer
                Node::Labelled {
                    label: "Root ssh:".into(),
                    body: vec![Node::Choice {
                        id: "first-run-root-ssh",
                        current,
                        width: 260.0,
                        options: root_ssh_options(),
                        pick: Msg::RootSsh,
                    }],
                },
                Node::Gap(6.0),
                Node::Text("Pick one of the three. You can change it later in My Server.".into()),
            ];
            // every choice is described all the time, so picking one moves nothing
            let has_key = !self.key.trim().is_empty();
            for level in RootSsh::ALL {
                let mut text = format!("{}: {}", level.label(), level.explain());
                if let Some(warning) = level.setup_warning(has_key) {
                    text.push(' ');
                    text.push_str(warning);
                }
                nodes.push(Node::Bullet(text));
            }
            nodes.extend([
                Node::Gap(10.0),
                Node::Buttons(vec![
                    Button::new("Back", !busy, Msg::Back),
                    Button::new("Finish", !busy, Msg::Create),
                ]),
                Node::Gap(6.0),
                status,
            ]);
            return nodes;
        }
        let secret =
            |label: &str, value: &Zeroizing<String>, edit: fn(Zeroizing<String>) -> Msg| {
                Node::Labelled {
                    label: label.into(),
                    body: vec![Node::Secret {
                        value: value.clone(),
                        size: Size::Points(260.0),
                        shown: false,
                        focus: false,
                        edit: Box::new(edit),
                        enter: None,
                    }],
                }
            };
        vec![
            Node::Header {
                icon: "gateway",
                title: "Step 1 of 2: your administrator account".into(),
                lines: vec![
                    "This server has no administrator yet. You are signed in as root only to \
                     set it up. Create the account you will use from now on; after the next \
                     step you sign in with it."
                        .into(),
                ],
            },
            Node::Gap(8.0),
            Node::Labelled {
                label: "Username:".into(),
                body: vec![Node::Input {
                    value: self.name.clone(),
                    size: Size::Points(260.0),
                    hint: String::new(),
                    focus: first,
                    edit: Box::new(Msg::Name),
                    enter: None,
                }],
            },
            Node::Gap(4.0),
            secret("Password:", &self.password, Msg::Password),
            Node::Gap(4.0),
            secret("Confirm:", &self.confirm, Msg::Confirm),
            Node::Gap(4.0),
            Node::Group {
                title: "SSH public key (optional)".into(),
                body: vec![Node::TextArea {
                    id: "first-run-key",
                    value: self.key.clone(),
                    height: 48.0,
                    hint: "ssh-ed25519 AAAA... paste your public key here".into(),
                    edit: Msg::Key,
                }],
            },
            Node::Gap(10.0),
            Node::Buttons(vec![Button::new("Next", !busy, Msg::Next)]),
            Node::Gap(6.0),
            status,
        ]
    }

    /// act on a click or keystroke; `Some` once the screen is over.
    pub fn update(&mut self, msg: Msg, services: &Services, host: &str) -> Option<Outcome> {
        match msg {
            Msg::Name(text) => self.name = text,
            Msg::Password(text) => self.password = text,
            Msg::Confirm(text) => self.confirm = text,
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
            // first run has no skipping: root is for this setup and nothing else
            Msg::Skip if self.first_run => {}
            Msg::Next if self.first_run && self.job.is_none() => {
                if self.account_ok() {
                    self.say("", false);
                    self.step = Step::RootSsh;
                }
            }
            Msg::Back if self.first_run && self.job.is_none() => self.step = Step::Account,
            Msg::RootSsh(i) if self.first_run => self.root_ssh = RootSsh::ALL.get(i).copied(),
            // the same rule as the buttons: nothing while a task or the warning is open
            Msg::Skip | Msg::Create if self.job.is_some() || self.warning.is_some() => {}
            Msg::Skip => return Some(Outcome::Skipped),
            // the root ssh page is the last one: finishing sends everything
            Msg::Create if self.first_run && self.step == Step::Account => {}
            Msg::Create => self.create(services, host),
            Msg::Next | Msg::Back | Msg::RootSsh(_) => {}
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
pub fn key_label(key: &str) -> String {
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
        let wait = std::sync::Mutex::new(wait);
        let held = services.spawn("root@gw", move |_, _| {
            let _ = wait.lock().unwrap().recv();
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

    #[test]
    fn skip_and_create_wait_for_the_warning() {
        let services = Services::new(
            std::sync::Arc::new(crate::gateway::Fake::new()),
            Wake::none(),
        );
        let mut setup = AdminSetup::with_test_keys(&[]);
        setup.update(Msg::TogglePasswordless, &services, "root@gw");
        assert!(setup.warning_open());
        assert_eq!(setup.update(Msg::Skip, &services, "root@gw"), None);
        setup.answer_warning(1);
        assert!(!setup.passwordless());
        assert_eq!(
            setup.update(Msg::Skip, &services, "root@gw"),
            Some(Outcome::Skipped)
        );
    }

    fn finish_first_run(setup: &mut AdminSetup) -> Outcome {
        let start = std::time::Instant::now();
        loop {
            if let Some(outcome) = setup.poll() {
                return outcome;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(5),
                "timed out: {}",
                setup.status()
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn first_run_takes_the_account_then_the_root_ssh_choice_and_sends_both_once() {
        use std::sync::Arc;

        let fake = Arc::new(crate::gateway::Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
        let mut setup = AdminSetup::first_run();
        assert!(setup.is_first_run() && !setup.on_root_ssh_page());
        // skipping is not offered, and not honoured
        assert_eq!(setup.update(Msg::Skip, &services, "gw"), None);
        // the account page checks before the next one
        setup.update(Msg::Next, &services, "gw");
        assert!(!setup.on_root_ssh_page() && setup.status().contains("username"));
        setup.name = "root".into();
        setup.set_passwords("pw", "pw");
        setup.update(Msg::Next, &services, "gw");
        assert!(!setup.on_root_ssh_page() && setup.status().contains("not allowed"));
        setup.name = "pat".into();
        setup.set_passwords("pw", "other");
        setup.update(Msg::Next, &services, "gw");
        assert!(!setup.on_root_ssh_page() && setup.status().contains("not the same"));
        setup.set_passwords("pw", "pw");
        // finishing from the first page does nothing
        setup.update(Msg::Create, &services, "gw");
        assert!(!setup.working());
        setup.update(Msg::Next, &services, "gw");
        assert!(setup.on_root_ssh_page());
        setup.update(Msg::Back, &services, "gw");
        assert!(!setup.on_root_ssh_page());
        setup.update(Msg::Next, &services, "gw");
        setup.update(Msg::RootSsh(1), &services, "gw");
        assert_eq!(setup.root_ssh_choice(), Some(RootSsh::KeysOnly));
        setup.update(Msg::Create, &services, "gw");
        assert!(setup.working());
        let outcome = finish_first_run(&mut setup);
        assert_eq!(
            outcome,
            Outcome::FirstRun {
                name: "pat".into(),
                note: None
            }
        );
        let calls = fake.calls();
        assert_eq!(calls, ["setup gw pat root-ssh=KeysOnly"]);
        assert!(
            !calls.concat().contains("pw"),
            "the password is not recorded"
        );
    }

    #[test]
    fn first_run_needs_a_pick_and_a_failure_goes_back_to_the_account() {
        use std::sync::Arc;

        let fake = Arc::new(crate::gateway::Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
        let mut setup = AdminSetup::first_run();
        setup.name = "pat".into();
        setup.set_passwords("pw", "pw");
        setup.update(Msg::Next, &services, "gw");
        // nothing is preselected, and finishing without a pick sends nothing
        assert_eq!(setup.root_ssh_choice(), None);
        setup.update(Msg::Create, &services, "gw");
        assert!(!setup.working() && setup.status().contains("Choose"));
        assert!(fake.calls().is_empty());
        // there is no fourth option to pick
        setup.update(Msg::RootSsh(3), &services, "gw");
        assert_eq!(setup.root_ssh_choice(), None);
        setup.update(Msg::RootSsh(0), &services, "gw");
        assert_eq!(setup.root_ssh_choice(), Some(RootSsh::Allowed));
        *fake.fail.lock().unwrap() = Some("an administrator already exists".into());
        setup.update(Msg::Create, &services, "gw");
        let start = std::time::Instant::now();
        while setup.working() {
            assert!(
                start.elapsed() < std::time::Duration::from_secs(5),
                "timed out"
            );
            assert_eq!(setup.poll(), None);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            setup.status().contains("already exists"),
            "{}",
            setup.status()
        );
        assert!(
            !setup.on_root_ssh_page(),
            "the password has to be typed again"
        );
        assert_eq!(fake.calls(), ["setup gw pat root-ssh=Allowed"]);
    }

    #[test]
    fn first_run_takes_an_optional_valid_ssh_key_and_sends_it() {
        use std::sync::Arc;

        let fake = Arc::new(crate::gateway::Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
        let mut setup = AdminSetup::first_run();
        setup.name = "pat".into();
        setup.set_passwords("pw", "pw");
        setup.key = "not a key".into();
        setup.update(Msg::Next, &services, "gw");
        assert!(!setup.on_root_ssh_page() && setup.status().contains("ssh public key"));
        setup.key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGoZ pat@laptop".into();
        setup.update(Msg::Next, &services, "gw");
        assert!(setup.on_root_ssh_page());
        setup.update(Msg::RootSsh(2), &services, "gw");
        setup.update(Msg::Create, &services, "gw");
        finish_first_run(&mut setup);
        assert_eq!(fake.calls(), ["setup gw pat root-ssh=Off key"]);
    }

    #[test]
    fn the_root_ssh_page_does_not_move_when_a_choice_is_picked() {
        let services = Services::new(
            std::sync::Arc::new(crate::gateway::Fake::new()),
            Wake::none(),
        );
        let mut setup = AdminSetup::first_run();
        setup.name = "pat".into();
        setup.set_passwords("pw", "pw");
        setup.update(Msg::Next, &services, "gw");
        let shape = |setup: &mut AdminSetup| -> Vec<String> {
            setup
                .view()
                .iter()
                .map(|n| match n {
                    Node::Bullet(t) | Node::Text(t) => t.clone(),
                    _ => String::new(),
                })
                .collect()
        };
        let before = shape(&mut setup);
        for pick in 0..3 {
            setup.update(Msg::RootSsh(pick), &services, "gw");
            assert_eq!(shape(&mut setup), before, "choice {pick}");
        }
        // keys only says the administrator needs a key while it has none
        assert!(before.iter().any(|t| t.contains("no ssh key")));
        setup.key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGoZ pat@laptop".into();
        assert!(!shape(&mut setup).iter().any(|t| t.contains("no ssh key")));
    }

    #[test]
    fn the_root_ssh_page_lists_exactly_the_three_levels() {
        let labels: Vec<String> = root_ssh_options().into_iter().map(|(_, l)| l).collect();
        let levels: Vec<&str> = RootSsh::ALL.iter().map(|l| l.label()).collect();
        assert_eq!(labels, levels);
        assert_eq!(labels.len(), 3);
    }
}
