//! the desktop every front end shows: which icons it has and in what order, the windows they
//! open, how big those windows start, and the title of the whole app.

use lanlord_model::access::{Area, Grants, Level};
use lanlord_model::components::Selection;
use lanlord_model::modules::MODULES;

/// a window the desktop can open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Module(usize),
    MyServer,
    Networks,
    Console,
    Hosts,
    Settings,
    /// opened by double-clicking the clock; it has no desktop icon.
    DateTime,
    /// the components setup wizard, opened from the menu; it has no desktop icon.
    Components,
    /// the Welcome window, shown after logon and from the menu; it has no desktop icon.
    Welcome,
    /// the changelog, read only; opened from the Welcome window and the menu's help rows.
    Changelog,
    /// the signed-in person's own account, for everyone; opened from the menu.
    MyAccount,
    /// users, roles and what each role may do; opened from the menu.
    Permissions,
    /// the progress of an apply; it opens by itself when one starts and has no desktop icon.
    Progress,
    /// the Task Manager, opened from the menu or by double-clicking the tray's network icon.
    TaskManager,
}

/// the console opens as 80 columns by 24 rows of text, the size of a terminal.
pub const CONSOLE_COLS: f32 = 80.0;
pub const CONSOLE_ROWS: f32 = 24.0;
/// a text cell in layout units, for the front ends that have no font to measure; the window
/// front end measures its own.
const CELL: (f32, f32) = (8.0, 16.0);

