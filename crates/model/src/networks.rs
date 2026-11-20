//! the networks a server is on, for the Networks window: its external network (the normal
//! one, the way to the internet) and lanlord's own networks, read from the interfaces and the
//! settings, and the form that adds or changes one. nothing here changes anything: the form
//! only builds the [`Edit`] that does.

use std::net::Ipv4Addr;

use serde_yaml_ng::Value;

use crate::internet::Internet;
use crate::lan_edit::{add_edit, apply_edit, set_edit, AddArgs, Edit, SetArgs};
use crate::lan_networks::{self, split_address, LanNetwork};
use crate::server::Iface;
use crate::settings::{DnsSettings, Settings, Subnet};
use crate::ssh_access::SshAccess;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Role {
    External,
    Lanlord,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NetworkRow {
    pub name: String,
    pub role: Role,
    pub interface: String,
    /// the address with its prefix, or empty when the interface has none.
    pub address: String,
    pub mtu: u32,
    pub link: String,
    /// the dhcp range, for a lanlord network that serves one.
    pub dhcp: Option<String>,
    /// how many clients hold a lease, when it is known.
    pub leases: Option<usize>,
    /// who may go out to the internet, for a lanlord network.
    #[serde(default)]
    pub internet: Option<Internet>,
    /// who may reach the server's sshd, for a lanlord network.
    pub ssh: Option<SshAccess>,
}

impl NetworkRow {
    /// the lines of the read-only properties box.
    pub fn details(&self) -> Vec<(String, String)> {
        let mut lines = vec![
            ("Name".to_string(), self.name.clone()),
            (
                "Kind".to_string(),
                match self.role {
                    Role::External => "External network",
                    Role::Lanlord => "LANlord network",
                }
                .to_string(),
            ),
            ("Interface".to_string(), self.interface.clone()),
            ("Address".to_string(), self.address.clone()),
            ("MTU".to_string(), self.mtu.to_string()),
            ("Link".to_string(), self.link.clone()),
        ];
        if let Some(dhcp) = &self.dhcp {
            lines.push(("DHCP range".to_string(), dhcp.clone()));
        }
        if let Some(leases) = self.leases {
            lines.push(("Clients with a lease".to_string(), leases.to_string()));
        }
        if let Some(internet) = &self.internet {
            lines.extend(internet.details());
        }
        if let Some(ssh) = &self.ssh {
            lines.extend(ssh.details());
        }
        lines
    }
}

fn row(iface: &Iface, name: &str, role: Role) -> NetworkRow {
    NetworkRow {
        name: name.to_string(),
        role,
        interface: iface.name.clone(),
        address: iface.addresses.first().cloned().unwrap_or_default(),
        mtu: iface.mtu,
        link: iface.link().to_string(),
        dhcp: None,
        leases: None,
        internet: None,
        ssh: None,
    }
}

/// the address the docker module puts on the internal interface, like `172.16.120.1/22`.
fn internal_address(settings: &Settings) -> Option<String> {
    match settings.section("docker")? {
        Value::Mapping(map) => match map.get("internal_address")? {
            Value::String(address) => Some(address.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn has_address(iface: &Iface, address: &str) -> bool {
    iface.addresses.iter().any(|a| a == address)
}

/// the external network and, when the settings define one, the lanlord network, with its dhcp
/// range and the number of `leases` it has handed out.
pub fn rows(ifaces: &[Iface], settings: &Settings, leases: Option<usize>) -> Vec<NetworkRow> {
    // settings with a `networks:` list name every network and its interface
    if let Ok(networks) = lan_networks::read(settings) {
        if !networks.legacy && !networks.list.is_empty() {
            return listed_rows(ifaces, &networks.list);
        }
    }
    let internal = internal_address(settings)
        .and_then(|address| ifaces.iter().find(|i| has_address(i, &address)));
    let mut rows = Vec::new();
    let external = ifaces.iter().find(|i| i.default_route).or_else(|| {
        ifaces
            .iter()
            .find(|i| !i.loopback() && Some(*i) != internal)
    });
    if let Some(iface) = external {
        rows.push(row(iface, "External Network", Role::External));
    }
    if let Some(iface) = internal {
        let dns = DnsSettings::from_settings(settings).unwrap_or_default();
        let mut lanlord = row(iface, &dns.zone, Role::Lanlord);
        lanlord.dhcp = dns
            .dhcp
            .as_ref()
            .map(|d| format!("{} - {}", d.range_start, d.range_end));
        lanlord.leases = lanlord.dhcp.as_ref().and(leases);
        lanlord.internet = Some(Internet::default());
        lanlord.ssh = Some(SshAccess::default());
        rows.push(lanlord);
    }
    rows
}

/// the external network and every network in the settings' `networks:` list, on its own interface.
fn listed_rows(ifaces: &[Iface], list: &[lan_networks::LanNetwork]) -> Vec<NetworkRow> {
    let lan = |iface: &Iface| list.iter().any(|n| n.interface == iface.name);
    let mut rows = Vec::new();
    let external = ifaces
        .iter()
        .find(|i| i.default_route && !lan(i))
        // when the default route leaves through a lanlord network, take the first other
        // interface that has an address
        .or_else(|| {
            ifaces
                .iter()
                .find(|i| !i.loopback() && !lan(i) && !i.addresses.is_empty())
        });
    if let Some(iface) = external {
        rows.push(row(iface, "External Network", Role::External));
    }
    for network in list {
        let dhcp = network
            .dhcp
            .as_ref()
            .map(|d| format!("{} - {}", d.range_start, d.range_end));
        let mut entry = match ifaces.iter().find(|i| i.name == network.interface) {
            Some(iface) => row(iface, &network.name, Role::Lanlord),
            None => NetworkRow {
                name: network.name.clone(),
                role: Role::Lanlord,
                interface: network.interface.clone(),
                address: network.address.clone(),
                mtu: network.mtu.unwrap_or(0),
                link: "interface not found".to_string(),
                dhcp: None,
                leases: None,
                internet: None,
                ssh: None,
            },
        };
        entry.dhcp = dhcp;
        entry.internet = Some(network.internet.clone());
        entry.ssh = Some(network.ssh.clone());
        rows.push(entry);
    }
    rows
}

/// the cards no network uses: not the loopback, not the external network's and not a lanlord
/// network's.
pub fn unused<'a>(ifaces: &'a [Iface], rows: &[NetworkRow]) -> Vec<&'a Iface> {
    ifaces
        .iter()
        .filter(|i| !i.loopback() && !rows.iter().any(|r| r.interface == i.name))
        .collect()
}

/// the card a new network starts on: an unused one that is up, else the first unused one.
pub fn suggested_card<'a>(unused: &[&'a Iface]) -> Option<&'a Iface> {
    unused
        .iter()
        .find(|i| i.state == "UP")
        .or_else(|| unused.first())
        .copied()
}

