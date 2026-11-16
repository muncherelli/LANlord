//! the LANlord menu: the server it works on, the windows, power actions, updates and quitting.
//! the rows are built here; each front end draws them and hands back the chosen action.

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
    /// the server is there and nothing is running against it.
    pub ready: bool,
    /// automatic updates are on; `None` where lanlord is not updated from the menu (the browser).
    pub updates: Option<bool>,
    /// this is a development build with a Developer submenu.
    pub developer: bool,
}

/// the line at the top of the menu naming the server every action targets.
pub fn server_line(host: Option<&str>) -> String {
    host.map_or_else(|| "No server".to_string(), |h| format!("Server: {h}"))
}

/// the main menu.
pub fn rows(state: &State) -> Vec<Row> {
    let mut rows = vec![Row::Item(Item {
        header: true,
        ..Item::new(&server_line(state.host), Action::ServerLine)
    })];
    if let Some(who) = state.signed_in {
        rows.push(Row::Item(Item::new(
            "Switch server...",
            Action::SwitchServer,
        )));
        rows.push(Row::Item(Item::new(
            &format!("Log off {who}..."),
            Action::LogOff,
        )));
    }
    rows.push(Row::Separator);
    rows.extend(MODULES.iter().enumerate().map(|(i, m)| {
        Row::Item(Item {
            art: Some(m.id),
            ..Item::new(m.name, Action::Open(Target::Module(i)))
        })
    }));
    for target in [
        Target::MyServer,
        Target::Networks,
        Target::Settings,
        Target::Console,
    ] {
        rows.push(Row::Item(Item {
            art: Some(target.id()),
            ..Item::new(target.title(), Action::Open(target))
        }));
    }
    rows.push(Row::Separator);
    for (label, power) in [
        ("Reboot server...", Power::Reboot),
        ("Shut down server...", Power::PowerOff),
    ] {
        rows.push(Row::Item(Item {
            enabled: state.ready,
            ..Item::new(label, Action::Power(power))
        }));
    }
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

    fn labels(updates: Option<bool>) -> Vec<String> {
        let state = State {
            host: Some("local"),
            signed_in: None,
            ready: true,
            updates,
            developer: false,
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
    fn the_updates_row_is_only_there_where_lanlord_can_update() {
        assert!(labels(Some(true)).iter().any(|l| l == "Automatic updates"));
        assert!(!labels(None).iter().any(|l| l == "Automatic updates"));
    }
}
