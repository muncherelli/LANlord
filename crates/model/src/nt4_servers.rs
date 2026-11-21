//! the nt4 servers of a gateway and the domains they serve.
//!
//! settings used to have one `nt4-domain:` section that was both the domain and the one samba
//! server. now the two are apart (see the nt4 server roadmap):
//!
//! ```yaml
//! domains:                        # what a domain is: its logon behaviour and its machine accounts
//!   RETRODOM:
//!     backend: nt4
//!     roaming_profiles: true
//!     machines: [ws1, ws2]
//! nt4-servers:                    # one samba container each; several per network are allowed
//!   - name: nt4-domain            # the id; the migrated server keeps this and its old names
//!     network: retronet           # default: the first network
//!     netbios_name: LANLORD       # default: the machine's hostname
//!     role: pdc                   # pdc | workgroup
//!     domain: RETRODOM            # pdc: a key of `domains`
//!     address: 192.0.2.5/24       # its own address; no address: the gateway's (the default)
//!     wins: serve                 # serve | off; one server per network serves wins
//!     software: true              # a software share, S: at logon
//! ```
//!
//! the logins (`users`, `admin_password`) and the samba tuning keys (`lanman_auth`, `log_level`)
//! stay in the `nt4-domain:` section for now, so their secrets keep their paths. the server named
//! `nt4-domain` is the one that was there before: container `lanlord-nt4`, data in
//! `/opt/lanlord/nt4`. every other server is `lanlord-nt4-<name>` with data in
//! `/opt/lanlord/nt4/<name>`.

use std::collections::BTreeMap;

use anyhow::Result;
use serde_yaml_ng::{Mapping, Value};

use crate::format::Context;
use crate::settings::{Settings, ValidationError};

/// the settings key of the list of servers.
pub const SERVERS: &str = "nt4-servers";
/// the settings key of the domains.
pub const DOMAINS: &str = "domains";
/// the section the logins stay in, and the id of the server migrated from it.
pub const LEGACY: &str = "nt4-domain";

/// folders of the state directory that a server must not be named after.
const RESERVED: [&str; 4] = ["bin", "config", "lib", "snapshots"];

/// the keys the old `nt4-domain:` section held that now belong to a domain or a server.
const MOVED: [&str; 8] = [
    "mode",
    "name",
    "netbios_name",
    "network",
    "address",
    "roaming_profiles",
    "software",
    "machines",
];

/// the key of `domains` that is `name`, ignoring case (domain names are not case sensitive).
pub fn domain_key(domains: &Mapping, name: &str) -> Option<Value> {
    domains
        .keys()
        .find(|k| k.as_str().is_some_and(|k| k.eq_ignore_ascii_case(name)))
        .cloned()
}