/// a dhcp range inside the subnet of `address` that leaves out the server's own address: the
/// hundreds of a large subnet, the upper half of a small one. none when no range fits.
pub fn suggest_range(address: &str) -> Option<(String, String)> {
    let (ip, prefix) = split_address(address)?;
    let subnet = Subnet::new(ip, prefix);
    let first = u32::from(subnet.network);
    let last = u32::from(subnet.broadcast()) - 1;
    let size = 1u64 << (32 - u32::from(prefix));
    let (start, end) = if size >= 256 {
        (first + 100, first + 200)
    } else {
        (first + (size / 2) as u32, last)
    };
    let (start, end) = (Ipv4Addr::from(start), Ipv4Addr::from(end));
    let fits = start < end && end <= Ipv4Addr::from(last) && !(start..=end).contains(&ip);
    fits.then(|| (start.to_string(), end.to_string()))
}

/// the fields of the form that adds a network or changes one, as typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    pub name: String,
    pub interface: String,
    pub address: String,
    pub range_start: String,
    pub range_end: String,
    pub lease: String,
    /// blank means the name.
    pub zone: String,
    /// blank means the card's own.
    pub mtu: String,
}

impl Draft {
    /// a new network on `iface`: its address and mtu, and a range inside its subnet.
    pub fn on_card(iface: &Iface) -> Self {
        let mut draft = Self::default();
        draft.pick_card(iface);
        draft
    }

