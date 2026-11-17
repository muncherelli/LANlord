//! edit the lanlord networks on a server: add, change and remove them, and set their static
//! leases. the networks live in the plain settings (see `lanlord_model::lan_networks`); every
//! change is checked, stored and then applied with the docker module (the address, mtu and
//! firewall) and the dns module (one dnsmasq per network).

use anyhow::{anyhow, bail, Result};
use lanlord_model::lan_networks::{self, find, LanNetwork, Networks};
use lanlord_model::settings::{Dhcp, Settings, StaticLease};
use serde_yaml_ng::Value;

use crate::apply;
use crate::bootstrap::AskPassword;
use crate::settings;
use crate::ssh::Session;
use crate::users::cached;

/// the modules a network change touches: docker keeps the address, mtu and firewall, dns runs
/// the dnsmasq of every network.
pub const MODULES: [&str; 2] = ["docker", "dns"];

/// the parts of a network a `set` may change; what is `None` stays as it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Change {
    pub interface: Option<String>,
    pub address: Option<String>,
    /// `Some(None)` takes the mtu setting away.
    pub mtu: Option<Option<u32>>,
    pub zone: Option<String>,
    pub dhcp_range: Option<(String, String)>,
    pub lease: Option<String>,
    /// turn the dhcp server off.
    pub dhcp_off: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    Add(LanNetwork),
    Set { name: String, change: Change },
    Remove(String),
    LeaseAdd { network: String, lease: StaticLease },
    LeaseRemove { network: String, host: String },
}

fn missing(name: &str) -> anyhow::Error {
    anyhow!("there is no network called {name}")
}

fn dhcp_of(network: &mut LanNetwork) -> &mut Dhcp {
    network.dhcp.get_or_insert_with(Dhcp::default)
}

fn set_parts(network: &mut LanNetwork, change: &Change) -> Result<()> {
    if let Some(interface) = &change.interface {
        network.interface = interface.clone();
    }
    if let Some(address) = &change.address {
        network.address = address.clone();
    }
    if let Some(mtu) = change.mtu {
        network.mtu = mtu;
    }
    if let Some(zone) = &change.zone {
        network.zone = zone.to_ascii_lowercase();
    }
    if change.dhcp_off {
        if change.dhcp_range.is_some() || change.lease.is_some() {
            bail!("a network cannot both have its dhcp turned off and set");
        }
        network.dhcp = None;
    }
    if let Some((start, end)) = &change.dhcp_range {
        let dhcp = dhcp_of(network);
        dhcp.range_start = start.clone();
        dhcp.range_end = end.clone();
    }
    if let Some(lease) = &change.lease {
        match network.dhcp.as_mut() {
            Some(dhcp) => dhcp.lease = lease.clone(),
            None => bail!("this network has no dhcp server; give it a range first"),
        }
    }
    Ok(())
}

/// apply one edit to a list of networks.
pub fn apply_edit(list: &mut Vec<LanNetwork>, edit: &Edit) -> Result<()> {
    match edit {
        Edit::Add(network) => {
            if find(list, &network.name).is_some() {
                bail!("there is already a network called {}", network.name);
            }
            list.push(network.clone());
        }
        Edit::Set { name, change } => {
            let network = list
                .iter_mut()
                .find(|n| n.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| missing(name))?;
            set_parts(network, change)?;
        }
        Edit::Remove(name) => {
            let before = list.len();
            list.retain(|n| !n.name.eq_ignore_ascii_case(name));
            if list.len() == before {
                return Err(missing(name));
            }
        }
        Edit::LeaseAdd { network, lease } => {
            let net = list
                .iter_mut()
                .find(|n| n.name.eq_ignore_ascii_case(network))
                .ok_or_else(|| missing(network))?;
            let dhcp = net
                .dhcp
                .as_mut()
                .ok_or_else(|| anyhow!("{network} has no dhcp server; give it a range first"))?;
            // a lease for a name or mac already there replaces it
            dhcp.hosts.retain(|h| {
                !h.name.eq_ignore_ascii_case(&lease.name) && !h.mac.eq_ignore_ascii_case(&lease.mac)
            });
            dhcp.hosts.push(lease.clone());
        }
        Edit::LeaseRemove { network, host } => {
            let net = list
                .iter_mut()
                .find(|n| n.name.eq_ignore_ascii_case(network))
                .ok_or_else(|| missing(network))?;
            let hosts = net.dhcp.as_mut().map(|d| &mut d.hosts);
            let Some(hosts) = hosts else {
                bail!("{network} has no dhcp server");
            };
            let before = hosts.len();
            hosts.retain(|h| !h.name.eq_ignore_ascii_case(host));
            if hosts.len() == before {
                bail!("{network} has no static lease for {host}");
            }
        }
    }
    Ok(())
}