impl Target {
    /// the id of the window's icon, and of the module for a module window.
    pub fn id(self) -> &'static str {
        match self {
            Self::Module(i) => MODULES[i].id,
            Self::MyServer => "my-server",
            Self::Networks => "networks",
            Self::Console => "console",
            Self::Hosts => "hosts",
            Self::Settings => "settings",
            Self::DateTime => "datetime",
            Self::Components => "components",
            Self::Welcome => "welcome",
            Self::Changelog => "changelog",
            Self::MyAccount => "my-account",
            Self::Permissions => "permissions",
            Self::Progress => "apply-progress",
            Self::TaskManager => "taskmgr",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Module(i) => MODULES[i].name,
            Self::MyServer => "My Server",
            Self::Networks => "Networks",
            Self::Console => "Console",
            Self::Hosts => "Hosts",
            Self::Settings => "Settings",
            Self::DateTime => "Date/Time Properties",
            Self::Components => "LANlord Components",
            Self::Welcome => "Welcome",
            Self::Changelog => "Changelog",
            Self::MyAccount => "My Account",
            Self::Permissions => "Users and Permissions",
            Self::Progress => "Applying Settings",
            Self::TaskManager => "Task Manager",
        }
    }

    /// the contents can grow (a console, a list, a text): the user may resize the window and the
    /// growable part fills it. the rest are forms, as big as their controls.
    pub fn growable(self) -> bool {
        match self {
            Self::Console
            | Self::Hosts
            | Self::Changelog
            | Self::MyServer
            | Self::Networks
            | Self::Settings
            | Self::Permissions
            | Self::Progress
            | Self::TaskManager => true,
            // the modules with a list of logins, hosts or commands; the rest are plain forms
            Self::Module(i) => matches!(MODULES[i].id, "nt4-domain" | "dns" | "remote-commands"),
            _ => false,
        }
    }

    /// the least height a growable window opens at, for contents that are near empty: the hosts
    /// list, the apply log and the process list are not a strip. 0 for the rest, which are as tall as their contents.
    pub fn floor(self) -> f32 {
        match self {
            Self::Hosts | Self::Progress | Self::TaskManager => self.size().1,
            _ => 0.0,
        }
    }

    /// the area whose data the window shows, `None` for a window that shows nothing of the
    /// server's (Hosts, Welcome, Changelog, My Account).
    pub fn area(self) -> Option<Area> {
        match self {
            Self::Module(i) => Some(match MODULES[i].id {
                "nt4-domain" => Area::Domain,
                "dns" => Area::Dns,
                "remote-commands" => Area::Commands,
                _ => Area::Services,
            }),
            // anyone may watch the Task Manager; ending a process is for administrators
            Self::MyServer | Self::TaskManager => Some(Area::Status),
            Self::Networks => Some(Area::Network),
            Self::Console => Some(Area::Commands),
            Self::Settings | Self::Components => Some(Area::Services),
            Self::DateTime => Some(Area::Clock),
            Self::Permissions => Some(Area::Users),
            // the progress of an apply opens by itself for whoever ran it; the apply operation is
            // what is checked
            Self::Hosts | Self::Welcome | Self::Changelog | Self::MyAccount | Self::Progress => {
                None
            }
        }
    }

    /// the windows that can show their data read-only; every other one only opens for someone
    /// who may change it.
    fn reads_only(self) -> bool {
        matches!(self, Self::MyServer | Self::Permissions | Self::TaskManager)
    }

    /// the level of the window's area needed to open it: View for the windows that can be
    /// read-only, Manage for the rest. nothing is needed where there is no area.
    pub fn opens_at(self) -> Level {
        match self.area() {
            None => Level::Hidden,
            Some(_) if self.reads_only() => Level::View,
            Some(_) => Level::Manage,
        }
    }

    /// may these grants open the window? the menu and the desktop hide it when not.
    pub fn visible(self, grants: &Grants) -> bool {
        self.area()
            .is_none_or(|area| grants.level(area) >= self.opens_at())
    }

    /// may these grants change what the window shows? when not, its controls are read-only.
    pub fn writable(self, grants: &Grants) -> bool {
        self.area()
            .is_none_or(|area| grants.level(area) >= Level::Manage)
    }

    /// a dialog has only a close button, cannot be resized and is as tall as its contents.
    pub fn is_dialog(self) -> bool {
        matches!(
            self,
            Self::Welcome | Self::DateTime | Self::Components | Self::MyAccount
        )
    }

    /// how big the window opens in the text view, in layout units. the gui measures its own: a
    /// window is as tall as its contents, a console as many cells as `CONSOLE_COLS` by
    /// `CONSOLE_ROWS`.
    pub fn size(self) -> (f32, f32) {
        match self {
            Self::Module(i) => match MODULES[i].id {
                "nt4-domain" => (600.0, 600.0),
                "dns" => (620.0, 580.0),
                "remote-commands" => (580.0, 580.0),
                _ => (480.0, 320.0),
            },
            Self::MyServer => (440.0, 480.0),
            Self::Networks => (560.0, 440.0),
            Self::Console => (CONSOLE_COLS * CELL.0, CONSOLE_ROWS * CELL.1),
            Self::Hosts => (380.0, 280.0),
            Self::Settings => (600.0, 520.0),
            Self::DateTime => (430.0, 420.0),
            Self::Components => (500.0, 470.0),
            Self::Welcome => (
                crate::welcome::PANEL + crate::welcome::TOPIC_BUTTON + 8.0,
                380.0,
            ),
            Self::Changelog => (520.0, 400.0),
            Self::MyAccount => (400.0, 0.0),
            Self::Permissions => (640.0, 520.0),
            Self::Progress => (480.0, 360.0),
            Self::TaskManager => (500.0, 490.0),
        }
    }
}

/// the desktop icons, top to bottom then left to right: the server and its networks, the
/// components this host has switched on (none until its settings were read), then the built-in
/// windows.
pub fn icons(enabled: Option<&Selection>) -> Vec<Target> {
    let mut icons = vec![Target::MyServer, Target::Networks];
    icons.extend(modules(enabled).into_iter().map(Target::Module));
    icons.extend([Target::Settings, Target::Console, Target::Hosts]);
    icons
}

/// the windows that exist for `icons`: the desktop icons plus the Components wizard, which has
/// none.
pub fn windows(icons: &[Target]) -> Vec<Target> {
    let mut windows = icons.to_vec();
    windows.push(Target::Components);
    windows.extend([Target::MyAccount, Target::Permissions]);
    windows
}

/// the positions in `MODULES` of the components switched on in `enabled`.
pub fn modules(enabled: Option<&Selection>) -> Vec<usize> {
    let Some(enabled) = enabled else {
        return Vec::new();
    };
    (0..MODULES.len())
        .filter(|&i| enabled.is_enabled(MODULES[i].id))
        .collect()
}