    /// move the draft to `iface`, taking its address and mtu and suggesting a range.
    pub fn pick_card(&mut self, iface: &Iface) {
        self.interface = iface.name.clone();
        self.address = iface.addresses.first().cloned().unwrap_or_default();
        self.mtu = iface.mtu.to_string();
        let (start, end) = suggest_range(&self.address).unwrap_or_default();
        (self.range_start, self.range_end) = (start, end);
        if self.lease.is_empty() {
            self.lease = crate::settings::DEFAULT_LEASE.to_string();
        }
    }

    /// the draft of a network that exists, to change it.
    pub fn of(network: &LanNetwork) -> Self {
        let dhcp = network.dhcp.as_ref();
        Self {
            name: network.name.clone(),
            interface: network.interface.clone(),
            address: network.address.clone(),
            range_start: dhcp.map(|d| d.range_start.clone()).unwrap_or_default(),
            range_end: dhcp.map(|d| d.range_end.clone()).unwrap_or_default(),
            lease: dhcp.map(|d| d.lease.clone()).unwrap_or_default(),
            zone: network.zone.clone(),
            mtu: network.mtu.map(|m| m.to_string()).unwrap_or_default(),
        }
    }

    fn mtu(&self) -> Result<Option<u32>, String> {
        match self.mtu.trim() {
            "" => Ok(None),
            text => text
                .parse()
                .map(Some)
                .map_err(|_| format!("the MTU {text:?} must be a number")),
        }
    }

    fn range(&self) -> Result<Option<String>, String> {
        match (self.range_start.trim(), self.range_end.trim()) {
            ("", "") => Ok(None),
            ("", _) | (_, "") => Err("give both ends of the DHCP range, or neither".into()),
            (start, end) => Ok(Some(format!("{start}-{end}"))),
        }
    }

