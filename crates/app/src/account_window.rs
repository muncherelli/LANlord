//! My Account: the signed-in person's profile, the accounts linked to theirs and an NT4-style
//! Change Password box. a password change goes to every linked account or to none, and the
//! window says how each one fared.

use lanlord_model::access::Profile;
use lanlord_model::identity::{AccountInfo, LinkState, PasswordReport};

use crate::form::{Form, FormAnswer, FormMsg};
use crate::remote::Remote;
use crate::services::Services;
use crate::view::{self, Button, Dialog, Node, Size, Table};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    FullName(String),
    Description(String),
    SaveProfile,
    ChangePassword,
    Reload,
    /// the text of the key to add.
    KeyDraft(String),
    AddKey,
    /// the key at this row of the list.
    SelectKey(usize),
    RemoveKey,
}

#[derive(Default)]
pub struct AccountWindow {
    remote: Remote<AccountInfo>,
    /// the outcome of the last password change.
    outcome: Remote<PasswordReport>,
    draft: Profile,
    form: Option<Form>,
    /// the person's own authorized ssh keys.
    keys: Remote<Vec<String>>,
    key_draft: String,
    key_selected: Option<usize>,
}

/// what a person sees for one link's result.
pub fn describe(state: &LinkState) -> String {
    match state {
        LinkState::Changed => "changed".into(),
        LinkState::Skipped(why) => format!("skipped: {why}"),
        LinkState::Failed(why) => format!("failed: {why}"),
        LinkState::RolledBack => "put back".into(),
        LinkState::RollbackFailed(why) => format!("could not be put back: {why}"),
        LinkState::NotTried => "left alone".into(),
    }
}

/// what is wrong with the three password fields, if anything.
pub fn password_problem(new: &str, confirm: &str) -> Option<&'static str> {
    if new.is_empty() {
        Some("Type the new password.")
    } else if new != confirm {
        Some("The new password and its confirmation are different.")
    } else {
        None
    }
}

impl AccountWindow {
    pub fn poll(&mut self) {
        if self.remote.poll() {
            if let Some(info) = &self.remote.data {
                self.draft = info.profile.clone();
            }
        }
        self.outcome.poll();
        if self.keys.poll() {
            self.key_selected = None;
        }
    }

    fn idle(&self, services: &Services, host: &str) -> bool {
        !services.busy(host) && !self.remote.busy() && !self.outcome.busy() && !self.keys.busy()
    }