/// "LANlord Client v0.1.0" on a mac or pc, "LANlord Server v0.1.0" when lanlord runs on the
/// gateway itself (server mode, which is also what the web ui is).
pub fn app_title(local: bool) -> String {
    let side = if local { "Server" } else { "Client" };
    format!("LANlord {side} v{}", env!("CARGO_PKG_VERSION"))
}

/// the title of the whole app: its name and version, plus the server once someone is signed in.
pub fn window_title(local: bool, signed_in_to: Option<&str>) -> String {
    match signed_in_to {
        Some(host) => format!("{} - {host}", app_title(local)),
        None => app_title(local),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_names_the_side_and_the_server() {
        let version = env!("CARGO_PKG_VERSION");
        assert_eq!(
            window_title(false, None),
            format!("LANlord Client v{version}")
        );
        assert_eq!(
            window_title(true, Some("local")),
            format!("LANlord Server v{version} - local")
        );
    }

    #[test]
    fn windows_follow_the_grants() {
        use lanlord_model::access::Access;
        let user = Access::default().grants("dana", false);
        let admin = Grants::all(Level::Manage);
        // a User sees My Server (read-only) and their own account, nothing that changes things
        assert!(Target::MyServer.visible(&user) && !Target::MyServer.writable(&user));
        assert!(Target::MyAccount.visible(&user) && Target::Hosts.visible(&user));
        for hidden in [
            Target::Networks,
            Target::Settings,
            Target::Console,
            Target::Permissions,
        ] {
            assert!(!hidden.visible(&user), "{hidden:?}");
        }
        assert!(Target::Permissions.visible(&admin) && Target::Permissions.writable(&admin));
        // viewing the users does not let you change them
        let mut viewer = Grants::default();
        viewer.0.insert(Area::Users, Level::View);
        assert!(Target::Permissions.visible(&viewer) && !Target::Permissions.writable(&viewer));
    }

    #[test]
    fn every_icon_has_its_own_id_and_the_clock_window_has_no_icon() {
        let all = Selection {
            enabled: MODULES
                .iter()
                .map(|m| (m.id.to_string(), Default::default()))
                .collect(),
        };
        let icons = icons(Some(&all));
        let ids: std::collections::HashSet<_> = icons.iter().map(|t| t.id()).collect();
        assert_eq!(ids.len(), icons.len());
        assert!(!icons.contains(&Target::DateTime));
        assert!(!icons.contains(&Target::Components));
        assert!(!icons.contains(&Target::Welcome));
    }

    #[test]
    fn only_switched_on_components_get_icons() {
        let builtins = [
            Target::MyServer,
            Target::Networks,
            Target::Settings,
            Target::Console,
            Target::Hosts,
        ];
        assert_eq!(icons(None), builtins);
        let mut some = Selection::default();
        some.enabled.insert("dns".into(), Default::default());
        some.enabled.insert("gateway".into(), Default::default());
        let ids: Vec<_> = icons(Some(&some)).iter().map(|t| t.id()).collect();
        assert_eq!(
            ids,
            [
                "my-server",
                "networks",
                "gateway",
                "dns",
                "settings",
                "console",
                "hosts"
            ]
        );
    }

    #[test]
    fn anyone_may_open_the_task_manager() {
        use lanlord_model::access::{Grants, Level};
        assert!(Target::TaskManager.visible(&Grants::fallback(false)));
        assert_eq!(Target::TaskManager.opens_at(), Level::View);
    }

    #[test]
    fn the_progress_window_has_no_icon_and_names_itself() {
        assert_eq!(Target::Progress.id(), "apply-progress");
        assert_eq!(Target::Progress.title(), "Applying Settings");
        // it opens by itself for whoever ran the apply, which is what the server checks
        assert_eq!(Target::Progress.area(), None);
        assert!(Target::Progress.size().0 > 0.0);
        assert!(!icons(None).contains(&Target::Progress));
        assert!(!windows(&icons(None)).contains(&Target::Progress));
    }
}