    /// the edit this form makes: an add, or with `existing` (the network's name and the draft it
    /// was opened with) a change of that network that sends only the fields that differ, so an
    /// edit made on the server meanwhile to another field is not undone. `list` is every lanlord
    /// network now and `external` the interface of the external network. what is wrong is said
    /// in the words of the cli's checks.
    pub fn edit(
        &self,
        existing: Option<(&str, &Draft)>,
        list: &[LanNetwork],
        external: Option<&str>,
    ) -> Result<Edit, Vec<String>> {
        let build = || -> Result<Edit, String> {
            let (mtu, range) = (self.mtu()?, self.range()?);
            let name = self.name.trim();
            let zone = Some(self.zone.trim().to_string()).filter(|z| !z.is_empty());
            let zone = zone.or_else(|| Some(name.to_string()));
            let lease = Some(self.lease.trim().to_string()).filter(|l| !l.is_empty());
            let lease = lease.filter(|_| range.is_some());
            let edit = match existing {
                None => add_edit(&AddArgs {
                    name: name.to_string(),
                    interface: self.interface.trim().to_string(),
                    address: self.address.trim().to_string(),
                    mtu,
                    dhcp_range: range,
                    lease,
                    zone,
                }),
                Some((existing, original)) => {
                    let was = (original.mtu()?, original.range()?);
                    let was_zone = original.zone.trim();
                    let was_zone = if was_zone.is_empty() {
                        original.name.trim()
                    } else {
                        was_zone
                    };
                    let (interface, address) = (self.interface.trim(), self.address.trim());
                    let differs =
                        |now: &str, before: &str| (now != before.trim()).then(|| now.to_string());
                    let range_moved = range != was.1;
                    let args = SetArgs {
                        interface: differs(interface, &original.interface),
                        address: differs(address, &original.address),
                        mtu: mtu.filter(|_| mtu != was.0),
                        no_mtu: mtu.is_none() && was.0.is_some(),
                        zone: zone.filter(|z| z != was_zone),
                        no_dhcp: range.is_none() && was.1.is_some(),
                        dhcp_range: range.clone().filter(|_| range_moved),
                        lease: lease.filter(|l| l != original.lease.trim()),
                    };
                    if args.interface.is_none()
                        && args.address.is_none()
                        && args.mtu.is_none()
                        && !args.no_mtu
                        && args.zone.is_none()
                        && !args.no_dhcp
                        && args.dhcp_range.is_none()
                        && args.lease.is_none()
                    {
                        return Err("nothing was changed".into());
                    }
                    set_edit(existing, &args)
                }
            };
            edit.map_err(|e| e.to_string())
        };
        let edit = build().map_err(|e| vec![e])?;
        let mut after = list.to_vec();
        apply_edit(&mut after, &edit).map_err(|e| vec![e.to_string()])?;
        let errors = lan_networks::validate(&after, external, true);
        if errors.is_empty() {
            Ok(edit)
        } else {
            Err(errors.iter().map(ToString::to_string).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::demo;

    const SETTINGS: &str = "docker:\n  internal_address: 172.16.120.1/22\ndns:\n  zone: retronet\n  dhcp:\n    range: [172.16.120.100, 172.16.120.200]\n    lease: 7d\n";

    #[test]
    fn the_external_and_the_lanlord_network_come_from_the_interfaces_and_settings() {
        let settings = Settings::parse(SETTINGS).unwrap();
        let rows = rows(&demo().interfaces, &settings, Some(3));
        assert_eq!(rows.len(), 2);
        let (external, lanlord) = (&rows[0], &rows[1]);
        assert_eq!(external.role, Role::External);
        assert_eq!(
            (
                external.interface.as_str(),
                external.address.as_str(),
                external.mtu
            ),
            ("eth0", "192.0.2.10/24", 1500)
        );
        assert_eq!(lanlord.role, Role::Lanlord);
        assert_eq!(
            (
                lanlord.name.as_str(),
                lanlord.interface.as_str(),
                lanlord.mtu
            ),
            ("retronet", "eth1", 1450)
        );
        assert_eq!(
            lanlord.dhcp.as_deref(),
            Some("172.16.120.100 - 172.16.120.200")
        );
        assert_eq!(lanlord.leases, Some(3));
        assert!(external.dhcp.is_none() && external.leases.is_none());
    }

    #[test]
    fn without_lanlord_settings_only_the_external_network_shows() {
        let rows = rows(&demo().interfaces, &Settings::empty(), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].interface, "eth0");
        // no default route: the first real interface stands in
        let mut ifaces = demo().interfaces;
        ifaces.iter_mut().for_each(|i| i.default_route = false);
        assert_eq!(
            super::rows(&ifaces, &Settings::empty(), None)[0].interface,
            "eth0"
        );
        assert!(super::rows(&[], &Settings::empty(), None).is_empty());
    }

    #[test]
    fn a_networks_list_gives_a_row_per_network_on_its_own_interface() {
        let settings = Settings::parse("networks:\n  - name: retronet\n    interface: eth1\n    address: 172.16.120.1/22\n    dhcp:\n      range: [172.16.120.100, 172.16.120.200]\n  - name: far\n    interface: eth9\n    address: 10.9.0.1/24\n    mtu: 1400\n").unwrap();
        let rows = rows(&demo().interfaces, &settings, Some(3));
        assert_eq!(rows.len(), 3);
        assert_eq!(
            (rows[0].role, rows[0].interface.as_str()),
            (Role::External, "eth0")
        );
        assert_eq!(
            (
                rows[1].name.as_str(),
                rows[1].interface.as_str(),
                rows[1].mtu
            ),
            ("retronet", "eth1", 1450)
        );
        assert_eq!(
            rows[1].dhcp.as_deref(),
            Some("172.16.120.100 - 172.16.120.200")
        );
        // an interface the server does not have is still listed, as not found
        assert_eq!(
            (
                rows[2].interface.as_str(),
                rows[2].link.as_str(),
                rows[2].mtu
            ),
            ("eth9", "interface not found", 1400)
        );
        assert_eq!(rows[2].address, "10.9.0.1/24");
    }

    #[test]
    fn the_external_row_survives_a_default_route_through_a_lanlord_network() {
        let settings = Settings::parse(
            "networks:\n  - name: retronet\n    interface: eth1\n    address: 172.16.120.1/22\n",
        )
        .unwrap();
        let mut interfaces = demo().interfaces;
        for iface in &mut interfaces {
            iface.default_route = iface.name == "eth1";
        }
        let rows = rows(&interfaces, &settings, None);
        assert_eq!(
            (rows[0].role, rows[0].interface.as_str()),
            (Role::External, "eth0")
        );
    }

    #[test]
    fn a_network_without_dhcp_has_no_range_and_no_lease_count() {
        let settings = Settings::parse("docker:\n  internal_address: 172.16.120.1/22\n").unwrap();
        let rows = rows(&demo().interfaces, &settings, Some(5));
        assert_eq!(rows[1].dhcp, None);
        assert_eq!(rows[1].leases, None);
        assert_eq!(rows[1].name, "retronet", "the default zone names it");
    }

    #[test]
    fn details_list_what_the_row_shows() {
        let settings = Settings::parse(SETTINGS).unwrap();
        let rows = rows(&demo().interfaces, &settings, Some(2));
        let details = rows[1].details();
        assert_eq!(details[1], ("Kind".into(), "LANlord network".into()));
        assert!(details.contains(&(
            "DHCP range".into(),
            "172.16.120.100 - 172.16.120.200".into()
        )));
        assert!(details.contains(&("Clients with a lease".into(), "2".into())));
        assert_eq!(rows[0].details()[1].1, "External network");
    }

    fn card(name: &str, state: &str, address: &str) -> Iface {
        Iface {
            name: name.into(),
            mtu: 1450,
            state: state.into(),
            addresses: if address.is_empty() {
                vec![]
            } else {
                vec![address.into()]
            },
            default_route: false,
        }
    }

    fn lan(name: &str, interface: &str, address: &str) -> LanNetwork {
        LanNetwork::new(name, interface, address)
    }

    #[test]
    fn a_card_is_unused_until_a_network_or_the_external_one_has_it() {
        let ifaces = vec![
            card("lo", "UNKNOWN", "127.0.0.1/8"),
            card("eth0", "UP", "192.0.2.10/24"),
            card("eth1", "UP", "172.16.120.1/22"),
            card("eth2", "DOWN", ""),
            card("eth3", "UP", "198.51.100.1/24"),
        ];
        let settings = Settings::parse(
            "networks:\n  - name: retronet\n    interface: eth1\n    address: 172.16.120.1/22\n",
        )
        .unwrap();
        let rows = rows(&ifaces, &settings, None);
        let free = unused(&ifaces, &rows);
        assert_eq!(
            free.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
            ["eth2", "eth3"]
        );
        // an unused card that is up is preferred, else the first one
        assert_eq!(suggested_card(&free).unwrap().name, "eth3");
        assert_eq!(suggested_card(&free[..1]).unwrap().name, "eth2");
        assert!(suggested_card(&[]).is_none());
    }

    #[test]
    fn a_suggested_range_is_inside_the_subnet_and_leaves_out_the_server() {
        assert_eq!(
            suggest_range("172.16.120.1/22"),
            Some(("172.16.120.100".into(), "172.16.120.200".into()))
        );
        assert_eq!(
            suggest_range("198.51.100.1/24"),
            Some(("198.51.100.100".into(), "198.51.100.200".into()))
        );
        assert_eq!(
            suggest_range("198.51.100.129/25"),
            Some(("198.51.100.192".into(), "198.51.100.254".into()))
        );
        // the server sits where the range would be, and a /30 has no room
        assert_eq!(suggest_range("198.51.100.150/24"), None);
        assert_eq!(suggest_range("198.51.100.1/30"), None);
        assert_eq!(suggest_range("nonsense"), None);
    }

    #[test]
    fn a_new_network_starts_from_its_card() {
        let draft = Draft::on_card(&card("eth3", "UP", "198.51.100.1/24"));
        assert_eq!(
            (draft.interface.as_str(), draft.address.as_str()),
            ("eth3", "198.51.100.1/24")
        );
        assert_eq!(draft.mtu, "1450");
        assert_eq!(draft.range_start, "198.51.100.100");
        assert_eq!(draft.lease, "7d");
        // a card without an address leaves the address and range to fill in
        let bare = Draft::on_card(&card("eth2", "DOWN", ""));
        assert_eq!((bare.address.as_str(), bare.range_start.as_str()), ("", ""));
    }

    fn filled() -> Draft {
        Draft {
            name: "lab".into(),
            ..Draft::on_card(&card("eth3", "UP", "198.51.100.1/24"))
        }
    }

    #[test]
    fn a_good_form_makes_the_add_the_cli_makes() {
        let Ok(Edit::Add(network)) = filled().edit(None, &[], Some("eth0")) else {
            panic!("an add");
        };
        assert_eq!(
            (network.name.as_str(), network.zone.as_str(), network.mtu),
            ("lab", "lab", Some(1450))
        );
        let dhcp = network.dhcp.unwrap();
        assert_eq!(
            (
                dhcp.range_start.as_str(),
                dhcp.range_end.as_str(),
                dhcp.lease.as_str()
            ),
            ("198.51.100.100", "198.51.100.200", "7d")
        );
        // no range, no dhcp
        let plain = Draft {
            range_start: String::new(),
            range_end: String::new(),
            ..filled()
        };
        let Ok(Edit::Add(network)) = plain.edit(None, &[], None) else {
            panic!("an add");
        };
        assert!(network.dhcp.is_none());
    }

    #[test]
    fn the_form_refuses_what_the_cli_refuses() {
        let list = [lan("retronet", "eth1", "172.16.120.1/22")];
        let errors = |draft: Draft| {
            draft
                .edit(None, &list, Some("eth0"))
                .unwrap_err()
                .join("; ")
        };
        assert!(errors(Draft {
            address: "198.51.100.1".into(),
            ..filled()
        })
        .contains("must look like"));
        assert!(errors(Draft {
            range_end: "198.51.101.9".into(),
            ..filled()
        })
        .contains("range"));
        assert!(errors(Draft {
            range_end: String::new(),
            ..filled()
        })
        .contains("both ends"));
        assert!(errors(Draft {
            name: "retronet".into(),
            ..filled()
        })
        .contains("already a network"));
        assert!(errors(Draft {
            interface: "eth1".into(),
            ..filled()
        })
        .contains("two networks"));
        assert!(errors(Draft {
            interface: "eth0".into(),
            ..filled()
        })
        .contains("external"));
        assert!(errors(Draft {
            name: "no good".into(),
            ..filled()
        })
        .contains("plain name"));
        assert!(errors(Draft {
            mtu: "big".into(),
            ..filled()
        })
        .contains("number"));
        assert!(errors(Draft {
            mtu: "100".into(),
            ..filled()
        })
        .contains("between"));
        assert!(errors(Draft {
            address: "172.16.121.1/24".into(),
            ..filled()
        })
        .contains("overlaps"));
    }

    #[test]
    fn changing_a_network_sends_only_the_fields_that_changed() {
        let mut network = lan("lab", "eth3", "198.51.100.1/24");
        network.mtu = Some(1400);
        let list = [network.clone()];
        let original = Draft::of(&network);
        let mut draft = original.clone();
        draft.address = "198.51.100.2/24".into();
        draft.mtu = String::new();
        let Ok(Edit::Set { name, change }) = draft.edit(Some(("lab", &original)), &list, None)
        else {
            panic!("a set");
        };
        assert_eq!(name, "lab");
        assert_eq!(change.address.as_deref(), Some("198.51.100.2/24"));
        assert_eq!(change.mtu, Some(None));
        assert!(change.interface.is_none() && change.zone.is_none());
        assert!(!change.dhcp_off && change.dhcp_range.is_none() && change.lease.is_none());
    }

    #[test]
    fn a_form_with_nothing_changed_says_so_and_clearing_the_range_turns_dhcp_off() {
        let mut network = lan("lab", "eth3", "198.51.100.1/24");
        network.dhcp = Some(crate::settings::Dhcp {
            range_start: "198.51.100.100".into(),
            range_end: "198.51.100.200".into(),
            lease: "7d".into(),
            hosts: Vec::new(),
        });
        let list = [network.clone()];
        let original = Draft::of(&network);
        let errors = original
            .edit(Some(("lab", &original)), &list, None)
            .unwrap_err();
        assert_eq!(errors, ["nothing was changed"]);
        let mut draft = original.clone();
        draft.range_start = String::new();
        draft.range_end = String::new();
        let Ok(Edit::Set { change, .. }) = draft.edit(Some(("lab", &original)), &list, None) else {
            panic!("a set");
        };
        assert!(change.dhcp_off && change.address.is_none());
    }
}