/// `172.16.120.100-172.16.120.200` as its two ends.
pub fn parse_range(text: &str) -> Result<(String, String)> {
    match text.split_once('-') {
        Some((start, end)) if !start.trim().is_empty() && !end.trim().is_empty() => {
            Ok((start.trim().to_string(), end.trim().to_string()))
        }
        _ => bail!("the dhcp range must look like 172.16.120.100-172.16.120.200"),
    }
}

/// what `lanlord networks add` asks for.
#[derive(Clone, Debug, Default)]
pub struct AddArgs {
    pub name: String,
    pub interface: String,
    pub address: String,
    pub mtu: Option<u32>,
    pub dhcp_range: Option<String>,
    pub lease: Option<String>,
    pub zone: Option<String>,
}

/// the edit that adds the network `args` describes.
pub fn add_edit(args: &AddArgs) -> Result<Edit> {
    let mut network = LanNetwork::new(&args.name, &args.interface, &args.address);
    network.mtu = args.mtu;
    if let Some(zone) = &args.zone {
        network.zone = zone.to_ascii_lowercase();
    }
    match (&args.dhcp_range, &args.lease) {
        (Some(range), lease) => {
            let (start, end) = parse_range(range)?;
            network.dhcp = Some(Dhcp {
                range_start: start,
                range_end: end,
                lease: lease.clone().unwrap_or_else(|| "7d".into()),
                hosts: Vec::new(),
            });
        }
        (None, Some(_)) => bail!("a lease time needs a dhcp range"),
        (None, None) => {}
    }
    Ok(Edit::Add(network))
}

/// what `lanlord networks set` asks for; flags that take something away are separate.
#[derive(Clone, Debug, Default)]
pub struct SetArgs {
    pub interface: Option<String>,
    pub address: Option<String>,
    pub mtu: Option<u32>,
    pub no_mtu: bool,
    pub zone: Option<String>,
    pub dhcp_range: Option<String>,
    pub lease: Option<String>,
    pub no_dhcp: bool,
}

/// the edit that changes the network `name` as `args` says.
pub fn set_edit(name: &str, args: &SetArgs) -> Result<Edit> {
    if args.mtu.is_some() && args.no_mtu {
        bail!("give an mtu or take it away, not both");
    }
    let change = Change {
        interface: args.interface.clone(),
        address: args.address.clone(),
        mtu: if args.no_mtu {
            Some(None)
        } else {
            args.mtu.map(Some)
        },
        zone: args.zone.clone(),
        dhcp_range: args.dhcp_range.as_deref().map(parse_range).transpose()?,
        lease: args.lease.clone(),
        dhcp_off: args.no_dhcp,
    };
    if change == Change::default() {
        bail!("nothing to change: give at least one of --interface, --address, --mtu, --zone, --dhcp-range, --lease or --no-dhcp");
    }
    Ok(Edit::Set {
        name: name.to_string(),
        change,
    })
}

impl Edit {
    /// a sentence for what the edit does.
    pub fn describe(&self) -> String {
        match self {
            Edit::Add(n) => format!(
                "added the network {} on {} ({})",
                n.name, n.interface, n.address
            ),
            Edit::Set { name, .. } => format!("changed the network {name}"),
            Edit::Remove(name) => format!("removed the network {name}"),
            Edit::LeaseAdd { network, lease } => {
                format!(
                    "{network}: {} ({}) gets {}",
                    lease.name, lease.mac, lease.address
                )
            }
            Edit::LeaseRemove { network, host } => {
                format!("{network}: {host} no longer has a static lease")
            }
        }
    }
}

