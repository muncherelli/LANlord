//! the logon scripts tab: the scripts every domain logon runs (`logon.bat` for everyone and
//! `<login>.bat` for one login), edited here instead of in the netlogon share by hand.

use anyhow::Result;
use eframe::egui::{self, CollapsingHeader, ScrollArea, TextEdit, Ui};
use lanlord_app::ask::AskPassword;
use lanlord_model::logon::{self, valid_script_name};

use crate::controls;
use crate::form::{Form, FormAnswer};
use crate::remote::{Remote, ShowNote};
use crate::theme::{self, WHITE};
use crate::widgets::Table;
use lanlord_app::gateway::Gateway;
use lanlord_app::services::Services;

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

    pub fn show(&mut self, ui: &mut Ui, services: &Services, host: Option<&str>) {
        let Some(host) = host else {
            ui.label("No host yet. Add one in the Hosts window first.");
            return;
        };
        if let Some(name) = self.want.take() {
            self.selected = name;
            self.reload(services, host);
        } else if self.remote.wants_load(host, self.dirty()) {
            self.reload(services, host);
        }
        let idle = !services.busy(host) && !self.remote.busy();
        ui.label(
            "Every domain logon runs these after S: is mapped: logon.bat for everyone, then <login>.bat for one login.",
        );
        ui.add_space(4.0);
        controls::group(ui, "Scripts", |ui| self.list(ui, services, host, idle));
        ui.add_space(4.0);
        let caption = format!("{}.bat", self.selected);
        controls::group(ui, &caption, |ui| self.editor(ui, services, host, idle));
        ui.add_space(4.0);
        self.generated(ui);
        ui.add_space(4.0);
        self.remote.show_note(ui);
    }

    fn list(&mut self, ui: &mut Ui, services: &Services, host: &str, idle: bool) {
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
        let table = Table {
            id: "logon-scripts",
            headings: &["Script", "Runs for"],
            widths: &[160.0, 200.0],
            rows: &rows,
            selected: names.iter().position(|n| *n == self.selected),
            bad: &[],
            height: 80.0,
        };
        if let Some(i) = table.show(ui) {
            if self.dirty() {
                self.remote.say(
                    true,
                    "save or revert the open script before opening another",
                );
            } else if names[i] != self.selected {
                self.selected.clone_from(&names[i]);
                self.reload(services, host);
            }
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let free = idle && !self.dirty() && self.form.is_none();
            if theme::action(ui, "New script...", free).clicked() {
                self.form = Some(
                    Form::new(
                        "logon-new",
                        "New logon script",
                        "The login it runs for, or logon for everyone.",
                    )
                    .text("Name", ""),
                );
            }
            if theme::action(ui, "Reload scripts", idle && !self.dirty()).clicked() {
                self.reload(services, host);
            }
        });
    }

    fn editor(&mut self, ui: &mut Ui, services: &Services, host: &str, idle: bool) {
        theme::sunken(ui, WHITE, |ui| {
            ScrollArea::vertical()
                .id_salt("logon-editor")
                .show(ui, |ui| {
                    ui.add_sized(
                        [ui.available_width(), 130.0],
                        TextEdit::multiline(&mut self.draft)
                            .font(egui::TextStyle::Monospace)
                            .frame(egui::Frame::NONE)
                            .desired_width(f32::INFINITY),
                    );
                });
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let changed = idle && self.dirty();
            if theme::action(ui, "Save script", changed).clicked() {
                self.save(services, host);
            }
            if theme::action(ui, "Revert", changed).clicked() {
                self.draft.clone_from(&self.base);
            }
        });
    }

    fn generated(&self, ui: &mut Ui) {
        CollapsingHeader::new("lanlord.bat (generated, read-only)")
            .id_salt("logon-generated")
            .show(ui, |ui| {
                match self.remote.data.as_ref().and_then(|d| d.generated.as_ref()) {
                    Some(text) => {
                        ui.add(
                            TextEdit::multiline(&mut text.as_str())
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY),
                        );
                    }
                    None => {
                        ui.label("Not written yet; apply the nt4-domain module.");
                    }
                }
            });
    }

    /// the box that names a new script.
    pub fn dialog(&mut self, ctx: &egui::Context) {
        let Some(mut form) = self.form.take() else {
            return;
        };
        match form.show(ctx) {
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

    #[cfg(test)]
    pub fn form_mut(&mut self) -> Option<&mut Form> {
        self.form.as_mut()
    }

    #[cfg(test)]
    pub fn belongs_to(&self, host: &str) -> bool {
        self.remote.belongs_to(host)
    }

    #[cfg(test)]
    pub fn note(&self) -> Option<&(bool, String)> {
        self.remote.note.as_ref()
    }

    #[cfg(test)]
    pub fn selected(&self) -> &str {
        &self.selected
    }

    #[cfg(test)]
    pub fn draft_mut(&mut self) -> &mut String {
        &mut self.draft
    }

    #[cfg(test)]
    pub fn entries_for_test(&self) -> Vec<String> {
        self.entries()
    }
}
