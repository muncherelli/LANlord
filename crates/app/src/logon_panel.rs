//! the logon scripts tab: the scripts every domain logon runs (`logon.bat` for everyone and
//! `<login>.bat` for one login), edited here instead of in the netlogon share by hand.

use crate::ask::AskPassword;
use anyhow::Result;
use lanlord_model::logon::{self, valid_script_name};

use crate::form::{Form, FormAnswer, FormMsg};
use crate::gateway::Gateway;
use crate::remote::Remote;
use crate::services::Services;
use crate::view::{self, Button, Dialog, Node, Table};

pub struct LogonData {
    /// the scripts on the gateway.
    pub names: Vec<String>,
    /// the generated lanlord.bat, for reference.
    pub generated: Option<String>,
    /// the script being edited, `None` when it is not on the gateway yet.
    pub script: Option<String>,
}

fn load(gateway: &dyn Gateway, host: &str, name: &str, ask: AskPassword) -> Result<LogonData> {
    Ok(LogonData {
        names: gateway.logon_scripts(host, ask)?,
        generated: gateway.read_generated_logon(host, ask)?,
        script: gateway.read_logon(host, name, ask)?,
    })
}

/// what a click or a keystroke in the panel means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    Pick(usize),
    New,
    Reload,
    /// the editor now holds this text.
    Edit(String),
    Save,
    Revert,
}

pub struct LogonPanel {
    remote: Remote<LogonData>,
    /// the script in the editor.
    selected: String,
    /// the text in the editor, and what it held when it was loaded.
    draft: String,
    base: String,
    /// a script that was named here and is not on the gateway yet.
    extra: Option<String>,
    /// a script to open on the next frame, chosen in the dialog.
    want: Option<String>,
    form: Option<Form>,
}

impl Default for LogonPanel {
    fn default() -> Self {
        Self {
            remote: Remote::default(),
            selected: logon::COMMON.into(),
            draft: String::new(),
            base: String::new(),
            extra: None,
            want: None,
            form: None,
        }
    }
}

impl LogonPanel {
    pub fn poll(&mut self) {
        if self.remote.poll() {
            if let Some(data) = &self.remote.data {
                self.base = data.script.clone().unwrap_or_default();
                self.draft.clone_from(&self.base);
                if self.extra.as_ref().is_some_and(|n| data.names.contains(n)) {
                    self.extra = None;
                }
            }
        }
    }

    pub fn dirty(&self) -> bool {
        self.remote.data.is_some() && self.draft != self.base
    }

