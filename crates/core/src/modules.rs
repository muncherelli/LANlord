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

pub const MODULES: &[Module] = &[
    Module {
        id: "gateway",
        name: "Gateway",
        icon: ["┌─────┐", "│ ▒▒▒ │", "└─┬─┬─┘"],
        summary: "Baseline for the gateway: its state record and message of the day.",
        example: "gateway:\n  motd: \"welcome to the retronet\\n\"",
        status: Status::Available,
    },
    Module {
        id: "docker",
        name: "Docker",
        icon: ["  ▄▄▄  ", "▄█████▄", " ▀▀▀▀▀ "],
        summary: "Installs Docker and gives eth1 its own address, with eth0 firewalled off from the retro lan.",
        example: "docker:\n  lan_interface: eth1\n  lan_address: 172.16.120.2/22",
        status: Status::Planned,
    },
    Module {
        id: "nt4-domain",
        name: "NT4 Domain",
        icon: ["╔═════╗", "║ NT4 ║", "╚═════╝"],
        summary: "A Samba NT4-style primary domain controller in Docker, on eth1 only, with a basic set of logins.",
        example: "nt4-domain:\n  domain: RETRONET\n  users:\n    - name: james",
        status: Status::Planned,
    },
    Module {
        id: "web-proxy",
        name: "Web Proxy",
        icon: ["  ◢◣   ", " ◢██◣  ", "◢████◣ "],
        summary: "Caddy in Docker, proxying sites on the retro lan and the lanlord web interface.",
        example: "web-proxy:\n  sites:\n    - host: nostalgia.lan\n      upstream: 172.16.120.20:80",
        status: Status::Planned,
    },
    Module {
        id: "remote-commands",
        name: "Remote Cmds",
        icon: ["┌─────┐", "│ C:\\>│", "└─────┘"],
        summary: "Run commands on NT4 servers through the Resource Kit Remote Command Service.",
        example: "remote-commands:\n  servers:\n    - name: NTSRV1\n      address: 172.16.120.10",
        status: Status::Planned,
    },
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
