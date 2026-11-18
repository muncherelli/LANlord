//! Users and Permissions: the roles and what each may do in each area, and which roles each
//! user has. someone with only View can read it all and change nothing.

use lanlord_model::access::{Access, Area, Level, Role};

use crate::form::{Form, FormAnswer, FormMsg};
use crate::remote::Remote;
use crate::services::Services;
use crate::view::{self, Button, Dialog, Node, Table};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    PickUser(usize),
    PickRole(usize),
    /// the selected user has or has not this role.
    ToggleRole(usize),
    /// the selected custom role gets a level: `area * 3 + level`.
    Grant(usize),
    AddUser,
    NewRole,
    RemoveRole,
    Save,
    Reload,
}

#[derive(Default)]
pub struct PermissionsWindow {
    remote: Remote<Access>,
    draft: Access,
    user: Option<usize>,
    role: usize,
    form: Option<(Add, Form)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Add {
    User,
    Role,
}

fn encode(area: usize, level: usize) -> usize {
    area * 3 + level
}

impl PermissionsWindow {
    pub fn poll(&mut self) {
        if self.remote.poll() {
            if let Some(access) = &self.remote.data {
                self.draft = access.clone();
            }
        }
    }

    fn dirty(&self) -> bool {
        self.remote.data.as_ref().is_some_and(|d| *d != self.draft)
    }

    fn users(&self) -> Vec<String> {
        self.draft.members.keys().cloned().collect()
    }

    fn roles(&self) -> Vec<Role> {
        self.draft.all_roles()
    }

    fn reload(&mut self, services: &Services, host: &str) {
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| gateway.load_access(&target, ask));
        self.remote
            .track(host, job, "loading...", "loaded the permissions", true);
    }

    fn save(&mut self, services: &Services, host: &str) {
        let (target, access) = (host.to_string(), self.draft.clone());
        let job = services.spawn(host, move |gateway, ask| {
            gateway.save_access(&target, &access, ask)?;
            gateway.load_access(&target, ask)
        });
        self.remote.track(host, job, "saving...", "saved", false);
    }

    fn user_rows(&self) -> Vec<Vec<String>> {
        let users = self.users();
        users
            .iter()
            .map(|u| vec![u.clone(), self.draft.roles_of(u, false).join(", ")])
            .collect()
    }

    fn user_group(&self, writable: bool) -> Node<Msg> {
        let mut body = vec![Node::Table(Table {
            id: "permission-users",
            headings: vec!["User", "Roles"],
            widths: vec![140.0],
            rows: self.user_rows(),
            selected: self.user,
            bad: Vec::new(),
            height: 90.0,
            select: Msg::PickUser,
        })];
        if let Some(user) = self.user.and_then(|i| self.users().get(i).cloned()) {
            let mine = self.draft.roles_of(&user, false);
            for (i, role) in self.roles().iter().enumerate() {
                body.push(Node::Check {
                    label: role.name.clone(),
                    checked: mine.contains(&role.name),
                    enabled: writable,
                    toggle: Msg::ToggleRole(i),
                });
            }
        }
        body.push(Node::Toolbar(vec![Button::new(
            "Add user...",
            writable,
            Msg::AddUser,
        )]));
        Node::Group {
            title: "Users".into(),
            body,
        }
    }

    fn level_choice(&self, area: usize, role: &Role, writable: bool) -> Node<Msg> {
        let a = Area::ALL[area];
        let options = Level::ALL
            .iter()
            .enumerate()
            .map(|(l, level)| (encode(area, l), level.title().to_string()))
            .collect();
        let choice = Node::Choice {
            id: "permission-level",
            current: role.level(a).title().into(),
            width: 90.0,
            options,
            pick: Msg::Grant,
        };
        let editable = writable && !Role::is_builtin(&role.name);
        let cell = if editable {
            choice
        } else {
            Node::Text(role.level(a).title().into())
        };
        Node::Labelled {
            label: a.title().into(),
            body: vec![cell],
        }
    }

