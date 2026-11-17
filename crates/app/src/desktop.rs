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
}

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
        }
    }

    /// how big the window opens, in layout units, before it is cut down to fit the desktop.
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
            Self::Console => (600.0, 300.0),
            Self::Hosts => (380.0, 280.0),
            Self::Settings => (600.0, 520.0),
            Self::DateTime => (430.0, 420.0),
            Self::Components => (500.0, 470.0),
            Self::Welcome => (560.0, 360.0),
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
}
