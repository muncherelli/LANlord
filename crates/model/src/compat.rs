//! which components need which of the doctor's checks, so every front end shows the same list.
//!
//! components are named here as the user knows them (`nt4`, `network-services`) and each is run by
//! one module today. [`COMPONENTS`] is the only place that ties a name to its module: renaming the
//! `dns` module to `network-services` later is a one-line change there.

use crate::doctor::{Check, Report, Status};

/// a component as the compatibility views name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Named {
    /// the component's name, such as `nt4`.
    pub name: &'static str,
    /// what it does, for the "needed for" column.
    pub title: &'static str,
    /// the module that runs it today.
    pub module: &'static str,
}

/// every component that needs something of the machine, keyed by name.
pub const COMPONENTS: &[Named] = &[
    Named {
        name: "nt4",
        title: "NT4 Server",
        module: "nt4-domain",
    },
    Named {
        name: "network-services",
        title: "Network services (DHCP and DNS)",
        module: "dns",
    },
    Named {
        name: "time-server",
        title: "Time Server (NTP)",
        module: "time-server",
    },
    Named {
        name: "web-proxy",
        title: "Web proxy",
        module: "web-proxy",
    },
    Named {
        name: "docker",
        title: "Docker",
        module: "docker",
    },
];

/// the modules that run in docker, and so need what docker needs: nesting and keyctl in an lxc
/// container (keyctl is only needed when the container's ids are shifted; the report marks a
/// privileged container's keyctl check ok).
const DOCKER_CHECKS: &[&str] = &["lxc-nesting", "lxc-keyctl", "nested-container"];

/// the time server runs in docker too, and needs udp 123 free on the host.
const TIME_CHECKS: &[&str] = &["lxc-nesting", "lxc-keyctl", "nested-container", "ntp-port"];

/// the checks of the report that `module` needs.
pub fn checks_for_module(module: &str) -> &'static [&'static str] {
    match module {
        "time-server" => TIME_CHECKS,
        m if crate::doctor::DOCKER_MODULES.contains(&m) => DOCKER_CHECKS,
        "vde" | "wireguard" | "link" => &["tun"],
        _ => &[],
    }
}

/// the component named `name`.
pub fn component(name: &str) -> Option<&'static Named> {
    COMPONENTS.iter().find(|c| c.name == name)
}

/// the checks the component named `name` needs.
pub fn checks_for(name: &str) -> &'static [&'static str] {
    component(name).map_or(&[], |c| checks_for_module(c.module))
}

/// what needs the check `id`: the titles of the components that do, and the other things that
/// depend on it.
pub fn needed_for(id: &str) -> Vec<&'static str> {
    let mut uses: Vec<&'static str> = COMPONENTS
        .iter()
        .filter(|c| checks_for_module(c.module).contains(&id))
        .map(|c| c.title)
        .collect();
    if id == "tun" {
        uses.push("Remote attachments (VDE and 86Box links)");
        uses.push("Userspace wireguard tunnels (wireguard-go, boringtun)");
    }
    uses
}

/// what the check `id` is for, in plain words: every id the probe reports has one.
pub fn purpose(id: &str) -> &'static str {
    match id {
        "link-bridge" => "bridges, which join the machines of a network",
        "link-dummy" => "dummy links, which stand in for a network card that is not there",
        "link-vlan" => "802.1q vlans, which split one network cable into several networks",
        "link-veth" => "veth pairs, which wire a container or a bridge to a network",
        "link-macvlan" => "macvlan links, which give a network its own address on a card",
        "link-ipvlan" => "ipvlan links, which share a card between several networks",
        "link-vxlan" => "vxlan tunnels, which link networks across gateways",
        "link-gretap" => "gretap tunnels, which link networks across gateways",
        "link-wireguard" => "wireguard tunnels, which link networks over the internet",
        "tun" => "a virtual network device that remote attachments and userspace wireguard open",
        "ip-forward" => "routing between your networks and out to the internet",
        "nftables" => "the firewall rules that share the internet and forward ports",
        "lxc-nesting" => "running containers inside this one, as docker does",
        "lxc-keyctl" => "the keyring calls that docker makes inside a container",
        "nested-container" => "running docker when this server is itself a container",
        "fuse" => "mounting file systems from userspace, as some file sharing does",
        "docker" => "running the components that come as docker containers",
        "network-manager" => "keeping the network manager from taking over lanlord's networks",
        "vde" => "virtual switches that remote pcs plug into",
        "ntp-port" => "answering clock requests on udp port 123",
        "proxmox-macfilter" => "letting remote pcs' traffic through the proxmox firewall",
        _ => "",
    }
}