fn text(map: &Mapping, key: &str) -> Option<String> {
    match map.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// whether the settings still have the single-server shape: the old section holds keys that have
/// moved, and there is no list of servers.
pub fn has_legacy(settings: &Settings) -> bool {
    settings.section(SERVERS).is_none()
        && matches!(
            settings.section(LEGACY),
            Some(Value::Mapping(map)) if MOVED.iter().any(|k| map.contains_key(*k))
        )
}

/// settings step: the `nt4-domain:` section becomes one domain and one server. the server keeps
/// the container, the data folder and the NetBIOS name the old section gave, so a running
/// install is not disturbed. the old section stays for what has not moved (logins).
pub fn migrate_legacy(settings: &mut Settings, _: &Context) -> Result<()> {
    if !has_legacy(settings) {
        return Ok(());
    }
    let Some(Value::Mapping(old)) = settings.section(LEGACY).cloned() else {
        return Ok(());
    };
    let mode = text(&old, "mode").unwrap_or_else(|| "domain".into());
    let name = text(&old, "name").unwrap_or_else(|| "RETRONET".into());

    let mut domain = Mapping::new();
    domain.insert("backend".into(), "nt4".into());
    if let Some(v) = old.get("roaming_profiles") {
        domain.insert("roaming_profiles".into(), v.clone());
    }
    if let Some(v) = old.get("machines") {
        domain.insert("machines".into(), v.clone());
    }

    let mut server = Mapping::new();
    server.insert("name".into(), LEGACY.into());
    // the old play read only the network of this section (and served WINS whatever the
    // components entry listed), so nothing else is inferred: the install behaves as before
    let network = text(&old, "network");
    if let Some(network) = network {
        server.insert("network".into(), network.into());
    }
    if let Some(v) = text(&old, "netbios_name") {
        server.insert("netbios_name".into(), v.into());
    }
    if mode == "workgroup" {
        server.insert("role".into(), "workgroup".into());
        server.insert("workgroup".into(), name.as_str().into());
    } else {
        server.insert("role".into(), "pdc".into());
        server.insert("domain".into(), name.as_str().into());
    }
    if let Some(v) = text(&old, "address") {
        server.insert("address".into(), v.into());
    }
    server.insert("wins".into(), "serve".into());
    if let Some(v) = old.get("software") {
        server.insert("software".into(), v.clone());
    }

    let mut domains = match settings.section(DOMAINS) {
        Some(Value::Mapping(m)) => m.clone(),
        _ => Mapping::new(),
    };
    domains
        .entry(name.as_str().into())
        .or_insert(Value::Mapping(domain));

    let mut rest = old;
    for key in MOVED {
        rest.remove(key);
    }
    settings.set_section(LEGACY, Value::Mapping(rest));
    settings.set_section(DOMAINS, Value::Mapping(domains));
    settings.set_section(SERVERS, Value::Sequence(vec![Value::Mapping(server)]));
    Ok(())
}

/// the effective network of a server: its own, or the default one, in lower case.
fn network_of(server: &Mapping, default_network: &str) -> String {
    text(server, "network")
        .unwrap_or_else(|| default_network.to_string())
        .to_ascii_lowercase()
}

/// everything wrong with the domains and the list of servers in `root` (the settings document).
/// `default_network` is where a server with no `network` goes: the first network.
pub fn validate(root: &Mapping, default_network: &str) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    let domains: Vec<String> = match root.get(DOMAINS) {
        Some(Value::Mapping(map)) => map
            .keys()
            .filter_map(Value::as_str)
            .map(str::to_ascii_uppercase)
            .collect(),
        _ => Vec::new(),
    };
    check_domain_names(root, &mut errors);
    let Some(Value::Sequence(servers)) = root.get(SERVERS) else {
        return errors;
    };
    let mut seen = Seen::default();
    for (i, server) in servers.iter().enumerate() {
        let Value::Mapping(server) = server else {
            errors.push(error(format!("{SERVERS}[{i}]"), "must be a mapping".into()));
            continue;
        };
        let network = network_of(server, default_network);
        let one = One {
            i,
            server,
            network: &network,
        };
        check_name(&one, &mut seen, &mut errors);
        check_netbios(&one, &mut seen, &mut errors);
        check_role(&one, &domains, &mut seen, &mut errors);
        check_address(&one, &mut seen, &mut errors);
        check_wins(&one, &mut seen, &mut errors);
    }
    errors
}

fn error(path: String, message: String) -> ValidationError {
    ValidationError { path, message }
}

/// what the servers seen so far hold, to find the next one that clashes with them.
#[derive(Default)]
struct Seen {
    names: BTreeMap<String, usize>,
    netbios: BTreeMap<(String, String), usize>,
    pdcs: BTreeMap<(String, String), usize>,
    wins: BTreeMap<String, usize>,
    addresses: BTreeMap<(String, String), usize>,
}

/// the server being checked: its place in the list, its entry and its network.
struct One<'a> {
    i: usize,
    server: &'a Mapping,
    network: &'a str,
}

impl One<'_> {
    fn at(&self, key: &str) -> String {
        format!("{SERVERS}[{}].{key}", self.i)
    }
}

const NETBIOS_RULE: &str = "up to 15 letters, digits, - or _, starting with a letter or digit";

fn check_domain_names(root: &Mapping, errors: &mut Vec<ValidationError>) {
    let names = root
        .get(DOMAINS)
        .and_then(Value::as_mapping)
        .into_iter()
        .flat_map(|m| m.keys().filter_map(Value::as_str));
    for name in names.filter(|n| !crate::nt4::netbios_ok(n)) {
        errors.push(error(
            format!("{DOMAINS}.{name}"),
            "a domain name is up to 15 letters, digits, - or _".into(),
        ));
    }
}

fn check_name(one: &One, seen: &mut Seen, errors: &mut Vec<ValidationError>) {
    let name = text(one.server, "name").unwrap_or_default();
    let lower = name.to_ascii_lowercase();
    if name.is_empty() {
        errors.push(error(one.at("name"), "a server needs a name".into()));
    } else if RESERVED.contains(&lower.as_str()) || lower.ends_with("-prev") {
        errors.push(error(
            one.at("name"),
            format!("{name} is kept for other uses"),
        ));
    } else if let Some(first) = seen.names.insert(lower, one.i) {
        errors.push(error(
            one.at("name"),
            format!("{name} is also the name of server {first}"),
        ));
    }
}

fn check_netbios(one: &One, seen: &mut Seen, errors: &mut Vec<ValidationError>) {
    let given = text(one.server, "netbios_name");
    if given.as_deref().is_some_and(|n| !crate::nt4::netbios_ok(n)) {
        errors.push(error(one.at("netbios_name"), NETBIOS_RULE.into()));
        return;
    }
    // no name means the machine's own, which two servers cannot share either
    let key = (
        one.network.to_string(),
        given.unwrap_or_default().to_ascii_uppercase(),
    );
    if let Some(first) = seen.netbios.insert(key, one.i) {
        errors.push(error(
            one.at("netbios_name"),
            format!("server {first} has the same NetBIOS name on this network"),
        ));
    }
}

