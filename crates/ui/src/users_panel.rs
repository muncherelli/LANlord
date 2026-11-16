//! the logins of the nt4 domain: names and the domain admin flag in the plain
//! settings, passwords in the encrypted ones. every change runs when you
//! click, in the background, and applies the module.

use eframe::egui::{self, Ui};
use lanlord_core::bootstrap::AskPassword;
use lanlord_core::users::{self, Change, User};

use crate::form::{Form, FormAnswer, ShowForm};
use crate::remote::{Remote, ShowNote};
use crate::theme;
use crate::widgets::Table;
use lanlord_app::gateway::Gateway;
use lanlord_app::services::Services;

pub struct UsersData {
    pub users: Vec<User>,
    /// the paths of the secrets that have a value; never the values.
    pub secrets_set: Vec<String>,
}

/// the logins stored on `host`.
pub fn load(gateway: &dyn Gateway, host: &str, ask: AskPassword) -> anyhow::Result<UsersData> {
    let document = gateway.load_document(host, ask)?;
    Ok(UsersData {
        users: users::list(document.plain.as_bytes())?,
        secrets_set: document.secrets_set,
    })
}

enum Ask {
    Add,
    Password(String),
    Delete(String),
}

#[derive(Default)]
pub struct UsersPanel {
    remote: Remote<UsersData>,
    selected: Option<usize>,
    form: Option<(Ask, Form)>,
}

impl UsersPanel {
    pub fn poll(&mut self) {
        if self.remote.poll() {
            let count = self.remote.data.as_ref().map_or(0, |d| d.users.len());
            self.selected = self.selected.filter(|i| *i < count);
        }
    }

    fn users(&self) -> &[User] {
        self.remote.data.as_ref().map_or(&[], |d| &d.users)
    }

    fn current(&self) -> Option<&User> {
        self.users().get(self.selected?)
    }

