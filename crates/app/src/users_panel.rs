//! the logins of the nt4 domain: names and the domain admin flag in the plain settings,
//! passwords in the encrypted ones. every change runs when you click, in the background, and
//! applies the module.

use lanlord_core::bootstrap::AskPassword;
use lanlord_core::users::{self, Change, User};

use crate::form::{Form, FormAnswer, FormMsg};
use crate::gateway::Gateway;
use crate::remote::Remote;
use crate::services::Services;
use crate::view::{self, Button, Dialog, Node, Table};

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

/// what a click in the panel means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Msg {
    Pick(usize),
    Add,
    Password,
    Delete,
    ToggleAdmin,
    ToggleSoftwareAdmin,
    Reload,
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

    /// the panel for `host`; starts a load when it shows another host's logins.
    pub fn view(&mut self, services: &Services, host: Option<&str>) -> Vec<Node<Msg>> {
        let Some(host) = host else {
            return vec![Node::Text(
                "No host yet. Add one in the Hosts window first.".into(),
            )];
        };
        if self.remote.wants_load(host, false) {
            self.reload(services, host);
        }
        let idle = !services.busy(host) && !self.remote.busy();
        let free = idle && self.form.is_none();
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
        let empty = rows.is_empty() && self.remote.note.is_none();
        let picked = self.current();
        let has = picked.is_some();
        let admin = picked.is_some_and(|u| u.admin);
        let software = picked.is_some_and(|u| u.software_admin);
        // domain admins already write to the software share, so the flag only matters for others
        let others = picked.is_some_and(|u| !u.admin);
        let mut nodes = vec![Node::Table(Table {
            id: "users-table",
            headings: vec!["Login", "Domain admin", "Software admin", "Password"],
            widths: vec![150.0, 100.0, 110.0],
            rows,
            selected: self.selected,
            bad: Vec::new(),
            height: 200.0,
            select: Msg::Pick,
        })];
        if empty {
            nodes.push(Node::Text("No logins yet.".into()));
        }
        nodes.extend([
            Node::Gap(6.0),
            Node::Toolbar(vec![
                Button::new("Add login...", free, Msg::Add),
                Button::new("Password...", free && has, Msg::Password),
                Button::new("Delete...", free && has, Msg::Delete),
            ]),
            Node::Gap(2.0),
            Node::Toolbar(vec![
                Button::new(
                    if admin {
                        "Remove domain admin"
                    } else {
                        "Make domain admin"
                    },
                    idle && has,
                    Msg::ToggleAdmin,
                ),
                Button::new(
                    if software {
                        "Remove software admin"
                    } else {
                        "Make software admin"
                    },
                    idle && has && others,
                    Msg::ToggleSoftwareAdmin,
                ),
                Button::new("Reload logins", idle, Msg::Reload),
            ]),
            Node::Gap(6.0),
            view::status(&self.remote),
        ]);
        nodes
    }

    /// act on a click in the panel for `host`.
    pub fn update(&mut self, msg: Msg, services: &Services, host: &str) {
        let picked = self.current().cloned();
        match (msg, picked) {
            (Msg::Pick(i), _) => self.selected = Some(i),
            (Msg::Add, _) => {
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
            (Msg::Password, Some(user)) => {
                let prompt = format!("New password for {}:", user.name);
                self.form = Some((
                    Ask::Password(user.name),
                    Form::new("user-password", "Change password", &prompt).secret("Password"),
                ));
            }
            (Msg::Delete, Some(user)) => {
                let prompt = format!("Delete {}? The account goes at the next apply.", user.name);
                self.form = Some((
                    Ask::Delete(user.name),
                    Form::new("user-delete", "Delete a login", &prompt),
                ));
            }
            (Msg::ToggleAdmin, Some(user)) => {
                let change = Change::SetAdmin {
                    name: user.name,
                    admin: !user.admin,
                };
                self.change(services, host, vec![change], "domain admin changed");
            }
            (Msg::ToggleSoftwareAdmin, Some(user)) => {
                let change = Change::SetSoftwareAdmin {
                    name: user.name,
                    software_admin: !user.software_admin,
                };
                self.change(services, host, vec![change], "software admin changed");
            }
            (Msg::Reload, _) => self.reload(services, host),
            (Msg::Password | Msg::Delete | Msg::ToggleAdmin | Msg::ToggleSoftwareAdmin, None) => {}
        }
    }

    /// the box for adding, changing or deleting a login, while one is open.
    pub fn dialog(&mut self) -> Option<Dialog<FormMsg>> {
        self.form.as_mut().map(|(_, form)| form.view())
    }

    /// act on a message from the box for `host`.
    pub fn update_dialog(&mut self, msg: FormMsg, services: &Services, host: Option<&str>) {
        let Some((ask, mut form)) = self.form.take() else {
            return;
        };
        match (form.update(msg), host) {
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

    /// the open box, for tests that fill it in.
    pub fn form_mut(&mut self) -> Option<&mut Form> {
        self.form.as_mut().map(|(_, form)| form)
    }

    pub fn names(&self) -> Vec<String> {
        self.users().iter().map(|u| u.name.clone()).collect()
    }

    pub fn select(&mut self, name: &str) {
        self.selected = self.users().iter().position(|u| u.name == name);
    }

    pub fn note(&self) -> Option<&(bool, String)> {
        self.remote.note.as_ref()
    }

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