/// the lines `lanlord networks list` prints.
pub fn render_list(networks: &Networks) -> Vec<String> {
    let mut lines = Vec::new();
    if networks.list.is_empty() {
        lines.push("no lanlord networks".to_string());
    }
    for n in &networks.list {
        let dhcp = match &n.dhcp {
            Some(d) => format!(
                "dhcp {}-{} ({}), {} static",
                d.range_start,
                d.range_end,
                d.lease,
                d.hosts.len()
            ),
            None => "no dhcp".to_string(),
        };
        let interface = if n.interface.is_empty() {
            "(detected)"
        } else {
            &n.interface
        };
        let address = if n.address.is_empty() {
            "(detected)"
        } else {
            &n.address
        };
        let mtu = n.mtu.map(|m| format!(", mtu {m}")).unwrap_or_default();
        lines.push(format!(
            "{}: {interface} {address}{mtu}, zone {}, {dhcp}",
            n.name, n.zone
        ));
    }
    if networks.legacy {
        lines.push("(read from the older single-network settings; the first change writes a networks list)".to_string());
    }
    lines
}

/// the interface of the normal network, when the settings say which it is.
fn external_interface(settings: &Settings) -> Option<String> {
    for section in ["network", "docker"] {
        if let Some(Value::Mapping(map)) = settings.section(section) {
            if let Some(Value::String(name)) = map.get("host_interface") {
                return Some(name.clone());
            }
        }
    }
    None
}