    fn role_group(&self, writable: bool) -> Node<Msg> {
        let roles = self.roles();
        let names: Vec<String> = roles.iter().map(|r| r.name.clone()).collect();
        let role = roles.get(self.role).cloned().unwrap_or_default();
        let mut body = vec![Node::List {
            id: "permission-roles",
            items: names,
            selected: Some(self.role),
            min_height: 60.0,
            empty: String::new(),
            select: Msg::PickRole,
        }];
        body.extend((0..Area::ALL.len()).map(|a| self.level_choice(a, &role, writable)));
        body.push(Node::Toolbar(vec![
            Button::new("New role...", writable, Msg::NewRole),
            Button::new(
                "Remove role",
                writable && !Role::is_builtin(&role.name),
                Msg::RemoveRole,
            ),
        ]));
        Node::Group {
            title: "Roles".into(),
            body,
        }
    }

    pub fn view(
        &mut self,
        services: &Services,
        host: Option<&str>,
        writable: bool,
    ) -> Vec<Node<Msg>> {
        let Some(host) = host else {
            return vec![Node::Text("No server yet. Sign in to one first.".into())];
        };
        if self.remote.wants_load(host, self.dirty()) {
            self.reload(services, host);
        }
        let idle = !services.busy(host) && !self.remote.busy();
        // a save or reload in flight replaces the draft when it ends, so nothing is edited meanwhile
        let writable = writable && !self.remote.busy();
        vec![
            Node::Columns(vec![
                (None, vec![self.user_group(writable)]),
                (None, vec![self.role_group(writable)]),
            ]),
            Node::Gap(4.0),
            // single buttons in a row, so each keeps its natural width in the text view
            Node::Row(vec![
                Node::Button(Button::new(
                    "Save",
                    writable && idle && self.dirty(),
                    Msg::Save,
                )),
                Node::Button(Button::new("Reload", idle && !self.dirty(), Msg::Reload)),
            ]),
            view::status(&self.remote),
        ]
    }

    /// act on a click for `host`. nothing changes without Manage: the server refuses it too.
    pub fn update(&mut self, msg: Msg, services: &Services, host: &str, writable: bool) {
        match msg {
            Msg::PickUser(i) => self.user = Some(i),
            Msg::PickRole(i) => self.role = i,
            Msg::Reload if !self.dirty() && !self.remote.busy() => self.reload(services, host),
            _ if !writable || self.remote.busy() => {}
            Msg::Save if self.dirty() => self.save(services, host),
            other => self.edit(other),
        }
    }

    fn edit(&mut self, msg: Msg) {
        match msg {
            Msg::ToggleRole(i) => self.toggle_role(i),
            Msg::Grant(key) => self.grant(key),
            Msg::AddUser => self.form = Some((Add::User, name_form("add-user", "Add user"))),
            Msg::NewRole => self.form = Some((Add::Role, name_form("new-role", "New role"))),
            Msg::RemoveRole => self.remove_role(),
            _ => {}
        }
    }

    fn toggle_role(&mut self, i: usize) {
        let Some(user) = self.user.and_then(|u| self.users().get(u).cloned()) else {
            return;
        };
        let Some(name) = self.roles().get(i).map(|r| r.name.clone()) else {
            return;
        };
        let mut mine = self.draft.roles_of(&user, false);
        match mine.iter().position(|n| *n == name) {
            Some(at) => drop(mine.remove(at)),
            None => mine.push(name),
        }
        self.draft.members.insert(user, mine);
    }

    fn grant(&mut self, key: usize) {
        let (area, level) = (Area::ALL.get(key / 3), Level::ALL.get(key % 3));
        let builtins = Role::builtins().len();
        let (Some(area), Some(level)) = (area, level) else {
            return;
        };
        if let Some(role) = self
            .role
            .checked_sub(builtins)
            .and_then(|i| self.draft.roles.get_mut(i))
        {
            role.grants.insert(*area, *level);
        }
    }

