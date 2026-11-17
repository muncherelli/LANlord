//! the networks a server is on, for the Networks window: its external network (the normal
//! one, the way to the internet) and lanlord's own networks, read from the interfaces and the
//! settings. nothing here changes anything.

use serde_yaml_ng::Value;

use crate::lan_networks;
use crate::server::Iface;
use crate::settings::{DnsSettings, Settings};

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
            },
        };
        entry.dhcp = dhcp;
        rows.push(entry);
    }
    rows
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
}
