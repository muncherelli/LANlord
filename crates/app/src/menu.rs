//! the LANlord menu: the server it works on, the windows, power actions, updates and quitting.
//! the rows are built here; each front end draws them and hands back the chosen action.

use lanlord_model::access::{Area, Grants, Level};
use lanlord_model::modules::MODULES;
use lanlord_model::power::Power;

use crate::desktop::Target;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Open(Target),
    /// back to the logon box to pick another server.
    SwitchServer,
    /// back to the logon box.
    LogOff,
    Power(Power),
    /// switch the background update check on or off.
    ToggleUpdates,
    /// open or close the Developer submenu.
    Developer,
    /// rebuild this build from its checkout and start it again.
    Rebuild,
    /// look for an update now.
    CheckUpdates,
    About,
    Quit,
    /// the server line at the top of the menu, which only names the server.
    ServerLine,
}

pub enum Row {
    Item(Item),
    Separator,
}

pub struct Item {
    pub label: String,
    /// the id of the small icon drawn before the label.
    pub art: Option<&'static str>,
    pub action: Action,
    pub ticked: bool,
    /// the row opens a submenu.
    pub arrow: bool,
    pub enabled: bool,
    /// a line of text that names something and does nothing when clicked.
    pub header: bool,
}

impl Item {
    pub fn new(label: &str, action: Action) -> Self {
        Self {
            label: label.into(),
            art: None,
            action,
            ticked: false,
            arrow: false,
            enabled: true,
            header: false,
        }
    }

    pub fn label(&self) -> &str {
        &self.label
    }
}

/// what the menu needs to know about the desktop.
pub struct State<'a> {
    /// the server every action targets.
    pub host: Option<&'a str>,
    /// someone signed in through the logon box (so they can switch server or log off).
    pub signed_in: Option<&'a str>,
    /// a different server can be chosen at the logon box (not in the browser, which has one).
    pub switch_server: bool,
    /// what the signed-in account may do; what it may not is left out of the menu.
    pub grants: Grants,
    /// the server is there and nothing is running against it.
    pub ready: bool,
    /// automatic updates are on; `None` where lanlord is not updated from the menu (the browser).
    pub updates: Option<bool>,
    /// this is a development build with a Developer submenu.
    pub developer: bool,
    /// the positions in `MODULES` of the components this host has switched on.
    pub modules: Vec<usize>,
}

/// the line at the top of the menu naming the server every action targets.
pub fn server_line(host: Option<&str>) -> String {
    host.map_or_else(|| "No server".to_string(), |h| format!("Server: {h}"))
}

/// everyone's own account, and the permissions window for whoever may open it.
fn account_rows(state: &State) -> Vec<Row> {
    let mut rows = vec![Row::Separator];
    for target in [Target::MyAccount, Target::Permissions] {
        if target.visible(&state.grants) {
            rows.push(Row::Item(Item::new(
                &format!("{}...", target.title()),
                Action::Open(target),
            )));
        }
    }
    rows
}

/// reboot and shut down, for whoever may manage the server's status.
fn power_rows(state: &State) -> Vec<Row> {
    if state.grants.level(Area::Status) < Level::Manage {
        return Vec::new();
    }
    let mut rows = vec![Row::Separator];
    for (label, power) in [
        ("Reboot server...", Power::Reboot),
        ("Shut down server...", Power::PowerOff),
    ] {
        rows.push(Row::Item(Item {
            enabled: state.ready,
            ..Item::new(label, Action::Power(power))
        }));
    }
    rows
}

