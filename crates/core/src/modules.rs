//! the features lanlord offers, shown as icons that open windows. a module's
//! ansible tasks carry its id as a tag, and its settings live under that
//! key in your settings file.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// works today.
    Available,
    /// on the roadmap; the window says so.
    Planned,
}

#[derive(Debug)]
pub struct Module {
    /// the ansible tag and the key in the settings file.
    pub id: &'static str,
    pub name: &'static str,
    /// three lines of pixel art for the desktop.
    pub icon: [&'static str; 3],
    pub summary: &'static str,
    /// an example of the settings this module reads.
    pub example: &'static str,
    pub status: Status,
}

impl Module {
    const fn new(
        id: &'static str,
        name: &'static str,
        icon: [&'static str; 3],
        summary: &'static str,
        example: &'static str,
        status: Status,
    ) -> Self {
        Self {
            id,
            name,
            icon,
            summary,
            example,
            status,
        }
    }
}

pub const MODULES: &[Module] = &[
    Module::new(
        "gateway",
        "Gateway",
        ["┌─────┐", "│ ▒▒▒ │", "└─┬─┬─┘"],
        "Baseline for the gateway: its state record and message of the day.",
        "gateway:\n  motd: \"welcome to the retronet\\n\"",
        Status::Available,
    ),
    Module::new(
        "docker",
        "Docker",
        ["  ▄▄▄  ", "▄█████▄", " ▀▀▀▀▀ "],
        "Installs Docker and gives eth1 its own address, with eth0 firewalled off from the retro lan.",
        "docker:\n  lan_interface: eth1\n  lan_address: 172.16.120.2/22",
        Status::Planned,
    ),
    Module::new(
        "nt4-domain",
        "NT4 Domain",
        ["╔═════╗", "║ NT4 ║", "╚═════╝"],
        "A Samba NT4-style primary domain controller in Docker, on eth1 only, with a basic set of logins.",
        "nt4-domain:\n  domain: RETRONET\n  users:\n    - name: james",
        Status::Planned,
    ),
    Module::new(
        "web-proxy",
        "Web Proxy",
        ["  ◢◣   ", " ◢██◣  ", "◢████◣ "],
        "Caddy in Docker, proxying sites on the retro lan and the lanlord web interface.",
        "web-proxy:\n  sites:\n    - host: nostalgia.lan\n      upstream: 172.16.120.20:80",
        Status::Planned,
    ),
    Module::new(
        "remote-commands",
        "Remote Cmds",
        ["┌─────┐", "│ C:\\>│", "└─────┘"],
        "Run commands on NT4 servers through the Resource Kit Remote Command Service.",
        "remote-commands:\n  servers:\n    - name: NTSRV1\n      address: 172.16.120.10",
        Status::Planned,
    ),
];

/// look a module up by its id.
pub fn find(id: &str) -> Option<&'static Module> {
    MODULES.iter().find(|m| m.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_safe_as_tags() {
        let mut seen = std::collections::HashSet::new();
        for module in MODULES {
            assert!(seen.insert(module.id), "duplicate id {}", module.id);
            assert!(module
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        }
    }

    #[test]
    fn icons_are_the_same_width() {
        for module in MODULES {
            let widths: Vec<_> = module.icon.iter().map(|l| l.chars().count()).collect();
            assert!(widths.iter().all(|w| *w == widths[0]), "{}", module.id);
        }
    }

    #[test]
    fn examples_are_keyed_by_the_module_id() {
        for module in MODULES {
            assert!(module.example.starts_with(&format!("{}:", module.id)));
        }
    }

    #[test]
    fn find_returns_modules_by_id() {
        assert_eq!(find("gateway").unwrap().name, "Gateway");
        assert!(find("nope").is_none());
    }

    #[test]
    fn gateway_is_the_available_module() {
        assert_eq!(find("gateway").unwrap().status, Status::Available);
    }
}