    fn remove_role(&mut self) {
        let Some(at) = self.role.checked_sub(Role::builtins().len()) else {
            return;
        };
        if at < self.draft.roles.len() {
            let gone = self.draft.roles.remove(at).name;
            for names in self.draft.members.values_mut() {
                names.retain(|n| *n != gone);
            }
            self.role = 0;
        }
    }

    pub fn dialog(&mut self) -> Option<Dialog<FormMsg>> {
        self.form.as_mut().map(|(_, form)| form.view())
    }

    pub fn update_dialog(&mut self, msg: FormMsg) {
        let Some((kind, mut form)) = self.form.take() else {
            return;
        };
        match form.update(msg) {
            None => self.form = Some((kind, form)),
            Some(FormAnswer::Cancel) => {}
            Some(FormAnswer::Ok) => {
                let name = form.value(0).to_string();
                match self.add(kind, &name) {
                    Ok(()) => {}
                    Err(problem) => {
                        form.error = Some(problem);
                        self.form = Some((kind, form));
                    }
                }
            }
        }
    }

    fn add(&mut self, kind: Add, name: &str) -> Result<(), String> {
        if name.is_empty() {
            return Err("Type a name.".into());
        }
        if self.remote.busy() {
            return Err("Wait for the save to finish.".into());
        }
        match kind {
            Add::User => {
                self.draft
                    .members
                    .entry(name.to_string())
                    .or_insert_with(|| vec![lanlord_model::access::DEFAULT_ROLE.to_string()]);
            }
            Add::Role => {
                self.draft.roles.push(Role {
                    name: name.to_string(),
                    grants: Default::default(),
                });
                if let Some(problem) = self.draft.problem() {
                    self.draft.roles.pop();
                    return Err(problem);
                }
            }
        }
        Ok(())
    }
}