fn check_role(one: &One, domains: &[String], seen: &mut Seen, errors: &mut Vec<ValidationError>) {
    match text(one.server, "role").unwrap_or_default().as_str() {
        "pdc" => {
            let domain = text(one.server, "domain").unwrap_or_default();
            let upper = domain.to_ascii_uppercase();
            if !domains.contains(&upper) {
                errors.push(error(
                    one.at("domain"),
                    format!("there is no domain {domain}"),
                ));
            } else if let Some(first) = seen.pdcs.insert((one.network.to_string(), upper), one.i) {
                errors.push(error(
                    one.at("domain"),
                    format!("server {first} is already the domain controller of {domain} on this network"),
                ));
            }
        }
        "workgroup" => {
            let group = text(one.server, "workgroup").unwrap_or_default();
            if !crate::nt4::netbios_ok(&group) {
                errors.push(error(one.at("workgroup"), NETBIOS_RULE.into()));
            }
        }
        _ => errors.push(error(one.at("role"), "must be pdc or workgroup".into())),
    }
}

/// servers run on the host network: each one needs its own address on the network. no address
/// means the gateway's, which only one server can have.
fn check_address(one: &One, seen: &mut Seen, errors: &mut Vec<ValidationError>) {
    let address = text(one.server, "address")
        .map(|a| a.split('/').next().unwrap_or_default().to_string())
        .unwrap_or_default();
    let key = (one.network.to_string(), address.clone());
    let Some(first) = seen.addresses.insert(key, one.i) else {
        return;
    };
    let which = if address.is_empty() {
        "the gateway's address".to_string()
    } else {
        format!("the address {address}")
    };
    errors.push(error(
        one.at("address"),
        format!("server {first} already uses {which} on this network; give this server an address of its own"),
    ));
}