/// the main menu.
pub fn rows(state: &State) -> Vec<Row> {
    let mut rows = vec![Row::Item(Item {
        header: true,
        ..Item::new(&server_line(state.host), Action::ServerLine)
    })];
    if let Some(who) = state.signed_in {
        if state.switch_server {
            rows.push(Row::Item(Item::new(
                "Switch server...",
                Action::SwitchServer,
            )));
        }
        rows.push(Row::Item(Item::new(
            &format!("Log off {who}..."),
            Action::LogOff,
        )));
    }
    rows.push(Row::Separator);
    rows.extend(
        state
            .modules
            .iter()
            .filter(|&&i| Target::Module(i).visible(&state.grants))
            .map(|&i| {
                let m = &MODULES[i];
                Row::Item(Item {
                    art: Some(m.id),
                    ..Item::new(m.name, Action::Open(Target::Module(i)))
                })
            }),
    );
    for target in [
        Target::MyServer,
        Target::Networks,
        Target::Settings,
        Target::Console,
    ]
    .into_iter()
    .filter(|t| t.visible(&state.grants))
    {
        rows.push(Row::Item(Item {
            art: Some(target.id()),
            ..Item::new(target.title(), Action::Open(target))
        }));
    }
    if Target::Components.visible(&state.grants) {
        rows.push(Row::Item(Item {
            enabled: state.host.is_some(),
            ..Item::new("Add/Remove Components...", Action::Open(Target::Components))
        }));
    }
    rows.extend(account_rows(state));
    rows.extend(power_rows(state));
    rows.push(Row::Separator);
    if let Some(on) = state.updates {
        rows.push(Row::Item(Item {
            ticked: on,
            ..Item::new("Automatic updates", Action::ToggleUpdates)
        }));
    }
    if state.developer {
        rows.push(Row::Item(Item {
            arrow: true,
            ..Item::new("Developer", Action::Developer)
        }));
    }
    rows.push(Row::Separator);
    rows.push(Row::Item(Item::new(
        "Welcome...",
        Action::Open(Target::Welcome),
    )));
    rows.push(Row::Item(Item::new("About LANlord", Action::About)));
    rows.push(Row::Item(Item::new("Quit LANlord", Action::Quit)));
    rows
}

/// the Developer submenu, only there in a development build.
pub fn developer_rows(rebuilding: bool) -> Vec<Row> {
    vec![
        Row::Item(Item {
            enabled: !rebuilding,
            ..Item::new("Rebuild and relaunch...", Action::Rebuild)
        }),
        Row::Item(Item::new("Check for updates now", Action::CheckUpdates)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanlord_model::access::Access;

    fn labels(updates: Option<bool>) -> Vec<String> {
        let state = State {
            host: Some("local"),
            signed_in: None,
            switch_server: true,
            grants: Grants::all(Level::Manage),
            ready: true,
            updates,
            developer: false,
            modules: Vec::new(),
        };
        rows(&state)
            .into_iter()
            .filter_map(|row| match row {
                Row::Item(item) => Some(item.label().to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_browser_logs_off_but_has_no_other_server_to_switch_to() {
        let state = |switch_server| State {
            host: Some("local"),
            signed_in: Some("ann"),
            switch_server,
            grants: Grants::all(Level::Manage),
            ready: true,
            updates: None,
            developer: false,
            modules: Vec::new(),
        };
        let has = |state: &State, label: &str| {
            rows(state)
                .iter()
                .any(|r| matches!(r, Row::Item(i) if i.label() == label))
        };
        assert!(has(&state(true), "Switch server..."));
        assert!(!has(&state(false), "Switch server..."));
        assert!(has(&state(false), "Log off ann..."));
    }

    #[test]
    fn only_those_who_may_get_the_power_and_admin_items() {
        let titles = |grants| {
            let state = State {
                host: Some("local"),
                signed_in: Some("ann"),
                switch_server: false,
                grants,
                ready: true,
                updates: None,
                developer: false,
                modules: Vec::new(),
            };
            rows(&state)
                .into_iter()
                .filter_map(|r| match r {
                    Row::Item(i) => Some(i.label),
                    Row::Separator => None,
                })
                .collect::<Vec<_>>()
        };
        let admin = titles(Grants::all(Level::Manage));
        for item in [
            "Reboot server...",
            "Settings",
            "Networks",
            "Users and Permissions...",
        ] {
            assert!(admin.iter().any(|l| l == item), "{item}");
        }
        let user = titles(Access::default().grants("ann", false));
        for item in [
            "Reboot server...",
            "Settings",
            "Networks",
            "Console",
            "Users and Permissions...",
        ] {
            assert!(!user.iter().any(|l| l == item), "{item}");
        }
        for item in ["My Server", "My Account..."] {
            assert!(user.iter().any(|l| l == item), "{item}");
        }
    }

    #[test]
    fn the_updates_row_is_only_there_where_lanlord_can_update() {
        assert!(labels(Some(true)).iter().any(|l| l == "Automatic updates"));
        assert!(!labels(None).iter().any(|l| l == "Automatic updates"));
    }
}
