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
}

#[derive(Default)]
pub struct AccountWindow {
    remote: Remote<AccountInfo>,
    /// the outcome of the last password change.
    outcome: Remote<PasswordReport>,
    draft: Profile,
    form: Option<Form>,
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
    }

    fn idle(&self, services: &Services, host: &str) -> bool {
        !services.busy(host) && !self.remote.busy() && !self.outcome.busy()
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
        let idle = self.idle(services, host);
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
                })],
            },
            Node::Gap(4.0),
            Node::Toolbar(vec![Button::new(
                "Change Password...",
                idle && self.form.is_none(),
                Msg::ChangePassword,
            )]),
        ];
        nodes.extend(self.outcome_nodes());
        nodes.push(Node::Gap(4.0));
        nodes.push(view::status(&self.remote));
        nodes
    }

    fn profile_group(&self, idle: bool) -> Node<Msg> {
        let input = |value: &str, edit: fn(String) -> Msg| Node::Input {
            value: value.into(),
            size: Size::FillLess(8.0),
            focus: false,
            hint: String::new(),
            edit: Box::new(edit),
            enter: None,
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
            Msg::FullName(text) => self.draft.full_name = text,
            Msg::Description(text) => self.draft.description = text,
            Msg::SaveProfile if self.idle(services, host) => self.save_profile(services, host),
            Msg::Reload if self.idle(services, host) => self.reload(services, host),
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
}