fn name_form(id: &'static str, title: &str) -> Form {
    Form::new(id, title, "").text("Name", "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanlord_model::access::{ADMINISTRATORS, USERS};

    fn window() -> PermissionsWindow {
        let mut w = PermissionsWindow::default();
        w.draft.members.insert("dana".into(), vec![USERS.into()]);
        w.user = Some(0);
        w
    }

    #[test]
    fn roles_are_toggled_on_the_selected_user() {
        let mut w = window();
        w.toggle_role(0);
        assert_eq!(w.draft.members["dana"], [USERS, ADMINISTRATORS]);
        w.toggle_role(0);
        assert_eq!(w.draft.members["dana"], [USERS]);
    }

    #[test]
    fn only_custom_roles_take_levels_and_removing_one_unassigns_it() {
        let mut w = window();
        assert_eq!(w.add(Add::Role, "Helpdesk"), Ok(()));
        assert!(w.add(Add::Role, ADMINISTRATORS).is_err());
        w.role = 0;
        w.grant(encode(0, 2));
        assert!(w.draft.roles[0].grants.is_empty(), "built-ins are fixed");
        w.role = 3;
        w.grant(encode(5, 2));
        assert_eq!(w.draft.roles[0].grants[&Area::ALL[5]], Level::Manage);
        w.draft
            .members
            .insert("dana".into(), vec!["Helpdesk".into()]);
        w.remove_role();
        assert!(w.draft.roles.is_empty() && w.draft.members["dana"].is_empty());
    }

    fn setup() -> (
        PermissionsWindow,
        Services,
        std::sync::Arc<crate::gateway::Fake>,
    ) {
        let fake = std::sync::Arc::new(crate::gateway::Fake::new());
        let services = Services::new(fake.clone(), crate::wake::Wake::none());
        (PermissionsWindow::default(), services, fake)
    }

    fn settle(window: &mut PermissionsWindow, services: &Services) {
        let start = std::time::Instant::now();
        loop {
            window.poll();
            if !services.busy("gw") && !window.remote.busy() {
                return;
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(5), "stuck");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn without_a_server_the_window_says_so() {
        let (mut window, services, _) = setup();
        assert_eq!(window.view(&services, None, true).len(), 1);
    }

    #[test]
    fn it_loads_once_and_lists_users_and_roles() {
        let (mut window, services, fake) = setup();
        let _ = window.view(&services, Some("gw"), true);
        settle(&mut window, &services);
        window
            .draft
            .members
            .insert("dana".into(), vec![USERS.into()]);
        window.remote.data = Some(window.draft.clone());
        window.update(Msg::PickUser(0), &services, "gw", true);
        window.update(Msg::PickRole(1), &services, "gw", true);
        let nodes = window.view(&services, Some("gw"), true);
        assert_eq!(nodes.len(), 4);
        assert_eq!(
            window.user_rows(),
            [["alice", ADMINISTRATORS], ["ben", USERS], ["dana", USERS]]
        );
        assert_eq!(fake.calls(), ["load-access gw"]);
        window.update(Msg::Reload, &services, "gw", true);
        settle(&mut window, &services);
        assert_eq!(fake.calls().len(), 2);
    }

    #[test]
    fn edits_are_saved_and_then_read_back() {
        let (mut window, services, fake) = setup();
        let _ = window.view(&services, Some("gw"), true);
        settle(&mut window, &services);
        window.update(Msg::AddUser, &services, "gw", true);
        assert!(window.dialog().is_some());
        let mut form = window.form.take().unwrap().1;
        form.set(0, "dana");
        window.form = Some((Add::User, form));
        window.update_dialog(FormMsg::Ok);
        assert!(window.form.is_none() && window.dirty());
        window.update(Msg::Save, &services, "gw", true);
        settle(&mut window, &services);
        assert!(fake.calls().contains(&"save-access gw".to_string()));
    }

    #[test]
    fn nothing_changes_without_manage() {
        let (mut window, services, fake) = setup();
        window
            .draft
            .members
            .insert("dana".into(), vec![USERS.into()]);
        window.user = Some(0);
        for msg in [
            Msg::ToggleRole(0),
            Msg::AddUser,
            Msg::NewRole,
            Msg::RemoveRole,
        ] {
            window.update(msg, &services, "gw", false);
        }
        window.update(Msg::Grant(encode(0, 1)), &services, "gw", false);
        assert_eq!(window.draft.members["dana"], [USERS]);
        assert!(window.form.is_none());
        window.remote.data = Some(Access::default());
        window.update(Msg::Save, &services, "gw", false);
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn a_name_box_needs_a_name_and_a_new_role_needs_a_new_name() {
        let (mut window, services, _) = setup();
        window.update(Msg::NewRole, &services, "gw", true);
        window.update_dialog(FormMsg::Ok);
        assert!(window.form.as_ref().unwrap().1.error.is_some());
        window.update_dialog(FormMsg::Cancel);
        assert!(window.form.is_none());
    }

    #[test]
    fn nothing_is_edited_while_a_load_or_save_is_in_flight() {
        let (mut window, services, _) = setup();
        let _ = window.view(&services, Some("gw"), true);
        assert!(window.remote.busy());
        window.update(Msg::AddUser, &services, "gw", true);
        window.update(Msg::NewRole, &services, "gw", true);
        window.update(Msg::Reload, &services, "gw", true);
        assert!(window.form.is_none());
        // the controls are drawn disabled meanwhile
        let nodes = window.view(&services, Some("gw"), true);
        let Node::Columns(columns) = &nodes[0] else {
            panic!("no columns");
        };
        let Node::Group { body, .. } = &columns[0].1[0] else {
            panic!("no users group");
        };
        assert!(matches!(body.last(), Some(Node::Toolbar(b)) if !b[0].enabled));
        // a box that was open when the work began cannot add either
        assert!(window.add(Add::User, "dana").is_err());
        settle(&mut window, &services);
        assert!(window.add(Add::User, "dana").is_ok());
    }
}