/// what the check `id` is for and what uses it, as one line for the compatibility lists.
pub fn explain(id: &str) -> String {
    let uses = needed_for(id);
    match (purpose(id), uses.is_empty()) {
        (p, true) => p.to_string(),
        ("", false) => uses.join(", "),
        (p, false) => format!("{p}; used by {}", uses.join(", ")),
    }
}

/// the checks of `report` that the module `module` needs and that are not ok.
pub fn problems<'a>(report: &'a Report, module: &str) -> Vec<&'a Check> {
    let ids = checks_for_module(module);
    report
        .checks
        .iter()
        .filter(|c| ids.contains(&c.id.as_str()) && c.status != Status::Ok)
        .collect()
}

/// the checks that definitely fail for `module`. only these stop a component: a probe that
/// cannot tell never blocks anything.
pub fn blockers<'a>(report: &'a Report, module: &str) -> Vec<&'a Check> {
    problems(report, module)
        .into_iter()
        .filter(|c| c.status == Status::Fail)
        .collect()
}

/// the failing or warning checks that the switched-on `modules` need, each once: what the tray
/// alert is about. a check that could not be tried (unknown) does not alert.
pub fn alerts<'a, 'm>(
    report: &'a Report,
    modules: impl IntoIterator<Item = &'m str>,
) -> Vec<&'a Check> {
    let mut found: Vec<&Check> = Vec::new();
    for module in modules {
        for check in problems(report, module) {
            if check.status != Status::Unknown && !found.iter().any(|c| c.id == check.id) {
                found.push(check);
            }
        }
    }
    found
}

/// does `module` need any check at all, so that it is worth running the probe for it?
pub fn needs_probe(module: &str) -> bool {
    !checks_for_module(module).is_empty()
}

/// the marks of the four statuses as they travel in a list's text: a check mark, a cross, an
/// exclamation mark and a question mark. each front end draws them its own way (the gui paints
/// them as shapes, the tui colours the character); this is the one place that maps a status
/// to its mark.
pub fn mark(status: Status) -> &'static str {
    match status {
        Status::Ok => "\u{2713}",
        Status::Fail => "\u{2717}",
        Status::Warn => "!",
        Status::Unknown => "?",
    }
}