/// the settings text after `edit`, checked. the networks are always written as a `networks:`
/// list, so a server with the older single-network settings is migrated by its first change.
pub fn edit_settings(plain: &str, edit: &Edit) -> Result<String> {
    let mut settings = Settings::parse(plain)?;
    let Networks { mut list, .. } = lan_networks::read(&settings)?;
    apply_edit(&mut list, edit)?;
    let errors = lan_networks::validate(&list, external_interface(&settings).as_deref(), true);
    if !errors.is_empty() {
        bail!(
            "{}",
            errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    lan_networks::write(&mut settings, &list);
    // the docker module keeps the addresses and the firewall, so the section has to exist
    if settings.section("docker").is_none() {
        settings.set_section("docker", Value::Mapping(serde_yaml_ng::Mapping::new()));
    }
    Ok(settings.text())
}

/// the networks stored on the server.
pub fn networks_on(
    session: &Session,
    host: &str,
    identity: &age::x25519::Identity,
    ask_password: AskPassword,
) -> Result<Networks> {
    let stored = settings::load_document_with(session, identity, ask_password)?;
    if stored == settings::Document::default() {
        bail!("no settings on {host}; run lanlord config push first");
    }
    lan_networks::read(&Settings::parse(&stored.plain)?)
}

/// make `edit` in the server's settings and apply the docker and dns modules.
pub fn change(
    session: &Session,
    host: &str,
    identity: &age::x25519::Identity,
    edit: &Edit,
    ask_password: AskPassword,
) -> Result<()> {
    let mut ask = cached(ask_password);
    let stored = settings::load_document_with(session, identity, &mut ask)?;
    if stored == settings::Document::default() {
        bail!("no settings on {host}; run lanlord config push first");
    }
    let edited = edit_settings(&stored.plain, edit)?;
    settings::save_document_with(session, host, identity, &edited, &[], &mut ask)?;
    let modules = MODULES.map(String::from);
    apply::apply_with(session, host, Some(identity), &modules, &mut ask)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = "docker:\n  internal_address: 172.16.120.1/22\n  internal_interface: eth1\ndns:\n  zone: retronet\n  dhcp:\n    range: [172.16.123.1, 172.16.123.254]\n";

    fn lab() -> LanNetwork {
        LanNetwork::new("lab", "eth2", "192.168.50.1/24")
    }

    fn lease(name: &str, mac: &str, address: &str) -> StaticLease {
        StaticLease {
            name: name.into(),
            mac: mac.into(),
            address: address.into(),
        }
    }

    fn list_after(text: &str) -> Vec<LanNetwork> {
        lan_networks::read(&Settings::parse(text).unwrap())
            .unwrap()
            .list
    }

    #[test]
    fn the_first_change_migrates_the_older_settings() {
        let text = edit_settings(OLD, &Edit::Add(lab())).unwrap();
        assert!(
            !text.contains("\ndns:") && !text.contains("internal_address"),
            "{text}"
        );
        let list = list_after(&text);
        assert_eq!(
            list.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
            ["retronet", "lab"]
        );
        assert_eq!(list[0].dhcp.as_ref().unwrap().range_end, "172.16.123.254");
    }

    #[test]
    fn a_first_network_creates_the_docker_section() {
        let text = edit_settings("gateway: {}\n", &Edit::Add(lab())).unwrap();
        assert!(text.contains("docker:"), "{text}");
        assert_eq!(list_after(&text).len(), 1);
    }

    #[test]
    fn a_network_is_changed_in_parts() {
        let one = edit_settings("docker: {}\n", &Edit::Add(lab())).unwrap();
        let change = Change {
            mtu: Some(Some(1400)),
            dhcp_range: Some(("192.168.50.100".into(), "192.168.50.200".into())),
            lease: Some("12h".into()),
            zone: Some("Lab".into()),
            ..Change::default()
        };
        let two = edit_settings(
            &one,
            &Edit::Set {
                name: "LAB".into(),
                change,
            },
        )
        .unwrap();
        let net = &list_after(&two)[0];
        assert_eq!(net.mtu, Some(1400));
        assert_eq!(net.zone, "lab");
        let dhcp = net.dhcp.as_ref().unwrap();
        assert_eq!(
            (dhcp.range_start.as_str(), dhcp.lease.as_str()),
            ("192.168.50.100", "12h")
        );
        let off = Change {
            dhcp_off: true,
            mtu: Some(None),
            ..Change::default()
        };
        let three = edit_settings(
            &two,
            &Edit::Set {
                name: "lab".into(),
                change: off,
            },
        )
        .unwrap();
        let net = &list_after(&three)[0];
        assert!(net.dhcp.is_none() && net.mtu.is_none());
    }

    #[test]
    fn a_lease_needs_a_dhcp_server_and_replaces_a_matching_one() {
        let one = edit_settings("docker: {}\n", &Edit::Add(lab())).unwrap();
        let add = |text: &str, l: StaticLease| {
            edit_settings(
                text,
                &Edit::LeaseAdd {
                    network: "lab".into(),
                    lease: l,
                },
            )
        };
        assert!(add(&one, lease("pc", "02:00:00:00:00:01", "192.168.50.20")).is_err());
        let ranged = edit_settings(
            &one,
            &Edit::Set {
                name: "lab".into(),
                change: Change {
                    dhcp_range: Some(("192.168.50.100".into(), "192.168.50.200".into())),
                    ..Change::default()
                },
            },
        )
        .unwrap();
        let first = add(&ranged, lease("pc", "02:00:00:00:00:01", "192.168.50.20")).unwrap();
        // the same name with another address replaces the lease
        let second = add(&first, lease("PC", "02:00:00:00:00:02", "192.168.50.21")).unwrap();
        let nets = list_after(&second);
        let hosts = &nets[0].dhcp.as_ref().unwrap().hosts;
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].address, "192.168.50.21");
        let gone = edit_settings(
            &second,
            &Edit::LeaseRemove {
                network: "lab".into(),
                host: "pc".into(),
            },
        )
        .unwrap();
        assert!(list_after(&gone)[0].dhcp.as_ref().unwrap().hosts.is_empty());
        assert!(edit_settings(
            &gone,
            &Edit::LeaseRemove {
                network: "lab".into(),
                host: "pc".into()
            }
        )
        .is_err());
    }

    #[test]
    fn bad_edits_are_refused_with_what_is_wrong() {
        let two = edit_settings("docker: {}\n", &Edit::Add(lab())).unwrap();
        // the same name, an overlapping network and an unknown network
        assert!(edit_settings(&two, &Edit::Add(lab()))
            .unwrap_err()
            .to_string()
            .contains("already"));
        let overlap = LanNetwork::new("b", "eth3", "192.168.50.129/25");
        assert!(edit_settings(&two, &Edit::Add(overlap))
            .unwrap_err()
            .to_string()
            .contains("overlaps"));
        assert!(edit_settings(&two, &Edit::Remove("x".into()))
            .unwrap_err()
            .to_string()
            .contains("no network"));
        let ghost = Edit::Set {
            name: "x".into(),
            change: Change::default(),
        };
        assert!(edit_settings(&two, &ghost).is_err());
        // the normal network's interface is off limits when the settings name it
        let with_host = "network:\n  host_interface: eth0\n";
        let on_host = LanNetwork::new("c", "eth0", "10.9.0.1/24");
        assert!(edit_settings(with_host, &Edit::Add(on_host))
            .unwrap_err()
            .to_string()
            .contains("external"));
        // an older network without an interface must be given one before it can be written
        let err = edit_settings("dns: {}\n", &Edit::Add(lab())).unwrap_err();
        assert!(err.to_string().contains("interface"), "{err}");
        let fix = Edit::Set {
            name: "retronet".into(),
            change: Change {
                interface: Some("eth1".into()),
                address: Some("172.16.120.1/22".into()),
                ..Change::default()
            },
        };
        assert!(edit_settings("dns: {}\n", &fix).is_ok());
        // dhcp cannot be turned off and set at once, nor given a lease without a server
        let both = Change {
            dhcp_off: true,
            lease: Some("1d".into()),
            ..Change::default()
        };
        assert!(edit_settings(
            &two,
            &Edit::Set {
                name: "lab".into(),
                change: both
            }
        )
        .is_err());
        let lease_only = Change {
            lease: Some("1d".into()),
            ..Change::default()
        };
        assert!(edit_settings(
            &two,
            &Edit::Set {
                name: "lab".into(),
                change: lease_only
            }
        )
        .is_err());
    }

    #[test]
    fn arguments_become_edits() {
        assert_eq!(parse_range("10.0.0.10-10.0.0.20").unwrap().1, "10.0.0.20");
        assert!(parse_range("10.0.0.10").is_err() && parse_range("-10.0.0.20").is_err());
        let add = add_edit(&AddArgs {
            name: "lab".into(),
            interface: "eth2".into(),
            address: "192.168.50.1/24".into(),
            mtu: Some(1400),
            dhcp_range: Some("192.168.50.100-192.168.50.200".into()),
            lease: Some("12h".into()),
            zone: Some("Lab".into()),
        })
        .unwrap();
        let Edit::Add(net) = &add else {
            panic!("an add")
        };
        assert_eq!(
            (
                net.mtu,
                net.zone.as_str(),
                net.dhcp.as_ref().unwrap().lease.as_str()
            ),
            (Some(1400), "lab", "12h")
        );
        assert!(add.describe().contains("added the network lab on eth2"));
        let plain = add_edit(&AddArgs {
            name: "a".into(),
            interface: "eth1".into(),
            address: "10.0.0.1/24".into(),
            ..AddArgs::default()
        })
        .unwrap();
        assert!(matches!(&plain, Edit::Add(n) if n.dhcp.is_none()));
        let lease_only = AddArgs {
            lease: Some("1d".into()),
            ..AddArgs::default()
        };
        assert!(add_edit(&lease_only).is_err());
    }

    #[test]
    fn set_arguments_become_a_change_or_an_error() {
        assert!(set_edit("lab", &SetArgs::default()).is_err());
        assert!(set_edit(
            "lab",
            &SetArgs {
                mtu: Some(1400),
                no_mtu: true,
                ..SetArgs::default()
            }
        )
        .is_err());
        assert!(set_edit(
            "lab",
            &SetArgs {
                dhcp_range: Some("bad".into()),
                ..SetArgs::default()
            }
        )
        .is_err());
        let edit = set_edit(
            "lab",
            &SetArgs {
                no_mtu: true,
                no_dhcp: true,
                zone: Some("x".into()),
                ..SetArgs::default()
            },
        )
        .unwrap();
        let Edit::Set { name, change } = &edit else {
            panic!("a set")
        };
        assert_eq!(
            (name.as_str(), change.mtu, change.dhcp_off),
            ("lab", Some(None), true)
        );
        assert_eq!(edit.describe(), "changed the network lab");
        let with_mtu = set_edit(
            "lab",
            &SetArgs {
                mtu: Some(1400),
                ..SetArgs::default()
            },
        )
        .unwrap();
        assert!(matches!(
            with_mtu,
            Edit::Set {
                change: Change {
                    mtu: Some(Some(1400)),
                    ..
                },
                ..
            }
        ));
        assert_eq!(Edit::Remove("x".into()).describe(), "removed the network x");
        let l = Edit::LeaseAdd {
            network: "a".into(),
            lease: lease("pc", "02:00:00:00:00:01", "10.0.0.5"),
        };
        assert!(l
            .describe()
            .contains("pc (02:00:00:00:00:01) gets 10.0.0.5"));
        assert!(Edit::LeaseRemove {
            network: "a".into(),
            host: "pc".into()
        }
        .describe()
        .contains("no longer"));
    }

    #[test]
    fn the_list_says_what_each_network_has() {
        let old = lan_networks::read(&Settings::parse(OLD).unwrap()).unwrap();
        let lines = render_list(&old);
        assert!(
            lines[0].starts_with(
                "retronet: eth1 172.16.120.1/22, zone retronet, dhcp 172.16.123.1-172.16.123.254"
            ),
            "{lines:?}"
        );
        assert!(lines[1].contains("older single-network settings"));
        let bare = lan_networks::read(&Settings::parse("dns: {}\n").unwrap()).unwrap();
        assert!(render_list(&bare)[0].contains("(detected)"));
        let none = lan_networks::read(&Settings::empty()).unwrap();
        assert_eq!(render_list(&none), ["no lanlord networks"]);
        let mut with_mtu = lab();
        with_mtu.mtu = Some(1400);
        let networks = Networks {
            list: vec![with_mtu],
            legacy: false,
        };
        assert!(render_list(&networks)[0].contains(", mtu 1400, zone lab, no dhcp"));
    }

    #[test]
    fn removing_the_last_network_leaves_an_empty_list() {
        let one = edit_settings("docker: {}\n", &Edit::Add(lab())).unwrap();
        let none = edit_settings(&one, &Edit::Remove("lab".into())).unwrap();
        assert!(list_after(&none).is_empty());
        assert!(none.contains("networks"), "{none}");
    }

    #[cfg(unix)]
    mod gateway {
        use super::*;
        use crate::testgw::{identity, no_password, Gateway};

        #[test]
        fn a_change_is_stored_and_applied_and_the_list_is_read_back() {
            let (gw, id) = (Gateway::new("lan-change"), identity("lan-change"));
            gw.seed_sealed(&id, OLD);
            let before = networks_on(&gw.session, "gw", &id, &mut no_password).unwrap();
            assert!(before.legacy && before.list.len() == 1);
            change(&gw.session, "gw", &id, &Edit::Add(lab()), &mut no_password).unwrap();
            let plain = gw.plain().unwrap();
            assert!(
                plain.contains("networks:") && plain.contains("lab"),
                "{plain}"
            );
            let after = networks_on(&gw.session, "gw", &id, &mut no_password).unwrap();
            assert!(!after.legacy && after.list.len() == 2);
        }

        #[test]
        fn a_server_without_settings_is_refused_plainly() {
            let (gw, id) = (Gateway::new("lan-none"), identity("lan-none"));
            let err = networks_on(&gw.session, "gw", &id, &mut no_password).unwrap_err();
            assert!(err.to_string().contains("no settings"), "{err}");
            assert!(change(&gw.session, "gw", &id, &Edit::Add(lab()), &mut no_password).is_err());
        }

        #[test]
        fn a_refused_edit_changes_nothing_on_the_server() {
            let (gw, id) = (Gateway::new("lan-bad"), identity("lan-bad"));
            gw.seed_sealed(&id, OLD);
            let before = gw.plain();
            let bad = LanNetwork::new("x", "eth5", "172.16.121.1/24");
            assert!(change(&gw.session, "gw", &id, &Edit::Add(bad), &mut no_password).is_err());
            assert_eq!(gw.plain(), before);
        }
    }
}