    /// `logon` first, then every script on the gateway and the one just named here.
    fn entries(&self) -> Vec<String> {
        let mut names: Vec<String> = vec![logon::COMMON.into()];
        let known = self.remote.data.iter().flat_map(|d| d.names.iter());
        for name in known.chain(self.extra.iter()) {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        names[1..].sort();
        names
    }

    fn reload(&mut self, services: &Services, host: &str) {
        let (target, name) = (host.to_string(), self.selected.clone());
        let job = services.spawn(host, move |gateway, ask| load(gateway, &target, &name, ask));
        self.remote
            .track(host, job, "loading...", "loaded the logon scripts", true);
    }

    fn save(&mut self, services: &Services, host: &str) {
        let (target, name, text) = (host.to_string(), self.selected.clone(), self.draft.clone());
        let job = services.spawn(host, move |gateway, ask| {
            gateway.write_logon(&target, &name, &text, ask)?;
            load(gateway, &target, &name, ask)
        });
        self.remote.track(host, job, "saving...", "saved", false);
    }

    /// a script may be opened or named: nothing runs, no unsaved edits, no box open.
    pub fn can_open(&self, services: &Services, host: &str) -> bool {
        !services.busy(host) && !self.remote.busy() && !self.dirty() && self.form.is_none()
    }

    /// the open script may be saved or reverted: nothing runs and it was edited.
    pub fn can_save(&self, services: &Services, host: &str) -> bool {
        !services.busy(host) && !self.remote.busy() && self.dirty()
    }

    /// the panel for `host`; loads the scripts when it shows another host's.
    pub fn view(&mut self, services: &Services, host: Option<&str>) -> Vec<Node<Msg>> {
        let Some(host) = host else {
            return vec![Node::Text(
                "No host yet. Add one in the Hosts window first.".into(),
            )];
        };
        if let Some(name) = self.want.take() {
            self.selected = name;
            self.reload(services, host);
        } else if self.remote.wants_load(host, self.dirty()) {
            self.reload(services, host);
        }
        let idle = !services.busy(host) && !self.remote.busy();
        let names = self.entries();
        let on_gateway = |name: &String| {
            self.remote
                .data
                .as_ref()
                .is_some_and(|d| d.names.contains(name))
        };
        let rows: Vec<Vec<String>> = names
            .iter()
            .map(|name| {
                let who = if name == logon::COMMON {
                    "everyone".to_string()
                } else {
                    name.clone()
                };
                let state = if on_gateway(name) {
                    ""
                } else {
                    " (not saved yet)"
                };
                vec![format!("{name}.bat"), format!("{who}{state}")]
            })
            .collect();
        let (free, changed) = (self.can_open(services, host), self.can_save(services, host));
        let generated = match self.remote.data.as_ref().and_then(|d| d.generated.as_ref()) {
            Some(text) => Node::Code(text.clone()),
            None => Node::Text("Not written yet; apply the nt4-domain module.".into()),
        };
        vec![
            Node::Text(
                "Every domain logon runs these after S: is mapped: logon.bat for everyone, then <login>.bat for one login.".into(),
            ),
            Node::Gap(4.0),
            Node::Group {
                title: "Scripts".into(),
                body: vec![
                    Node::Table(Table {
                        id: "logon-scripts",
                        headings: vec!["Script", "Runs for"],
                        widths: vec![160.0, 200.0],
                        rows,
                        selected: names.iter().position(|n| *n == self.selected),
                        bad: Vec::new(),
                        height: 80.0,
                        select: Msg::Pick,
                    }),
                    Node::Gap(4.0),
                    Node::Toolbar(vec![
                        Button::new("New script...", free, Msg::New),
                        Button::new("Reload scripts", idle && !self.dirty(), Msg::Reload),
                    ]),
                ],
            },
            Node::Gap(4.0),
            Node::Group {
                title: format!("{}.bat", self.selected),
                body: vec![
                    Node::TextArea {
                        id: "logon-editor",
                        value: self.draft.clone(),
                        height: 130.0,
                        hint: String::new(),
                        edit: Msg::Edit,
                    },
                    Node::Gap(4.0),
                    Node::Toolbar(vec![
                        Button::new("Save script", changed, Msg::Save),
                        Button::new("Revert", changed, Msg::Revert),
                    ]),
                ],
            },
            Node::Gap(4.0),
            Node::Fold {
                id: "logon-generated",
                title: "lanlord.bat (generated, read-only)".into(),
                body: vec![generated],
            },
            Node::Gap(4.0),
            view::status(&self.remote),
        ]
    }

    /// act on a click or keystroke for `host`.
    pub fn update(&mut self, msg: Msg, services: &Services, host: &str) {
        // the same rules as the buttons: a front end may send any message
        let reload = !services.busy(host) && !self.remote.busy() && !self.dirty();
        let allowed = match msg {
            Msg::New => self.can_open(services, host),
            Msg::Reload => reload,
            Msg::Save | Msg::Revert => self.can_save(services, host),
            Msg::Pick(_) | Msg::Edit(_) => true,
        };
        if !allowed {
            return;
        }
        match msg {
            Msg::Pick(i) => {
                let names = self.entries();
                if self.dirty() {
                    self.remote.say(
                        true,
                        "save or revert the open script before opening another",
                    );
                } else if names.get(i).is_some_and(|n| *n != self.selected) {
                    self.selected.clone_from(&names[i]);
                    self.reload(services, host);
                }
            }
            Msg::New => {
                self.form = Some(
                    Form::new(
                        "logon-new",
                        "New logon script",
                        "The login it runs for, or logon for everyone.",
                    )
                    .text("Name", ""),
                );
            }
            Msg::Reload => self.reload(services, host),
            Msg::Edit(text) => self.draft = text,
            Msg::Save => self.save(services, host),
            Msg::Revert => self.draft.clone_from(&self.base),
        }
    }

    /// the box that names a new script, while it is open.
    pub fn dialog(&mut self) -> Option<Dialog<FormMsg>> {
        self.form.as_mut().map(Form::view)
    }

    /// act on a message from the box.
    pub fn update_dialog(&mut self, msg: FormMsg) {
        let Some(mut form) = self.form.take() else {
            return;
        };
        match form.update(msg) {
            None => self.form = Some(form),
            Some(FormAnswer::Cancel) => {}
            Some(FormAnswer::Ok) => {
                let name = form.value(0).to_string();
                if valid_script_name(&name) {
                    let known = self
                        .remote
                        .data
                        .as_ref()
                        .is_some_and(|d| d.names.contains(&name));
                    if !known && name != logon::COMMON {
                        self.extra = Some(name.clone());
                    }
                    self.want = Some(name);
                } else {
                    form.error = Some(
                        "a lowercase login name (letters, digits, - _ .), or logon; lanlord is reserved"
                            .into(),
                    );
                    self.form = Some(form);
                }
            }
        }
    }

    /// the open box, for tests that fill it in.
    pub fn form_mut(&mut self) -> Option<&mut Form> {
        self.form.as_mut()
    }

    pub fn belongs_to(&self, host: &str) -> bool {
        self.remote.belongs_to(host)
    }

    pub fn note(&self) -> Option<&(bool, String)> {
        self.remote.note.as_ref()
    }

    pub fn selected(&self) -> &str {
        &self.selected
    }

    pub fn draft_mut(&mut self) -> &mut String {
        &mut self.draft
    }

    pub fn entries_for_test(&self) -> Vec<String> {
        self.entries()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::gateway::Fake;
    use crate::wake::Wake;

    #[test]
    fn nothing_unedited_is_saved_and_an_open_box_is_not_replaced() {
        let fake = Arc::new(Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
        let mut panel = LogonPanel::default();
        panel.update(Msg::Save, &services, "a@b");
        assert!(fake.calls().is_empty(), "nothing was edited");
        panel.update(Msg::New, &services, "a@b");
        panel.form_mut().unwrap().set(0, "dana");
        panel.update(Msg::New, &services, "a@b");
        assert_eq!(panel.form_mut().unwrap().value(0), "dana", "the box stays");
    }
}