fn check_wins(one: &One, seen: &mut Seen, errors: &mut Vec<ValidationError>) {
    if text(one.server, "wins").unwrap_or_else(|| "serve".into()) != "serve" {
        return;
    }
    if let Some(first) = seen.wins.insert(one.network.to_string(), one.i) {
        errors.push(error(
            one.at("wins"),
            format!(
                "server {first} already serves WINS on this network; set wins: off on one of them"
            ),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(text: &str) -> Mapping {
        match serde_yaml_ng::from_str::<Value>(text).unwrap() {
            Value::Mapping(m) => m,
            _ => panic!("not a mapping"),
        }
    }

    fn errors(text: &str) -> Vec<String> {
        validate(&root(text), "lab")
            .into_iter()
            .map(|e| format!("{}: {}", e.path, e.message))
            .collect()
    }

    const DOMAIN: &str = "domains:\n  RETRODOM: {}\n";

    #[test]
    fn a_clean_list_has_no_errors() {
        let text = format!(
            "{DOMAIN}nt4-servers:\n  - {{name: a, role: pdc, domain: RETRODOM, netbios_name: ONE, network: lab}}\n  - {{name: b, role: workgroup, workgroup: WG, netbios_name: TWO, network: lab, address: 192.0.2.5/24, wins: off}}\n"
        );
        assert_eq!(errors(&text), Vec::<String>::new());
    }

    #[test]
    fn server_names_are_unique() {
        let text = format!(
            "{DOMAIN}nt4-servers:\n  - {{name: a, role: pdc, domain: RETRODOM, netbios_name: ONE, network: lab}}\n  - {{name: A, role: workgroup, workgroup: WG, netbios_name: TWO, network: lab2}}\n"
        );
        let found = errors(&text);
        assert!(
            found.iter().any(|e| e.starts_with("nt4-servers[1].name")),
            "{found:?}"
        );
    }

    #[test]
    fn netbios_names_are_unique_per_network_only() {
        let same = "nt4-servers:\n  - {name: a, role: workgroup, workgroup: WG, netbios_name: ONE, network: lab}\n  - {name: b, role: workgroup, workgroup: WG, netbios_name: one, network: lab, wins: off}\n";
        assert!(errors(same).iter().any(|e| e.contains("same NetBIOS name")));
        let apart = "nt4-servers:\n  - {name: a, role: workgroup, workgroup: WG, netbios_name: ONE, network: lab}\n  - {name: b, role: workgroup, workgroup: WG, netbios_name: ONE, network: other}\n";
        assert_eq!(errors(apart), Vec::<String>::new());
    }

    #[test]
    fn one_pdc_per_domain_per_network() {
        let two = format!(
            "{DOMAIN}nt4-servers:\n  - {{name: a, role: pdc, domain: RETRODOM, netbios_name: ONE, network: lab}}\n  - {{name: b, role: pdc, domain: retrodom, netbios_name: TWO, network: lab, wins: off}}\n"
        );
        assert!(errors(&two).iter().any(|e| e.contains("domain controller")));
        let apart = format!(
            "{DOMAIN}nt4-servers:\n  - {{name: a, role: pdc, domain: RETRODOM, netbios_name: ONE, network: lab}}\n  - {{name: b, role: pdc, domain: RETRODOM, netbios_name: TWO, network: other}}\n"
        );
        assert_eq!(errors(&apart), Vec::<String>::new());
    }

    #[test]
    fn one_wins_server_per_network() {
        let text = "nt4-servers:\n  - {name: a, role: workgroup, workgroup: WG, netbios_name: ONE, network: lab}\n  - {name: b, role: workgroup, workgroup: WG, netbios_name: TWO, network: lab}\n";
        assert!(errors(text).iter().any(|e| e.contains("WINS")));
    }

    #[test]
    fn two_servers_cannot_share_an_address_on_a_network() {
        let gateway = "nt4-servers:\n  - {name: a, role: workgroup, workgroup: WG, netbios_name: ONE, network: lab}\n  - {name: b, role: workgroup, workgroup: WG, netbios_name: TWO, network: lab, wins: off}\n";
        let found = errors(gateway);
        assert!(
            found
                .iter()
                .any(|e| e.starts_with("nt4-servers[1].address") && e.contains("gateway's")),
            "{found:?}"
        );
        let same = "nt4-servers:\n  - {name: a, role: workgroup, workgroup: WG, netbios_name: ONE, network: lab, address: 192.0.2.5/24}\n  - {name: b, role: workgroup, workgroup: WG, netbios_name: TWO, network: lab, address: 192.0.2.5/24, wins: off}\n";
        assert!(errors(same).iter().any(|e| e.contains("192.0.2.5")));
        let apart = "nt4-servers:\n  - {name: a, role: workgroup, workgroup: WG, netbios_name: ONE, network: lab}\n  - {name: b, role: workgroup, workgroup: WG, netbios_name: TWO, network: lab, address: 192.0.2.5/24, wins: off}\n  - {name: c, role: workgroup, workgroup: WG, netbios_name: THREE, network: other}\n";
        assert_eq!(errors(apart), Vec::<String>::new());
    }

    #[test]
    fn a_pdc_needs_its_domain() {
        let text =
            "nt4-servers:\n  - {name: a, role: pdc, domain: NOPE, netbios_name: ONE, network: lab}\n";
        assert!(errors(text)
            .iter()
            .any(|e| e.contains("there is no domain NOPE")));
    }

    #[test]
    fn netbios_names_are_at_most_15_characters() {
        let text = "nt4-servers:\n  - {name: a, role: workgroup, workgroup: WG, netbios_name: ABCDEFGHIJKLMNOP, network: lab}\n";
        assert!(errors(text)
            .iter()
            .any(|e| e.starts_with("nt4-servers[0].netbios_name")));
    }

    #[test]
    fn reserved_names_are_refused() {
        let text = "nt4-servers:\n  - {name: lib, role: workgroup, workgroup: WG, netbios_name: ONE, network: lab}\n";
        assert!(errors(text)
            .iter()
            .any(|e| e.contains("kept for other uses")));
    }

    fn migrated(text: &str) -> Settings {
        let mut settings = Settings::parse(text).unwrap();
        migrate_legacy(&mut settings, &Context::default()).unwrap();
        settings
    }

    #[test]
    fn a_workgroup_migrates_to_a_workgroup_server() {
        let settings = migrated("nt4-domain:\n  mode: workgroup\n  name: HOME\n  network: lab\n");
        let server = &settings.section(SERVERS).unwrap()[0];
        assert_eq!(server["role"], "workgroup");
        assert_eq!(server["workgroup"], "HOME");
        assert_eq!(server["name"], LEGACY);
    }

    #[test]
    fn the_components_entry_changes_neither_the_network_nor_wins() {
        // the old play served WINS and read only the section's own network
        let settings = migrated(
            "components:\n  nt4-domain:\n    attach: [{network: lab}]\n    subcomponents: [files]\nnt4-domain:\n  mode: domain\n  name: RETRODOM\n",
        );
        let server = &settings.section(SERVERS).unwrap()[0];
        assert!(server.get("network").is_none());
        assert_eq!(server["wins"], "serve");
    }

    #[test]
    fn migrating_twice_changes_nothing() {
        let once = migrated("nt4-domain:\n  mode: domain\n  name: RETRODOM\n  machines: [ws1]\n");
        let twice = migrated(&once.text());
        assert_eq!(once.text(), twice.text());
        assert!(!has_legacy(&once));
    }
}