    fn reload(&mut self, services: &Services, host: &str) {
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| load(gateway, &target, ask));
        self.remote
            .track(host, job, "loading...", "loaded the logins", true);
    }

    /// make `changes` on the gateway, then show the logins as they are now.
    fn change(&mut self, services: &Services, host: &str, changes: Vec<Change>, done: &str) {
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| {
            gateway.change_users(&target, &changes, ask)?;
            load(gateway, &target, ask)
        });
        self.remote
            .track(host, job, "changing the logins...", done, false);
    }

    pub fn show(&mut self, ui: &mut Ui, services: &Services, host: Option<&str>) {
        let Some(host) = host else {
            ui.label("No host yet. Add one in the Hosts window first.");
            return;
        };
        if self.remote.wants_load(host, false) {
            self.reload(services, host);
        }
        let idle = !services.busy(host) && !self.remote.busy();
        let secrets = self.remote.data.as_ref().map(|d| &d.secrets_set);
        let rows: Vec<Vec<String>> = self
            .users()
            .iter()
            .map(|user| {
                let has = secrets.is_some_and(|s| s.contains(&users::secret_path(&user.name)));
                vec![
                    user.name.clone(),
                    if user.admin { "yes" } else { "no" }.into(),
                    if user.software_admin { "yes" } else { "no" }.into(),
                    if has { "set" } else { "not set" }.into(),
                ]
            })
            .collect();
        let table = Table {
            id: "users-table",
            headings: &["Login", "Domain admin", "Software admin", "Password"],
            widths: &[150.0, 100.0, 110.0],
            rows: &rows,
            selected: self.selected,
            bad: &[],
            height: 200.0,
        };
        if let Some(i) = table.show(ui) {
            self.selected = Some(i);
        }
        if rows.is_empty() && self.remote.note.is_none() {
            ui.label("No logins yet.");
        }
        ui.add_space(6.0);
        self.buttons(ui, services, host, idle);
        ui.add_space(6.0);
        self.remote.show_note(ui);
    }

    fn buttons(&mut self, ui: &mut Ui, services: &Services, host: &str, idle: bool) {
        let picked = self.current().cloned();
        let has = picked.is_some();
        ui.horizontal(|ui| {
            if theme::action(ui, "Add login...", idle && self.form.is_none()).clicked() {
                self.form = Some((
                    Ask::Add,
                    Form::new("user-add", "Add a login", "")
                        .text("Name", "")
                        .secret("Password")
                        .check("Domain admin", false)
                        .check(
                            "Software admin (for non-admins; admins write already)",
                            false,
                        ),
                ));
            }
            if theme::action(ui, "Password...", idle && has && self.form.is_none()).clicked() {
                if let Some(user) = &picked {
                    let prompt = format!("New password for {}:", user.name);
                    self.form = Some((
                        Ask::Password(user.name.clone()),
                        Form::new("user-password", "Change password", &prompt).secret("Password"),
                    ));
                }
            }
            if theme::action(ui, "Delete...", idle && has && self.form.is_none()).clicked() {
                if let Some(user) = &picked {
                    let prompt =
                        format!("Delete {}? The account goes at the next apply.", user.name);
                    self.form = Some((
                        Ask::Delete(user.name.clone()),
                        Form::new("user-delete", "Delete a login", &prompt),
                    ));
                }
            }
        });
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let admin = picked.as_ref().is_some_and(|u| u.admin);
            let label = if admin {
                "Remove domain admin"
            } else {
                "Make domain admin"
            };
            if theme::action(ui, label, idle && has).clicked() {
                if let Some(user) = &picked {
                    let change = Change::SetAdmin {
                        name: user.name.clone(),
                        admin: !admin,
                    };
                    self.change(services, host, vec![change], "domain admin changed");
                }
            }
            let software = picked.as_ref().is_some_and(|u| u.software_admin);
            let label = if software {
                "Remove software admin"
            } else {
                "Make software admin"
            };
            // domain admins already write to the software share, so the flag only matters for others
            let others = picked.as_ref().is_some_and(|u| !u.admin);
            if theme::action(ui, label, idle && has && others).clicked() {
                if let Some(user) = &picked {
                    let change = Change::SetSoftwareAdmin {
                        name: user.name.clone(),
                        software_admin: !software,
                    };
                    self.change(services, host, vec![change], "software admin changed");
                }
            }
            if theme::action(ui, "Reload logins", idle).clicked() {
                self.reload(services, host);
            }
        });
    }

    /// the box for adding, changing or deleting a login, drawn on top of everything.
    pub fn dialog(&mut self, ctx: &egui::Context, services: &Services, host: Option<&str>) {
        let Some((ask, mut form)) = self.form.take() else {
            return;
        };
        match (form.show(ctx), host) {
            (None, _) => self.form = Some((ask, form)),
            (Some(FormAnswer::Cancel), _) | (_, None) => {}
            (Some(FormAnswer::Ok), Some(host)) => match self.answer(&ask, &form) {
                Ok((changes, done)) => self.change(services, host, changes, &done),
                Err(error) => {
                    form.error = Some(error);
                    self.form = Some((ask, form));
                }
            },
        }
    }

    /// the changes an answered form asks for, or why it cannot.
    fn answer(&self, ask: &Ask, form: &Form) -> Result<(Vec<Change>, String), String> {
        match ask {
            Ask::Add => {
                let name = form.value(0).to_string();
                if !users::valid_name(&name) {
                    return Err(
                        "names are lowercase letters, digits, _ . - and start with a letter".into(),
                    );
                }
                if self.users().iter().any(|u| u.name == name) {
                    return Err(format!("{name} already exists; use Password... instead"));
                }
                let mut changes = vec![set_password(&name, form.raw(1))?];
                if form.checked(0) {
                    changes.push(Change::SetAdmin {
                        name: name.clone(),
                        admin: true,
                    });
                }
                if form.checked(1) {
                    changes.push(Change::SetSoftwareAdmin {
                        name,
                        software_admin: true,
                    });
                }
                Ok((changes, "login added".into()))
            }
            Ask::Password(name) => Ok((
                vec![set_password(name, form.raw(0))?],
                "password changed".into(),
            )),
            Ask::Delete(name) => Ok((
                vec![Change::Remove { name: name.clone() }],
                "login deleted".into(),
            )),
        }
    }

    #[cfg(test)]
    pub fn form_mut(&mut self) -> Option<&mut Form> {
        self.form.as_mut().map(|(_, form)| form)
    }

    #[cfg(test)]
    pub fn names(&self) -> Vec<String> {
        self.users().iter().map(|u| u.name.clone()).collect()
    }

    #[cfg(test)]
    pub fn select(&mut self, name: &str) {
        self.selected = self.users().iter().position(|u| u.name == name);
    }

    #[cfg(test)]
    pub fn note(&self) -> Option<&(bool, String)> {
        self.remote.note.as_ref()
    }

    #[cfg(test)]
    pub fn belongs_to(&self, host: &str) -> bool {
        self.remote.belongs_to(host)
    }
}

fn set_password(name: &str, password: &str) -> Result<Change, String> {
    if password.is_empty() {
        return Err("the password is empty; Generate makes one".into());
    }
    Ok(Change::SetPassword {
        name: name.into(),
        password: password.into(),
    })
}