    fn load_keys(&mut self, services: &Services, host: &str) {
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| gateway.my_keys(&target, ask));
        self.keys.track(
            host,
            job,
            "loading your ssh keys...",
            "loaded your ssh keys",
            true,
        );
    }

    /// add the typed key, or remove the selected one, then read the list again.
    fn change_keys(&mut self, services: &Services, host: &str, add: bool) {
        let key = if add {
            self.key_draft.trim().to_string()
        } else {
            // a key is removed by its id, so a line with options in front is found too
            match self.selected_key() {
                Some(key) => lanlord_model::sshauth::key_id(&key).unwrap_or(key),
                None => return,
            }
        };
        if add {
            if let Err(why) = lanlord_model::validate::ssh_key(&key) {
                self.keys
                    .say(true, format!("That is not an ssh public key: {why}."));
                return;
            }
            self.key_draft.clear();
        }
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| {
            if add {
                gateway.add_my_key(&target, &key, ask)?;
            } else {
                gateway.remove_my_key(&target, &key, ask)?;
            }
            gateway.my_keys(&target, ask)
        });
        let doing = if add {
            "adding the key..."
        } else {
            "removing the key..."
        };
        self.keys
            .track(host, job, doing, "saved your ssh keys", false);
    }

    fn selected_key(&self) -> Option<String> {
        let keys = self.keys.data.as_ref()?;
        keys.get(self.key_selected?).cloned()
    }

    fn key_group(&self, idle: bool) -> Node<Msg> {
        let rows = self.keys.data.iter().flatten().map(|k| {
            let data = k.split_whitespace().nth(1).unwrap_or("");
            let tail: String = data
                .chars()
                .rev()
                .take(12)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            vec![crate::admin_setup::key_label(k), format!("...{tail}")]
        });
        Node::Fold {
            id: "account-keys-fold",
            title: "SSH keys (sign in over ssh without a password)".into(),
            body: vec![
                Node::Table(Table {
                    id: "account-keys",
                    headings: vec!["Key", "Ends with"],
                    widths: vec![260.0],
                    rows: rows.collect(),
                    selected: self.key_selected,
                    bad: Vec::new(),
                    height: 64.0,
                    select: Msg::SelectKey,
                    steady: false,
                }),
                Node::TextArea {
                    id: "account-key-draft",
                    value: self.key_draft.clone(),
                    height: 40.0,
                    hint: "ssh-ed25519 AAAA... paste a public key here".into(),
                    edit: Msg::KeyDraft,
                },
                Node::Toolbar(vec![
                    Button::new(
                        "Add key",
                        idle && !self.key_draft.trim().is_empty(),
                        Msg::AddKey,
                    ),
                    Button::new(
                        "Remove selected",
                        idle && self.key_selected.is_some(),
                        Msg::RemoveKey,
                    ),
                ]),
            ],
        }
    }

    fn reload(&mut self, services: &Services, host: &str) {
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| gateway.account_info(&target, ask));
        self.remote
            .track(host, job, "loading...", "loaded your account", true);
    }

    fn save_profile(&mut self, services: &Services, host: &str) {
        let (target, profile) = (host.to_string(), self.draft.clone());
        let job = services.spawn(host, move |gateway, ask| {
            gateway.update_profile(&target, &profile, ask)?;
            gateway.account_info(&target, ask)
        });
        self.remote.track(host, job, "saving...", "saved", false);
    }

    fn links(&self) -> Vec<Vec<String>> {
        let Some(info) = &self.remote.data else {
            return Vec::new();
        };
        info.links
            .iter()
            .map(|l| vec![l.link.clone(), l.status.clone()])
            .collect()
    }

    fn outcome_nodes(&self) -> Vec<Node<Msg>> {
        let Some(report) = &self.outcome.data else {
            return Vec::new();
        };
        let mut nodes = vec![Node::Gap(4.0)];
        let headline = if report.ok {
            "Your password was changed."
        } else {
            "Your password was not changed; nothing was left half done."
        };
        nodes.push(Node::Text(headline.into()));
        for link in &report.links {
            nodes.push(Node::field(&link.link, describe(&link.state)));
        }
        nodes
    }

    pub fn view(&mut self, services: &Services, host: Option<&str>) -> Vec<Node<Msg>> {
        let Some(host) = host else {
            return vec![Node::Text("No server yet. Sign in to one first.".into())];
        };
        if self.remote.wants_load(host, false) {
            self.reload(services, host);
        }
        let mut idle = self.idle(services, host);
        // the keys follow the account, once the gateway is free
        if idle
            && self.remote.belongs_to(host)
            && self.keys.wants_load(host, !self.key_draft.is_empty())
        {
            self.load_keys(services, host);
            idle = false;
        }
        let info = self.remote.data.clone().unwrap_or_default();
        let mut nodes = vec![
            Node::Header {
                icon: "my-account",
                title: info.user.clone(),
                lines: vec![format!("Roles: {}", info.roles.join(", "))],
            },
            Node::Gap(4.0),
            self.profile_group(idle),
            Node::Gap(4.0),
            Node::Group {
                title: "Linked accounts".into(),
                body: vec![Node::Table(Table {
                    id: "account-links",
                    headings: vec!["Account", "Status"],
                    widths: vec![200.0],
                    rows: self.links(),
                    selected: None,
                    bad: Vec::new(),
                    height: 64.0,
                    select: |_| Msg::Reload,
                    steady: false,
                })],
            },
            Node::Gap(4.0),
            self.key_group(idle),
            Node::Gap(4.0),
            Node::Toolbar(vec![Button::new(
                "Change Password...",
                !self.remote.busy() && !self.outcome.busy() && self.form.is_none(),
                Msg::ChangePassword,
            )]),
        ];
        nodes.extend(self.outcome_nodes());
        nodes.push(Node::Gap(4.0));
        // the keys' own message shows while it is the latest thing that happened
        nodes.push(match self.keys.note {
            Some((true, _)) => view::status(&self.keys),
            _ => view::status(&self.remote),
        });
        nodes
    }

    fn profile_group(&self, idle: bool) -> Node<Msg> {
        // a save or reload in flight replaces the draft when it ends, so nothing is typed meanwhile
        let busy = self.remote.busy();
        let input = |value: &str, edit: fn(String) -> Msg| {
            let field = Node::Input {
                value: value.into(),
                size: Size::FillLess(8.0),
                focus: false,
                hint: String::new(),
                edit: Box::new(edit),
                enter: None,
            };
            if busy {
                Node::Disabled(vec![field])
            } else {
                field
            }
        };
        Node::Group {
            title: "Profile".into(),
            body: vec![
                Node::Labelled {
                    label: "Full name".into(),
                    body: vec![input(&self.draft.full_name, Msg::FullName)],
                },
                Node::Labelled {
                    label: "Description".into(),
                    body: vec![input(&self.draft.description, Msg::Description)],
                },
                Node::Toolbar(vec![Button::new(
                    "Save profile",
                    idle && self.profile_changed(),
                    Msg::SaveProfile,
                )]),
            ],
        }
    }

    fn profile_changed(&self) -> bool {
        self.remote
            .data
            .as_ref()
            .is_some_and(|d| d.profile != self.draft)
    }

    pub fn update(&mut self, msg: Msg, services: &Services, host: &str) {
        match msg {
            Msg::FullName(text) if !self.remote.busy() => self.draft.full_name = text,
            Msg::Description(text) if !self.remote.busy() => self.draft.description = text,
            Msg::SaveProfile if self.idle(services, host) => self.save_profile(services, host),
            Msg::Reload if self.idle(services, host) => self.reload(services, host),
            Msg::KeyDraft(text) if !self.keys.busy() => self.key_draft = text,
            Msg::SelectKey(row) => self.key_selected = Some(row),
            Msg::AddKey if self.idle(services, host) => self.change_keys(services, host, true),
            Msg::RemoveKey if self.idle(services, host) => self.change_keys(services, host, false),
            Msg::ChangePassword if self.form.is_none() => self.form = Some(password_form()),
            _ => {}
        }
    }

    pub fn dialog(&mut self) -> Option<Dialog<FormMsg>> {
        self.form.as_mut().map(Form::view)
    }

    /// act on the Change Password box; a good one starts the change.
    pub fn update_dialog(&mut self, msg: FormMsg, services: &Services, host: &str) {
        let Some(mut form) = self.form.take() else {
            return;
        };
        match form.update(msg) {
            None => self.form = Some(form),
            Some(FormAnswer::Cancel) => {}
            Some(FormAnswer::Ok) => match password_problem(form.raw(1), form.raw(2)) {
                Some(problem) => {
                    form.error = Some(problem.into());
                    self.form = Some(form);
                }
                None => self.change(&form, services, host),
            },
        }
    }

    fn change(&mut self, form: &Form, services: &Services, host: &str) {
        use zeroize::Zeroizing;
        let (target, old, new) = (
            host.to_string(),
            Zeroizing::new(form.raw(0).to_string()),
            Zeroizing::new(form.raw(1).to_string()),
        );
        let job = services.spawn(host, move |gateway, ask| {
            gateway.change_password(&target, &old, &new, ask)
        });
        self.outcome
            .track(host, job, "changing the password...", "done", false);
    }
}

