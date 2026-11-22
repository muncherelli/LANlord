//! the nt4 domain's settings tab: domain, server name, machines, roaming profiles and the
//! administrator password (which is only ever set, never shown).

use lanlord_model::nt4::{valid_machine, Nt4Settings};

use crate::form::{Form, FormAnswer, FormMsg};
use crate::remote::Remote;
use crate::services::{update_settings, Services};
use crate::view::{self, Button, Dialog, Node, Size, Table};

const ADMIN_PASSWORD: &str = "nt4-domain.admin_password";
const MODULE: &str = "nt4-domain";

pub struct Nt4Data {
    pub settings: Nt4Settings,
    pub admin_password_set: bool,
}

fn load(
    gateway: &dyn crate::gateway::Gateway,
    host: &str,
    ask: crate::ask::AskPassword,
) -> anyhow::Result<Nt4Data> {
    let document = gateway.load_document(host, ask)?;
    let settings = lanlord_model::settings::Settings::parse(&document.plain)?;
    Ok(Nt4Data {
        settings: Nt4Settings::from_settings(&settings)?,
        admin_password_set: document.secrets_set.iter().any(|p| p == ADMIN_PASSWORD),
    })
}

enum Ask {
    Machine,
    AdminPassword,
    /// reset the account of this machine.
    Reset(String),
}

/// what a click or a keystroke in the panel means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    Name(String),
    ServerName(String),
    /// the mode at this index of `MODES`.
    Mode(usize),
    ToggleRoaming,
    ToggleSoftware,
    SetPassword,
    Pick(usize),
    AddMachine,
    RemoveMachine,
    ResetMachine,
    Reload,
    SaveAndApply,
    Save,
}

const MODES: [&str; 2] = ["domain", "workgroup"];

/// which buttons work right now; the view greys out the rest and `update` ignores them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gates {
    pub set_password: bool,
    pub add_machine: bool,
    pub remove_machine: bool,
    pub reset_machine: bool,
    pub reload: bool,
    pub save: bool,
}

#[derive(Default)]
pub struct Nt4Panel {
    remote: Remote<Nt4Data>,
    pub draft: Nt4Settings,
    /// a new administrator password waiting for Save; kept in memory only.
    new_password: Option<String>,
    selected: Option<usize>,
    form: Option<(Ask, Form)>,
}

impl Nt4Panel {
    pub fn poll(&mut self) {
        if self.remote.poll() {
            if let Some(data) = &self.remote.data {
                self.draft = data.settings.clone();
            }
            self.new_password = None;
            self.selected = None;
        }
    }

    /// the machines in the settings, for the remote commands window.
    pub fn machines(&self) -> &[String] {
        self.remote
            .data
            .as_ref()
            .map_or(&[], |d| &d.settings.machines)
    }

    pub fn dirty(&self) -> bool {
        self.new_password.is_some()
            || self
                .remote
                .data
                .as_ref()
                .is_some_and(|d| d.settings != self.draft)
    }

    fn password_set(&self) -> bool {
        self.new_password.is_some()
            || self
                .remote
                .data
                .as_ref()
                .is_some_and(|d| d.admin_password_set)
    }

