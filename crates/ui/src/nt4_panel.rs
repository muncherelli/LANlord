//! the nt4 domain's settings tab: domain, server name, machines, roaming
//! profiles and the administrator password (which is only ever set, never shown).

use eframe::egui::{self, Ui};
use lanlord_core::nt4::{valid_machine, Nt4Settings};

use crate::controls;
use crate::form::{Form, FormAnswer};
use crate::remote::{Remote, ShowNote};
use crate::theme;
use crate::widgets::{bad_rows, field_error, labelled_edit, Table};
use lanlord_app::services::{update_settings, Services};

const ADMIN_PASSWORD: &str = "nt4-domain.admin_password";
const MODULE: &str = "nt4-domain";

pub struct Nt4Data {
    pub settings: Nt4Settings,
    pub admin_password_set: bool,
}

fn load(
    gateway: &dyn lanlord_app::gateway::Gateway,
    host: &str,
    ask: lanlord_core::bootstrap::AskPassword,
) -> anyhow::Result<Nt4Data> {
    let document = gateway.load_document(host, ask)?;
    let settings = lanlord_core::settings::Settings::parse(&document.plain)?;
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
            let edit = |settings: &mut lanlord_core::settings::Settings| {
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

    pub fn show(&mut self, ui: &mut Ui, services: &Services, host: Option<&str>) {
        let Some(host) = host else {
            ui.label("No host yet. Add one in the Hosts window first.");
            return;
        };
        if self.remote.wants_load(host, self.dirty()) {
            self.reload(services, host);
        }
        let idle = !services.busy(host) && !self.remote.busy();
        let errors = self.draft.validate(self.password_set());
        controls::group(ui, "Domain", |ui| {
            labelled_edit(ui, "Domain name", &mut self.draft.name, 170.0);
            field_error(ui, &errors, "nt4-domain.name");
            controls::row(ui, "Server name", |ui| {
                controls::edit(ui, &mut self.draft.netbios_name, 170.0);
                ui.label("blank: the gateway's hostname");
            });
            field_error(ui, &errors, "nt4-domain.netbios_name");
            controls::row(ui, "Mode", |ui| {
                let mode = self.draft.mode.clone();
                controls::dropdown(ui, "nt4-mode", &mode, 120.0, |ui| {
                    for mode in ["domain", "workgroup"] {
                        ui.selectable_value(&mut self.draft.mode, mode.to_string(), mode);
                    }
                });
            });
            field_error(ui, &errors, "nt4-domain.mode");
            controls::row(ui, "Options", |ui| {
                controls::check(ui, &mut self.draft.roaming_profiles, "Roaming profiles");
                ui.add_space(8.0);
                controls::check(ui, &mut self.draft.software, "Software share (S:)");
            });
            controls::row(ui, "Administrator", |ui| {
                let state = match (self.new_password.is_some(), self.password_set()) {
                    (true, _) => "password: new value on Save",
                    (false, true) => "password: set",
                    (false, false) => "password: not set",
                };
                ui.label(state);
                if theme::action(ui, "Set password...", idle && self.form.is_none()).clicked() {
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
            });
            field_error(ui, &errors, "nt4-domain.admin_password");
        });
        ui.add_space(4.0);
        controls::group(ui, "Machines to prepare accounts for", |ui| {
            self.machines_list(ui, &errors, idle);
        });
        ui.add_space(6.0);
        let blocked = !errors.is_empty();
        controls::buttons(ui, |ui| {
            if theme::action(ui, "Reload settings", idle).clicked() {
                self.reload(services, host);
            }
            if theme::action(ui, "Save & apply", idle && !blocked).clicked() {
                self.save(services, host, true);
            }
            if theme::action(ui, "Save settings", idle && !blocked).clicked() {
                self.save(services, host, false);
            }
        });
        ui.add_space(4.0);
        self.remote.show_note(ui);
    }

    fn machines_list(
        &mut self,
        ui: &mut Ui,
        errors: &[lanlord_core::settings::ValidationError],
        idle: bool,
    ) {
        let rows: Vec<Vec<String>> = self
            .draft
            .machines
            .iter()
            .map(|m| vec![m.clone()])
            .collect();
        let bad = bad_rows(errors, "nt4-domain.machines", rows.len());
        let table = Table {
            id: "nt4-machines",
            headings: &["Machine"],
            widths: &[200.0],
            rows: &rows,
            selected: self.selected,
            bad: &bad,
            height: 80.0,
        };
        if let Some(i) = table.show(ui) {
            self.selected = Some(i);
        }
        ui.horizontal(|ui| {
            if theme::action(ui, "Add machine...", self.form.is_none()).clicked() {
                self.form = Some((
                    Ask::Machine,
                    Form::new("nt4-machine", "Add a machine", "").text("Name", ""),
                ));
            }
            let picked = self.selected.filter(|i| *i < self.draft.machines.len());
            if theme::action(ui, "Remove machine", idle && picked.is_some()).clicked() {
                if let Some(i) = picked {
                    self.draft.machines.remove(i);
                    self.selected = None;
                }
            }
            if theme::action(ui, "Reset account...", idle && self.form.is_none() && picked.is_some())
                .clicked()
            {
                if let Some(name) = picked.map(|i| self.draft.machines[i].clone()) {
                    let text = format!(
                        "Set the account of {name} back to its first password?\nThe machine must be taken out of the domain (made a member of a workgroup) and joined again, with \"Create a Computer Account\" unticked. You may be asked for the sudo password."
                    );
                    self.form = Some((
                        Ask::Reset(name),
                        Form::new("nt4-reset", "Reset machine account", &text),
                    ));
                }
            }
        });
    }

    /// the box for a machine name or the administrator password.
    pub fn dialog(&mut self, ctx: &egui::Context, services: &Services, host: Option<&str>) {
        let Some((ask, mut form)) = self.form.take() else {
            return;
        };
        match form.show(ctx) {
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

    #[cfg(test)]
    pub fn form_mut(&mut self) -> Option<&mut Form> {
        self.form.as_mut().map(|(_, form)| form)
    }

    #[cfg(test)]
    pub fn select(&mut self, row: usize) {
        self.selected = Some(row);
    }

    #[cfg(test)]
    pub fn belongs_to(&self, host: &str) -> bool {
        self.remote.belongs_to(host)
    }

    #[cfg(test)]
    pub fn has_new_password(&self) -> bool {
        self.new_password.is_some()
    }

    #[cfg(test)]
    pub fn note(&self) -> Option<&(bool, String)> {
        self.remote.note.as_ref()
    }
}
