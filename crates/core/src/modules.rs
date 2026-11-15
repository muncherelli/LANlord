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
        "Installs Docker and drops NetBIOS and SMB on the host network. Finds the host and internal interfaces itself and works on lxc, vms and bare metal.",
        "docker: {}\n\n# optional, found automatically:\nnetwork:\n  host_interface: eth0\n  internal_interface: eth1",
        Status::Available,
    ),
    Module::new(
        "nt4-domain",
        "NT4 Domain",
        ["╔═════╗", "║ NT4 ║", "╚═════╝"],
        "A Samba server in Docker for old Windows clients, on the internal network only: an NT4-style domain controller or a plain workgroup, with a basic set of logins.",
        "nt4-domain:\n  mode: domain\n  name: RETRONET\n  admin_password: ...\n  machines: [ws1, ws2]\n  users:\n    - name: james\n      password: ...",
        Status::Available,
    ),
    Module::new(
        "web-proxy",
        "Web Proxy",
        ["  ◢◣   ", " ◢██◣  ", "◢████◣ "],
        "Caddy in Docker, proxying web requests to sites on the internal network and to the lanlord web interface.",
        "web-proxy:\n  sites:\n    - host: nostalgia.lan\n      upstream: 172.16.120.20:80",
        Status::Available,
    ),
    Module::new(
        "remote-commands",
        "Remote Cmds",
        ["┌─────┐", "│ C:\\>│", "└─────┘"],
        "Run commands on the gateway over ssh. Running them on NT4 machines is still being researched.",
        "remote-commands:\n  servers:\n    - name: NTSRV1\n      address: 172.16.120.10",
        Status::Available,
    ),
    Module::new(
        "dns",
        "DNS",
        ["┌─────┐", "│ DNS │", "└─────┘"],
        "dnsmasq in Docker: local-only DNS for the retronet and optional gatewayless DHCP for Windows 98, 2000, NT4 and old Linux clients.",
        "dns:\n  zone: retronet\n  records:\n    - {name: ntsrv1, address: 172.16.120.10}\n  dhcp:\n    range: [172.16.120.100, 172.16.120.200]\n    lease: 7d",
        Status::Available,
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
    fn the_built_modules_are_available() {
        assert_eq!(find("gateway").unwrap().status, Status::Available);
        assert_eq!(find("docker").unwrap().status, Status::Available);
        assert_eq!(find("nt4-domain").unwrap().status, Status::Available);
        assert_eq!(find("web-proxy").unwrap().status, Status::Available);
        assert_eq!(find("dns").unwrap().status, Status::Available);
        assert_eq!(find("remote-commands").unwrap().status, Status::Available);
    }
}
