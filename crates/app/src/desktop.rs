//! the desktop every front end shows: which icons it has and in what order, the windows they
//! open, how big those windows start, and the title of the whole app.

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
            | Self::Settings => true,
            // the modules with a list of logins, hosts or commands; the rest are plain forms
            Self::Module(i) => matches!(MODULES[i].id, "nt4-domain" | "dns" | "remote-commands"),
            _ => false,
        }
    }

    /// a dialog has only a close button, cannot be resized and is as tall as its contents.
    pub fn is_dialog(self) -> bool {
        matches!(self, Self::Welcome | Self::DateTime | Self::Components)
    }

    /// the window changes things on the server (settings, components, networks, users, the
    /// clock, commands), so only an administrator may open it. My Server stays open to anyone:
    /// it reads, and its purge is switched off for the rest. the Task Manager is open to anyone too; ending a process is the part that needs an admin.
    pub fn needs_admin(self) -> bool {
        !matches!(
            self,
            Self::MyServer | Self::Hosts | Self::Welcome | Self::Changelog | Self::TaskManager
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
    fn the_progress_window_has_no_icon_and_names_itself() {
        assert_eq!(Target::Progress.id(), "apply-progress");
        assert_eq!(Target::Progress.title(), "Applying Settings");
        assert!(
            Target::Progress.needs_admin(),
            "applying is for administrators"
        );
        assert!(Target::Progress.size().0 > 0.0);
        assert!(!icons(None).contains(&Target::Progress));
        assert!(!windows(&icons(None)).contains(&Target::Progress));
    }
}