/// split a list's text into the status of the mark it starts with and the rest: the mark is the
/// whole text, or is followed by two spaces.
pub fn parse_mark(text: &str) -> Option<(Status, &str)> {
    [Status::Ok, Status::Fail, Status::Warn, Status::Unknown]
        .into_iter()
        .find_map(|status| {
            let rest = text.strip_prefix(mark(status))?;
            if rest.is_empty() {
                Some((status, rest))
            } else {
                rest.strip_prefix("  ").map(|rest| (status, rest))
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNPRIVILEGED: &str = "virt_container=lxc\nvirt_vm=none\nuid_map=0 100000 65536\npve_marker=1\nnesting=no\nkeyctl=no\ntun=missing\nlink_bridge=ok\n";
    const PRIVILEGED: &str = "virt_container=lxc\nvirt_vm=none\nuid_map=0 0 4294967295\npve_marker=1\nnesting=yes\nkeyctl=no\ntun=ok\n";

    #[test]
    fn components_are_keyed_by_name_and_map_to_their_module() {
        assert_eq!(component("nt4").unwrap().module, "nt4-domain");
        assert_eq!(component("network-services").unwrap().module, "dns");
        assert!(component("nothing").is_none());
        assert_eq!(checks_for("nt4"), checks_for("network-services"));
        assert!(checks_for("nt4").contains(&"lxc-keyctl"));
        assert!(checks_for("nothing").is_empty());
    }

    #[test]
    fn every_module_that_runs_in_docker_needs_what_docker_needs() {
        for module in crate::doctor::DOCKER_MODULES {
            let checks = checks_for_module(module);
            assert!(checks.starts_with(DOCKER_CHECKS), "{module}");
        }
        assert_eq!(checks_for_module("dns"), DOCKER_CHECKS);
        // the time server also needs udp 123
        assert_eq!(checks_for("time-server"), TIME_CHECKS);
        assert!(needed_for("ntp-port").contains(&"Time Server (NTP)"));
        assert_eq!(needed_for("ntp-port").len(), 1);
        assert_eq!(checks_for_module("vde"), ["tun"]);
        assert!(checks_for_module("gateway").is_empty());
        assert!(!needs_probe("gateway"));
        assert!(needs_probe("dns"));
    }

    #[test]
    fn each_check_says_what_needs_it() {
        let uses = needed_for("lxc-nesting");
        assert!(uses.iter().any(|t| t.starts_with("NT4")));
        assert!(uses.iter().any(|t| t.starts_with("Network services")));
        let tun = needed_for("tun");
        assert!(tun.iter().any(|t| t.contains("VDE and 86Box")));
        assert!(tun.iter().any(|t| t.contains("wireguard-go")));
        assert!(
            !tun.iter()
                .any(|t| t.contains("vxlan") || t.contains("gretap")),
            "those do not open /dev/net/tun"
        );
        assert!(needed_for("ip-forward").is_empty());
    }

    #[test]
    fn every_check_the_probe_reports_says_what_it_is_for() {
        let proxmox = "virt_container=lxc\nvirt_vm=none\nuid_map=0 100000 65536\npve_marker=1\n";
        let nested = "virt_container=docker\nvirt_vm=none\n";
        let mut seen = std::collections::BTreeSet::new();
        for probe in [proxmox, nested, "virt_container=none\nvirt_vm=none\n"] {
            for check in Report::parse(probe).checks {
                assert!(!purpose(&check.id).is_empty(), "{}", check.id);
                assert!(!explain(&check.id).is_empty(), "{}", check.id);
                seen.insert(check.id);
            }
        }
        // 9 link kinds, and the 12 others the probe can add
        assert_eq!(seen.len(), 9 + 12, "{seen:?}");
    }

    #[test]
    fn the_explanation_names_the_purpose_and_the_components() {
        assert_eq!(
            explain("ip-forward"),
            "routing between your networks and out to the internet"
        );
        let ntp = explain("ntp-port");
        assert!(ntp.contains("udp port 123") && ntp.ends_with("used by Time Server (NTP)"));
    }

    #[test]
    fn an_unprivileged_container_without_features_blocks_docker_components() {
        let report = Report::parse(UNPRIVILEGED);
        let ids: Vec<&str> = blockers(&report, "nt4-domain")
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(ids, ["lxc-nesting", "lxc-keyctl"]);
        assert!(blockers(&report, "gateway").is_empty());
        let fix = blockers(&report, "dns")[0].fix.clone().unwrap();
        assert!(fix.contains("pct set <CTID> --features nesting=1,keyctl=1"));
    }

    #[test]
    fn a_privileged_container_with_nesting_blocks_nothing() {
        let report = Report::parse(PRIVILEGED);
        assert!(blockers(&report, "nt4-domain").is_empty());
        assert!(alerts(&report, ["nt4-domain", "dns"]).is_empty());
    }

    #[test]
    fn the_alert_lists_each_failing_check_once_and_only_for_what_is_on() {
        let report = Report::parse(UNPRIVILEGED);
        let ids = |modules: &[&str]| -> Vec<String> {
            alerts(&report, modules.iter().copied())
                .iter()
                .map(|c| c.id.clone())
                .collect()
        };
        assert_eq!(ids(&["nt4-domain", "dns"]), ["lxc-nesting", "lxc-keyctl"]);
        assert!(ids(&["gateway"]).is_empty(), "nothing on needs them");
        assert!(ids(&[]).is_empty());
        assert_eq!(ids(&["vde"]), ["tun"], "a warning counts for what needs it");
        assert!(ids(&["dns"]).iter().all(|id| id != "tun"));
    }

    #[test]
    fn the_tun_fix_gives_both_ways_and_is_in_the_box_when_it_fails() {
        let report = Report::parse(UNPRIVILEGED);
        let tun = report.checks.iter().find(|c| c.id == "tun").unwrap();
        let fix = tun.fix.as_deref().unwrap();
        assert!(fix.contains("pct set <CTID> --dev0 /dev/net/tun"));
        assert!(fix.contains("lxc.cgroup2.devices.allow: c 10:200 rwm"));
        assert!(fix.contains("lxc.mount.entry: /dev/net/tun dev/net/tun none bind,create=file"));
        assert!(fix.contains("/etc/pve/lxc/<CTID>.conf"));
        assert_eq!(problems(&report, "vde")[0].id, "tun");
    }

    #[test]
    fn marks_tell_the_four_statuses_apart() {
        let marks: Vec<&str> = [Status::Ok, Status::Fail, Status::Warn, Status::Unknown]
            .into_iter()
            .map(mark)
            .collect();
        assert_eq!(marks, ["\u{2713}", "\u{2717}", "!", "?"]);
        for status in [Status::Ok, Status::Fail, Status::Warn, Status::Unknown] {
            assert_eq!(parse_mark(mark(status)), Some((status, "")));
            let with = format!("{}  needs docker", mark(status));
            assert_eq!(parse_mark(&with), Some((status, "needs docker")));
        }
        assert_eq!(parse_mark("Needs docker"), None);
        assert_eq!(parse_mark("!bang"), None);
    }
}