/// the NT4-style box: the old password, the new one and the new one again.
pub fn password_form() -> Form {
    Form::new("change-password", "Change Password", "")
        .secret("Old Password:")
        .secret("New Password:")
        .secret("Confirm New Password:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_new_password_must_be_typed_twice_the_same() {
        assert!(password_problem("", "").is_some());
        assert!(password_problem("a", "b").is_some());
        assert_eq!(password_problem("a", "a"), None);
    }

    #[test]
    fn every_link_state_reads_as_a_sentence_fragment() {
        assert_eq!(describe(&LinkState::Changed), "changed");
        assert!(describe(&LinkState::Failed("no".into())).contains("no"));
        assert_eq!(describe(&LinkState::NotTried), "left alone");
    }

    #[test]
    fn a_mismatched_confirmation_keeps_the_box_open_with_the_reason() {
        let services = Services::new(
            std::sync::Arc::new(crate::gateway::Fake::new()),
            crate::wake::Wake::none(),
        );
        let mut window = AccountWindow::default();
        window.update(Msg::ChangePassword, &services, "gw");
        let mut form = window.form.take().unwrap();
        form.set(0, "old");
        form.set(1, "one");
        form.set(2, "two");
        window.form = Some(form);
        window.update_dialog(FormMsg::Ok, &services, "gw");
        let form = window.form.as_ref().unwrap();
        assert!(form.error.as_deref().unwrap().contains("different"));
        assert!(!window.outcome.busy());
    }

    fn setup() -> (
        AccountWindow,
        Services,
        std::sync::Arc<crate::gateway::Fake>,
    ) {
        let fake = std::sync::Arc::new(crate::gateway::Fake::new());
        let services = Services::new(fake.clone(), crate::wake::Wake::none());
        (AccountWindow::default(), services, fake)
    }

    fn settle(window: &mut AccountWindow, services: &Services) {
        let start = std::time::Instant::now();
        loop {
            window.poll();
            if window.idle(services, "gw") {
                return;
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(5), "stuck");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn without_a_server_the_window_says_so() {
        let (mut window, services, _) = setup();
        let nodes = window.view(&services, None);
        assert_eq!(nodes.len(), 1);
    }

    #[test]
    fn the_account_loads_once_and_fills_the_profile() {
        let (mut window, services, fake) = setup();
        let _ = window.view(&services, Some("gw"));
        settle(&mut window, &services);
        assert!(window.remote.data.is_some());
        let nodes = window.view(&services, Some("gw"));
        assert!(nodes.len() > 4);
        settle(&mut window, &services);
        let _ = window.view(&services, Some("gw"));
        settle(&mut window, &services);
        assert_eq!(
            fake.calls(),
            ["account gw", "my-keys gw"],
            "it does not reload by itself"
        );
        window.update(Msg::Reload, &services, "gw");
        settle(&mut window, &services);
        assert_eq!(fake.calls().len(), 3);
    }

    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGoZ pat@laptop";

    #[test]
    fn a_key_can_be_added_and_removed_and_a_bad_one_is_refused() {
        let (mut window, services, fake) = setup();
        let _ = window.view(&services, Some("gw"));
        settle(&mut window, &services);
        let _ = window.view(&services, Some("gw"));
        settle(&mut window, &services);
        window.update(Msg::KeyDraft("hello".into()), &services, "gw");
        window.update(Msg::AddKey, &services, "gw");
        assert!(matches!(&window.keys.note, Some((true, t)) if t.contains("ssh public key")));
        assert!(fake.keys.lock().unwrap().is_empty());
        window.update(Msg::KeyDraft(KEY.into()), &services, "gw");
        window.update(Msg::AddKey, &services, "gw");
        settle(&mut window, &services);
        assert_eq!(*fake.keys.lock().unwrap(), [KEY]);
        assert_eq!(window.keys.data.as_deref(), Some(&[KEY.to_string()][..]));
        assert!(window.key_draft.is_empty());
        window.update(Msg::RemoveKey, &services, "gw");
        settle(&mut window, &services);
        assert_eq!(
            fake.keys.lock().unwrap().len(),
            1,
            "nothing selected, nothing removed"
        );
        window.update(Msg::SelectKey(0), &services, "gw");
        window.update(Msg::RemoveKey, &services, "gw");
        settle(&mut window, &services);
        assert!(fake.keys.lock().unwrap().is_empty());
    }

    #[test]
    fn a_key_with_options_in_front_is_removed_by_its_id() {
        let (mut window, services, fake) = setup();
        let line = format!(r#"from="192.0.2.7",command="true" {KEY}"#);
        *fake.keys.lock().unwrap() = vec![line];
        let start = std::time::Instant::now();
        while window.keys.data.is_none() {
            let _ = window.view(&services, Some("gw"));
            window.poll();
            assert!(start.elapsed() < std::time::Duration::from_secs(5), "stuck");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        settle(&mut window, &services);
        window.update(Msg::SelectKey(0), &services, "gw");
        window.update(Msg::RemoveKey, &services, "gw");
        settle(&mut window, &services);
        assert!(fake.keys.lock().unwrap().is_empty());
    }

    #[test]
    fn a_changed_profile_can_be_saved() {
        let (mut window, services, fake) = setup();
        let _ = window.view(&services, Some("gw"));
        settle(&mut window, &services);
        assert!(!window.profile_changed());
        window.update(Msg::FullName("Dana Example".into()), &services, "gw");
        window.update(Msg::Description("front desk".into()), &services, "gw");
        assert!(window.profile_changed());
        window.update(Msg::SaveProfile, &services, "gw");
        settle(&mut window, &services);
        assert!(fake.calls().contains(&"profile gw".to_string()));
    }

    #[test]
    fn a_good_password_box_changes_the_password_and_the_outcome_is_shown() {
        let (mut window, services, fake) = setup();
        window.update(Msg::ChangePassword, &services, "gw");
        assert!(window.dialog().is_some());
        let mut form = window.form.take().unwrap();
        form.set(0, "old");
        form.set(1, "new");
        form.set(2, "new");
        window.form = Some(form);
        window.update_dialog(FormMsg::Ok, &services, "gw");
        assert!(window.form.is_none());
        settle(&mut window, &services);
        assert!(fake.calls().contains(&"password gw".to_string()));
        let nodes = window.view(&services, Some("gw"));
        assert!(nodes.len() > 8, "the outcome is listed");
    }

    #[test]
    fn cancelling_the_password_box_changes_nothing() {
        let (mut window, services, fake) = setup();
        window.update(Msg::ChangePassword, &services, "gw");
        window.update_dialog(FormMsg::Cancel, &services, "gw");
        assert!(window.form.is_none() && window.dialog().is_none());
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn the_profile_is_not_edited_while_a_load_or_save_is_in_flight() {
        let (mut window, services, _) = setup();
        let _ = window.view(&services, Some("gw"));
        assert!(window.remote.busy());
        window.update(Msg::FullName("typed".into()), &services, "gw");
        window.update(Msg::Description("typed".into()), &services, "gw");
        assert_eq!(window.draft, Profile::default());
        let nodes = window.view(&services, Some("gw"));
        let disabled = nodes.iter().any(|n| {
            matches!(n, Node::Group { title, body }
                if title == "Profile" && body.iter().any(|b| matches!(b,
                    Node::Labelled { body, .. } if matches!(body[0], Node::Disabled(_)))))
        });
        assert!(disabled);
        settle(&mut window, &services);
        window.update(Msg::FullName("typed".into()), &services, "gw");
        assert_eq!(window.draft.full_name, "typed");
    }
}
