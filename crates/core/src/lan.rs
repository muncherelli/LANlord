//! edit the lanlord networks on a server: add, change and remove them, and set their static
//! leases. the networks live in the plain settings (see `lanlord_model::lan_networks`); every
//! change is checked, stored and then applied with the docker module (the address, mtu and
//! firewall) and the dns module (one dnsmasq per network).

use anyhow::{bail, Result};
pub use lanlord_model::lan_edit::{
    add_edit, apply_edit, edit_settings, edit_settings_on, parse_range, set_edit, AddArgs, Change,
    Edit, SetArgs,
};
use lanlord_model::lan_networks::{self, Networks};
use lanlord_model::settings::Settings;

use crate::apply;
use crate::bootstrap::AskPassword;
use crate::settings;
use crate::ssh::Session;
use crate::users::cached;

/// the modules a network change touches: docker keeps the address, mtu and firewall, dns runs
/// the dnsmasq of every network.
pub const MODULES: [&str; 2] = ["docker", "dns"];

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

/// the networks in the plain settings text.
pub fn read_networks(plain: &str) -> Result<Networks> {
    lan_networks::read(&Settings::parse(plain)?)
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
    // the card the default route leaves through is the external network's, when the settings do
    // not say which it is
    let card = crate::server::read(session)
        .ok()
        .and_then(|f| f.interfaces.into_iter().find(|i| i.default_route))
        .map(|i| i.name);
    let edited = edit_settings_on(&stored.plain, edit, card.as_deref())?;
    settings::save_document_with(session, host, identity, &edited, &[], &mut ask)?;
    let modules = MODULES.map(String::from);
    apply::apply_with(session, host, Some(identity), &modules, &mut ask)
        .map_err(|e| anyhow::anyhow!("Saved, but applying failed: {e}. Fix it and click Apply."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanlord_model::lan_networks::LanNetwork;
    use lanlord_model::settings::StaticLease;

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
    fn a_network_that_is_still_used_cannot_be_removed() {
        let base = edit_settings(OLD, &Edit::Add(lab())).unwrap();
        let domain = format!("{base}nt4-domain:\n  network: lab\n");
        let error = edit_settings(&domain, &Edit::Remove("lab".into())).unwrap_err();
        assert!(
            error.to_string().contains("the nt4-domain network"),
            "{error}"
        );
        let attached =
            format!("{base}components:\n  files:\n    attach:\n      - network: lab\n        address: 192.168.50.5\n");
        let error = edit_settings(&attached, &Edit::Remove("lab".into())).unwrap_err();
        assert!(error.to_string().contains("the component files"), "{error}");
        assert!(edit_settings(&base, &Edit::Remove("lab".into())).is_ok());
    }

    #[test]
    fn the_default_route_card_is_not_a_lan_interface_when_the_settings_do_not_name_it() {
        // eth0 carries the default route and the settings do not say so
        let edit = Edit::Add(LanNetwork::new("lab", "eth0", "192.168.50.1/24"));
        let error = edit_settings_on("", &edit, Some("eth0")).unwrap_err();
        assert!(error.to_string().contains("eth0"), "{error}");
        assert!(edit_settings_on("", &edit, None).is_ok());
        // a card that a network already uses stays editable, whatever the route says
        let base = edit_settings_on("", &edit, None).unwrap();
        let set = Edit::Set {
            name: "lab".into(),
            change: Change {
                mtu: Some(Some(1400)),
                ..Change::default()
            },
        };
        assert!(edit_settings_on(&base, &set, Some("eth0")).is_ok());
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