    fn reload(&mut self, services: &Services, host: &str) {
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| load(gateway, &target, ask));
        self.remote
            .track(host, job, "loading...", "loaded the settings", true);
    }

    /// set a machine's account back to its first password, then reload.
    fn reset(&mut self, services: &Services, host: &str, name: &str) {
        let (target, name) = (host.to_string(), name.to_string());
        let job = services.spawn(host, move |gateway, ask| {
            gateway.reset_machine(&target, &name, ask)?;
            load(gateway, &target, ask)
        });
        self.remote.track(
            host,
            job,
            "resetting...",
            "reset the machine account",
            false,
        );
    }

    fn save(&mut self, services: &Services, host: &str, apply: bool) {
        let (target, draft) = (host.to_string(), self.draft.clone());
        let updates: Vec<(String, String)> = self
            .new_password
            .iter()
            .map(|p| (ADMIN_PASSWORD.to_string(), p.clone()))
            .collect();
        let modules = if apply {
            vec![MODULE.to_string()]
        } else {
            vec![]
        };
        let job = services.spawn(host, move |gateway, ask| {
            let edit = |settings: &mut lanlord_model::settings::Settings| {
                draft.apply_to(settings);
                Ok(())
            };
            update_settings(gateway, &target, &updates, edit, &modules, ask)?;
            load(gateway, &target, ask)
        });
        let (doing, done) = if apply {
            ("saving and applying...", "saved and applied")
        } else {
            ("saving...", "saved")
        };
        self.remote.track(host, job, doing, done, false);
    }

    /// what may run on `host` now: nothing else running, no box open, settings that pass the
    /// checks before a save, and a picked machine from `host`'s own settings.
    pub fn gates(&self, services: &Services, host: &str) -> Gates {
        let idle = !services.busy(host) && !self.remote.busy();
        let free = self.form.is_none();
        let picked = self.picked().is_some() && self.remote.belongs_to(host);
        let sound = self.draft.validate(self.password_set()).is_empty();
        Gates {
            set_password: idle && free,
            add_machine: free,
            remove_machine: idle && picked,
            reset_machine: idle && free && picked,
            reload: idle,
            save: idle && sound && self.remote.belongs_to(host),
        }
    }

    /// the panel for `host`; loads the settings when it shows another host's.
    pub fn view(&mut self, services: &Services, host: Option<&str>) -> Vec<Node<Msg>> {
        if let Some(host) = host {
            if self.remote.wants_load(host, self.dirty()) {
                self.reload(services, host);
            }
        }
        self.preview(services, host)
    }

    /// the panel as it is, without asking the server for anything: for measuring a page that is
    /// not open.
    pub fn preview(&self, services: &Services, host: Option<&str>) -> Vec<Node<Msg>> {
        let Some(host) = host else {
            return vec![Node::Text(
                "No host yet. Add one in the Hosts window first.".into(),
            )];
        };
        let gates = self.gates(services, host);
        let errors = self.draft.validate(self.password_set());
        let state = match (self.new_password.is_some(), self.password_set()) {
            (true, _) => "password: new value on Save",
            (false, true) => "password: set",
            (false, false) => "password: not set",
        };
        let mut domain = vec![Node::Labelled {
            label: "Domain name".into(),
            body: vec![Node::Input {
                value: self.draft.name.clone(),
                size: Size::Points(170.0),
                hint: String::new(),
                focus: false,
                edit: Box::new(Msg::Name),
                enter: None,
            }],
        }];
        domain.extend(view::field_errors(&errors, "nt4-domain.name"));
        domain.push(Node::Labelled {
            label: "Server name".into(),
            body: vec![
                Node::Input {
                    value: self.draft.netbios_name.clone(),
                    size: Size::Points(170.0),
                    hint: String::new(),
                    focus: false,
                    edit: Box::new(Msg::ServerName),
                    enter: None,
                },
                Node::Text("blank: the gateway's hostname".into()),
            ],
        });
        domain.extend(view::field_errors(&errors, "nt4-domain.netbios_name"));
        domain.push(Node::Labelled {
            label: "Mode".into(),
            body: vec![Node::Choice {
                id: "nt4-mode",
                current: self.draft.mode.clone(),
                width: 120.0,
                options: MODES
                    .iter()
                    .enumerate()
                    .map(|(i, m)| (i, (*m).to_string()))
                    .collect(),
                pick: Msg::Mode,
            }],
        });
        domain.extend(view::field_errors(&errors, "nt4-domain.mode"));
        domain.push(Node::Labelled {
            label: "Options".into(),
            body: vec![
                Node::Check {
                    label: "Roaming profiles".into(),
                    checked: self.draft.roaming_profiles,
                    enabled: true,
                    toggle: Msg::ToggleRoaming,
                },
                Node::Gap(8.0),
                Node::Check {
                    label: "Software share (S:)".into(),
                    checked: self.draft.software,
                    enabled: true,
                    toggle: Msg::ToggleSoftware,
                },
            ],
        });
        domain.push(Node::Labelled {
            label: "Administrator".into(),
            body: vec![
                Node::Text(state.into()),
                Node::Button(Button::new(
                    "Set password...",
                    gates.set_password,
                    Msg::SetPassword,
                )),
            ],
        });
        domain.extend(view::field_errors(&errors, "nt4-domain.admin_password"));
        vec![
            Node::Group {
                title: "Domain".into(),
                body: domain,
            },
            Node::Gap(4.0),
            Node::Group {
                title: "Machines to prepare accounts for".into(),
                body: vec![
                    Node::Table(Table {
                        id: "nt4-machines",
                        headings: vec!["Machine"],
                        widths: vec![200.0],
                        rows: self
                            .draft
                            .machines
                            .iter()
                            .map(|m| vec![m.clone()])
                            .collect(),
                        selected: self.selected,
                        bad: view::bad_rows(
                            &errors,
                            "nt4-domain.machines",
                            self.draft.machines.len(),
                        ),
                        height: 80.0,
                        select: Msg::Pick,
                        steady: false,
                    }),
                    Node::Toolbar(vec![
                        Button::new("Add machine...", gates.add_machine, Msg::AddMachine),
                        Button::new("Remove machine", gates.remove_machine, Msg::RemoveMachine),
                        Button::new("Reset account...", gates.reset_machine, Msg::ResetMachine),
                    ]),
                ],
            },
            Node::Gap(6.0),
            Node::Buttons(vec![
                Button::new("Reload settings", gates.reload, Msg::Reload),
                Button::new("Save & apply", gates.save, Msg::SaveAndApply),
                Button::new("Save settings", gates.save, Msg::Save),
            ]),
            Node::Gap(4.0),
            view::status(&self.remote),
        ]
    }

    fn picked(&self) -> Option<usize> {
        self.selected.filter(|i| *i < self.draft.machines.len())
    }

    /// act on a click or keystroke for `host`.
    pub fn update(&mut self, msg: Msg, services: &Services, host: &str) {
        // the same rules as the buttons: a front end may send any message
        let gates = self.gates(services, host);
        let allowed = match msg {
            Msg::SetPassword => gates.set_password,
            Msg::AddMachine => gates.add_machine,
            Msg::RemoveMachine => gates.remove_machine,
            Msg::ResetMachine => gates.reset_machine,
            Msg::Reload => gates.reload,
            Msg::SaveAndApply | Msg::Save => gates.save,
            _ => true,
        };
        if !allowed {
            return;
        }
        match msg {
            Msg::Name(text) => self.draft.name = text,
            Msg::ServerName(text) => self.draft.netbios_name = text,
            Msg::Mode(i) => {
                if let Some(mode) = MODES.get(i) {
                    self.draft.mode = (*mode).to_string();
                }
            }
            Msg::ToggleRoaming => self.draft.roaming_profiles = !self.draft.roaming_profiles,
            Msg::ToggleSoftware => self.draft.software = !self.draft.software,
            Msg::SetPassword => {
                self.form = Some((
                    Ask::AdminPassword,
                    Form::new(
                        "nt4-admin",
                        "Administrator password",
                        "Stored encrypted and never shown again; Show lets you copy it first.",
                    )
                    .secret("Password"),
                ));
            }
            Msg::Pick(i) => self.selected = Some(i),
            Msg::AddMachine => {
                self.form = Some((
                    Ask::Machine,
                    Form::new("nt4-machine", "Add a machine", "").text("Name", ""),
                ));
            }
            Msg::RemoveMachine => {
                if let Some(i) = self.picked() {
                    self.draft.machines.remove(i);
                    self.selected = None;
                }
            }
            Msg::ResetMachine => {
                if let Some(name) = self.picked().map(|i| self.draft.machines[i].clone()) {
                    let text = format!(
                        "Set the account of {name} back to its first password?\nThe machine must be taken out of the domain (made a member of a workgroup) and joined again, with \"Create a Computer Account\" unticked. You may be asked for the sudo password."
                    );
                    self.form = Some((
                        Ask::Reset(name),
                        Form::new("nt4-reset", "Reset machine account", &text),
                    ));
                }
            }
            Msg::Reload => self.reload(services, host),
            Msg::SaveAndApply => self.save(services, host, true),
            Msg::Save => self.save(services, host, false),
        }
    }

    /// the box for a machine name or the administrator password.
    pub fn dialog(&mut self) -> Option<Dialog<FormMsg>> {
        self.form.as_mut().map(|(_, form)| form.view())
    }

    /// act on a message from the box for `host`.
    pub fn update_dialog(&mut self, msg: FormMsg, services: &Services, host: Option<&str>) {
        let Some((ask, mut form)) = self.form.take() else {
            return;
        };
        match form.update(msg) {
            None => self.form = Some((ask, form)),
            Some(FormAnswer::Cancel) => {}
            Some(FormAnswer::Ok) => match ask {
                Ask::AdminPassword => {
                    self.new_password = Some(form.raw(0).to_string()).filter(|p| !p.is_empty());
                }
                Ask::Reset(name) => {
                    if let Some(host) = host {
                        self.reset(services, host, &name);
                    }
                }
                Ask::Machine => {
                    let name = form.value(0).to_string();
                    if !valid_machine(&name) {
                        form.error =
                            Some("a letter, then up to 14 letters, digits or dashes".into());
                        self.form = Some((Ask::Machine, form));
                    } else if !self.draft.machines.contains(&name) {
                        self.draft.machines.push(name);
                    }
                }
            },
        }
    }

    /// the open box, for tests that fill it in.
    pub fn form_mut(&mut self) -> Option<&mut Form> {
        self.form.as_mut().map(|(_, form)| form)
    }

    pub fn select(&mut self, row: usize) {
        self.selected = Some(row);
    }

    pub fn belongs_to(&self, host: &str) -> bool {
        self.remote.belongs_to(host)
    }

    pub fn has_new_password(&self) -> bool {
        self.new_password.is_some()
    }

    pub fn note(&self) -> Option<&(bool, String)> {
        self.remote.note.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::gateway::Fake;
    use crate::wake::Wake;

    #[test]
    fn broken_settings_are_not_saved_and_an_open_box_is_not_replaced() {
        let fake = Arc::new(Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
        let mut panel = Nt4Panel::default();
        // nothing loaded: the draft is not this host's and fails its checks
        panel.draft.name = "not a valid domain name!".into();
        assert!(!panel.gates(&services, "a@b").save);
        panel.update(Msg::Save, &services, "a@b");
        panel.update(Msg::SaveAndApply, &services, "a@b");
        assert!(fake.calls().is_empty());
        panel.update(Msg::AddMachine, &services, "a@b");
        panel.update(Msg::SetPassword, &services, "a@b");
        assert_eq!(
            panel.form_mut().map(|f| f.fields[0].label.clone()),
            Some("Name".into()),
            "the machine box stays"
        );
    }
}
