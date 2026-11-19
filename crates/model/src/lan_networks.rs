//! the networks lanlord manages: any number of lanlord networks, each on its own interface with
//! its own address, dhcp server, dns zone and static leases, listed under `networks:` in the
//! plain settings. an older single `dns:` section (with `docker.internal_address`) is read as one
//! network called retronet, and written back as a `networks:` list. nothing here touches a
//! machine; the playbooks do the work (see ansible/playbooks/networks.yml).

use std::collections::BTreeSet;
use std::net::Ipv4Addr;

use anyhow::{anyhow, bail, Result};
use serde_yaml_ng::{Mapping, Value};

use crate::format::Context;
use crate::settings::{valid_label, Dhcp, DnsSettings, Record, Settings, Subnet, ValidationError};

/// what the first network is called when it comes from the older single-network settings.
pub const LEGACY_NAME: &str = "retronet";

/// the smallest and largest mtu a network may ask for.
pub const MTU_RANGE: std::ops::RangeInclusive<u32> = 576..=9000;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LanNetwork {
    /// a plain label; names the container `lanlord-dns-<name>` and its state folder.
    pub name: String,
    /// the interface the network is on, like eth1. empty only for a network read from the older
    /// settings, where the playbooks find it.
    pub interface: String,
    /// the server's own address on the network, with its prefix, like 172.16.120.1/22.
    pub address: String,
    pub mtu: Option<u32>,
    /// the dns zone this network's dnsmasq answers for.
    pub zone: String,
    pub records: Vec<Record>,
    /// the dhcp server; none means this network gets no dhcp.
    pub dhcp: Option<Dhcp>,
}

impl LanNetwork {
    pub fn new(name: &str, interface: &str, address: &str) -> Self {
        Self {
            name: name.to_string(),
            interface: interface.to_string(),
            address: address.to_string(),
            mtu: None,
            zone: name.to_ascii_lowercase(),
            records: Vec::new(),
            dhcp: None,
        }
    }

    /// the server's own ip on this network.
    pub fn ip(&self) -> Option<Ipv4Addr> {
        split_address(&self.address).map(|(ip, _)| ip)
    }

    /// the network the address is on.
    pub fn subnet(&self) -> Option<Subnet> {
        split_address(&self.address).map(|(ip, prefix)| Subnet::new(ip, prefix))
    }
}

/// `172.16.120.1/22` as its parts.
pub fn split_address(text: &str) -> Option<(Ipv4Addr, u8)> {
    let (ip, prefix) = text.trim().split_once('/')?;
    let prefix: u8 = prefix.parse().ok().filter(|p| (8..=30).contains(p))?;
    Some((ip.parse().ok()?, prefix))
}

/// the networks in the settings, and whether they came from the older single-network shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Networks {
    pub list: Vec<LanNetwork>,
    pub legacy: bool,
}

fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn key(name: &str) -> Value {
    Value::String(name.into())
}

fn plain_interface(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 15
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.:-".contains(c))
}

/// the older settings as a mapping for the first network: `dns:` and the docker keys.
fn legacy(settings: &Settings) -> Result<Option<LanNetwork>> {
    let docker = match settings.section("docker") {
        Some(Value::Mapping(map)) => Some(map),
        _ => None,
    };
    let from_docker = |name: &str| {
        docker
            .and_then(|m| m.get(name))
            .and_then(text_of)
            .unwrap_or_default()
    };
    let has_dns = matches!(
        settings.section("dns"),
        Some(Value::Mapping(_)) | Some(Value::Null)
    );
    let address = from_docker("internal_address");
    if !has_dns && address.is_empty() {
        return Ok(None);
    }
    let dns = DnsSettings::from_settings(settings)?;
    let mut interface = from_docker("internal_interface");
    if interface.is_empty() {
        if let Some(Value::Mapping(net)) = settings.section("network") {
            interface = net
                .get("internal_interface")
                .and_then(text_of)
                .unwrap_or_default();
        }
    }
    Ok(Some(LanNetwork {
        name: LEGACY_NAME.into(),
        interface,
        address,
        mtu: None,
        zone: dns.zone,
        records: dns.records,
        dhcp: dns.dhcp,
    }))
}

fn parse_network(item: &Mapping, index: usize) -> Result<LanNetwork> {
    let path = format!("networks[{index}]");
    let name = item
        .get("name")
        .and_then(text_of)
        .ok_or_else(|| anyhow!("{path}.name is missing"))?;
    let mtu = match item.get("mtu") {
        None | Some(Value::Null) => None,
        Some(Value::Number(n)) => Some(
            n.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| anyhow!("{path}.mtu is not a whole number"))?,
        ),
        Some(_) => bail!("{path}.mtu is not a number"),
    };
    // dns zone, records and dhcp share their shape with the old `dns:` section
    let mut dns = Mapping::new();
    for part in ["zone", "records", "dhcp"] {
        if let Some(value) = item.get(part) {
            dns.insert(key(part), value.clone());
        }
    }
    let mut holder = Settings::empty();
    holder.set_section("dns", Value::Mapping(dns));
    let parsed = DnsSettings::from_settings(&holder)
        .map_err(|e| anyhow!("{}", e.to_string().replace("dns.", &format!("{path}."))))?;
    let zone = item
        .get("zone")
        .and_then(text_of)
        .unwrap_or_else(|| name.to_ascii_lowercase());
    Ok(LanNetwork {
        interface: item.get("interface").and_then(text_of).unwrap_or_default(),
        address: item.get("address").and_then(text_of).unwrap_or_default(),
        mtu,
        zone,
        records: parsed.records,
        dhcp: parsed.dhcp,
        name,
    })
}

/// read the networks: the `networks:` list when there is one, else the older single-network
/// settings as one network called retronet, else none.
pub fn read(settings: &Settings) -> Result<Networks> {
    match settings.section("networks") {
        Some(Value::Sequence(items)) => {
            let list = items
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    let map = item
                        .as_mapping()
                        .ok_or_else(|| anyhow!("networks[{i}] is not a mapping"))?;
                    parse_network(map, i)
                })
                .collect::<Result<_>>()?;
            Ok(Networks {
                list,
                legacy: false,
            })
        }
        Some(Value::Null) => Ok(Networks {
            list: Vec::new(),
            legacy: false,
        }),
        Some(_) => bail!("networks must be a list"),
        None => Ok(match legacy(settings)? {
            Some(network) => Networks {
                list: vec![network],
                legacy: true,
            },
            None => Networks {
                list: Vec::new(),
                legacy: false,
            },
        }),
    }
}

/// format step 1 to 2: the older single network becomes a `networks:` list. a network that does
/// not say its interface and address yet stays as it is (the playbooks find them), and so does
/// anything that cannot be read, so that an existing install behaves as it did.
pub fn migrate_legacy(settings: &mut Settings, _: &Context) -> Result<()> {
    let Ok(Networks { list, legacy: true }) = read(settings) else {
        return Ok(());
    };
    if validate(&list, None, true).is_empty() {
        write(settings, &list);
    }
    Ok(())
}

fn network_value(network: &LanNetwork) -> Value {
    let mut map = Mapping::new();
    map.insert(key("name"), key(&network.name));
    map.insert(key("interface"), key(&network.interface));
    map.insert(key("address"), key(&network.address));
    if let Some(mtu) = network.mtu {
        map.insert(key("mtu"), Value::Number(mtu.into()));
    }
    let mut holder = Settings::empty();
    DnsSettings {
        zone: network.zone.clone(),
        records: network.records.clone(),
        dhcp: network.dhcp.clone(),
    }
    .apply_to(&mut holder);
    if let Some(Value::Mapping(dns)) = holder.section("dns") {
        for (name, value) in dns {
            // an empty list says nothing
            if matches!(value, Value::Sequence(s) if s.is_empty()) {
                continue;
            }
            map.insert(name.clone(), value.clone());
        }
    }
    Value::Mapping(map)
}

/// store the networks as the `networks:` list. the older settings they replace (`dns:` and the
/// docker interface and address keys) are removed, so there is one source of truth.
pub fn write(settings: &mut Settings, list: &[LanNetwork]) {
    let items = list.iter().map(network_value).collect();
    settings.set_section("networks", Value::Sequence(items));
    settings.remove_section("dns");
    if let Some(Value::Mapping(docker)) = settings.section("docker") {
        let mut docker = docker.clone();
        docker.remove("internal_address");
        docker.remove("internal_interface");
        settings.set_section("docker", Value::Mapping(docker));
    }
}

fn overlap(a: &Subnet, b: &Subnet) -> bool {
    a.contains(b.network) || b.contains(a.network)
}

/// everything wrong with these networks. `external` is the interface of the normal network,
/// when it is known, which no lanlord network may use. empty means fine. a network read from
/// the older settings may leave its interface and address empty; `complete` says whether they
/// have to be there (they do before the networks are written back).
pub fn validate(
    list: &[LanNetwork],
    external: Option<&str>,
    complete: bool,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    let mut seen = Seen::default();
    for (i, network) in list.iter().enumerate() {
        let path = format!("networks[{i}]");
        errors.extend(seen.name_error(network, &path));
        errors.extend(seen.interface_error(network, &path, external, complete));
        errors.extend(mtu_error(network, &path));
        errors.extend(seen.zone_error(network, &path));
        if network.address.is_empty() && !complete {
            continue;
        }
        errors.extend(seen.address_errors(i, network, &path));
    }
    errors
}

/// the names, interfaces, zones and subnets of the networks checked so far.
#[derive(Default)]
struct Seen {
    names: BTreeSet<String>,
    interfaces: BTreeSet<String>,
    zones: BTreeSet<String>,
    subnets: Vec<(usize, Subnet)>,
}

fn fail(path: String, message: String) -> ValidationError {
    ValidationError { path, message }
}

impl Seen {
    fn name_error(&mut self, network: &LanNetwork, path: &str) -> Option<ValidationError> {
        let message = if !valid_label(&network.name) {
            "must be a plain name: letters, digits and dashes".into()
        } else if !self.names.insert(network.name.to_ascii_lowercase()) {
            format!("{} is used twice", network.name)
        } else {
            return None;
        };
        Some(fail(format!("{path}.name"), message))
    }

    fn interface_error(
        &mut self,
        network: &LanNetwork,
        path: &str,
        external: Option<&str>,
        complete: bool,
    ) -> Option<ValidationError> {
        let interface = &network.interface;
        let message = if interface.is_empty() {
            if !complete {
                return None;
            }
            "must say which interface the network is on, like eth1".into()
        } else if !plain_interface(interface) {
            "must be a plain interface name like eth1".into()
        } else if Some(interface.as_str()) == external {
            format!("{interface} is the external network's interface")
        } else if !self.interfaces.insert(interface.clone()) {
            format!("{interface} is used by two networks")
        } else {
            return None;
        };
        Some(fail(format!("{path}.interface"), message))
    }

    fn zone_error(&mut self, network: &LanNetwork, path: &str) -> Option<ValidationError> {
        let message = if !valid_label(&network.zone) {
            "must be a plain label like retronet".into()
        } else if !self.zones.insert(network.zone.to_ascii_lowercase()) {
            format!("{} is used by two networks", network.zone)
        } else {
            return None;
        };
        Some(fail(format!("{path}.zone"), message))
    }

    /// the address, its overlaps with earlier networks, and the DNS and DHCP settings.
    fn address_errors(
        &mut self,
        i: usize,
        network: &LanNetwork,
        path: &str,
    ) -> Vec<ValidationError> {
        let Some((ip, prefix)) = split_address(&network.address) else {
            return vec![fail(
                format!("{path}.address"),
                format!(
                    "{:?} must look like 172.16.120.1/22 with a prefix from 8 to 30",
                    network.address
                ),
            )];
        };
        let subnet = Subnet::new(ip, prefix);
        let mut errors = ip_errors(ip, &subnet, path);
        errors.extend(
            self.subnets
                .iter()
                .filter(|(_, other)| overlap(&subnet, other))
                .map(|(j, other)| {
                    fail(
                        format!("{path}.address"),
                        format!("{subnet} overlaps the network networks[{j}] ({other})"),
                    )
                }),
        );
        errors.extend(dns_errors(network, &subnet, ip, path));
        self.subnets.push((i, subnet));
        errors
    }
}

fn mtu_error(network: &LanNetwork, path: &str) -> Option<ValidationError> {
    if network.mtu.is_none_or(|mtu| MTU_RANGE.contains(&mtu)) {
        return None;
    }
    Some(fail(
        format!("{path}.mtu"),
        format!(
            "must be between {} and {}",
            MTU_RANGE.start(),
            MTU_RANGE.end()
        ),
    ))
}

/// an address that is the network's own, or one no network can have.
fn ip_errors(ip: Ipv4Addr, subnet: &Subnet, path: &str) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    if ip == subnet.network || ip == subnet.broadcast() {
        errors.push(fail(
            format!("{path}.address"),
            format!("{ip} is the network or broadcast address of {subnet}"),
        ));
    }
    if ip.is_loopback() || ip.is_multicast() || ip.is_unspecified() || ip.is_link_local() {
        errors.push(fail(
            format!("{path}.address"),
            format!("{ip} cannot be a network's address"),
        ));
    }
    errors
}

/// the problems of the network's DNS records and DHCP; the zone was checked already.
fn dns_errors(
    network: &LanNetwork,
    subnet: &Subnet,
    ip: Ipv4Addr,
    path: &str,
) -> Vec<ValidationError> {
    let dns = DnsSettings {
        zone: network.zone.clone(),
        records: network.records.clone(),
        dhcp: network.dhcp.clone(),
    };
    dns.validate(subnet, ip)
        .into_iter()
        .filter(|error| error.path != "dns.zone")
        .map(|error| fail(error.path.replacen("dns", path, 1), error.message))
        .collect()
}

/// find a network by name, ignoring case.
pub fn find<'a>(list: &'a [LanNetwork], name: &str) -> Option<&'a LanNetwork> {
    list.iter().find(|n| n.name.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::StaticLease;

    const OLD: &str = "docker:\n  internal_address: 172.16.120.1/22\n  internal_interface: eth1\ndns:\n  zone: retronet\n  records:\n    - {name: ntsrv1, address: 172.16.120.10}\n  dhcp:\n    range: [172.16.123.1, 172.16.123.254]\n    lease: 12h\n    hosts:\n      - {name: ws1, mac: \"02:00:00:00:00:01\", address: 172.16.121.1}\nnt4-domain:\n  mode: domain\n";

    const NEW: &str = "docker: {}\nnetworks:\n  - name: retronet\n    interface: eth1\n    address: 172.16.120.1/22\n    mtu: 1450\n    dhcp:\n      range: [172.16.123.1, 172.16.123.254]\n  - name: lab\n    interface: eth2\n    address: 192.168.50.1/24\n    zone: lab.test\n";

    fn parse(text: &str) -> Settings {
        Settings::parse(text).unwrap()
    }

    fn network(name: &str, interface: &str, address: &str) -> LanNetwork {
        LanNetwork::new(name, interface, address)
    }

    fn messages(list: &[LanNetwork]) -> Vec<String> {
        validate(list, Some("eth0"), true)
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn the_older_single_network_shape_is_read_as_retronet() {
        let read = read(&parse(OLD)).unwrap();
        assert!(read.legacy);
        let [net] = read.list.as_slice() else {
            panic!("one network")
        };
        assert_eq!(
            (
                net.name.as_str(),
                net.interface.as_str(),
                net.address.as_str()
            ),
            ("retronet", "eth1", "172.16.120.1/22")
        );
        assert_eq!(net.records[0].name, "ntsrv1");
        let dhcp = net.dhcp.as_ref().unwrap();
        assert_eq!(dhcp.lease, "12h");
        assert_eq!(dhcp.hosts.len(), 1);
        assert!(super::validate(&read.list, Some("eth0"), true).is_empty());
    }

    #[test]
    fn the_networks_list_is_read_with_its_defaults() {
        let read = read(&parse(NEW)).unwrap();
        assert!(!read.legacy);
        assert_eq!(read.list.len(), 2);
        assert_eq!(read.list[0].mtu, Some(1450));
        assert_eq!(read.list[0].zone, "retronet");
        assert_eq!(read.list[1].zone, "lab.test");
        assert!(read.list[1].dhcp.is_none());
        assert!(!super::validate(&read.list, Some("eth0"), true).is_empty());
        // lab.test is not a plain label, so the second network is refused
        assert!(messages(&read.list)[0].contains("networks[1].zone"));
    }

    #[test]
    fn nothing_configured_means_no_networks() {
        assert!(read(&parse("gateway: {}\n")).unwrap().list.is_empty());
        assert!(read(&parse("docker: {}\n")).unwrap().list.is_empty());
        assert!(read(&parse("networks: []\ndns: {}\n"))
            .unwrap()
            .list
            .is_empty());
        // docker alone, with an address, is a network without a dhcp server
        let docker = read(&parse("docker:\n  internal_address: 172.16.120.2/22\n")).unwrap();
        assert!(docker.legacy && docker.list[0].address == "172.16.120.2/22");
        assert!(docker.list[0].dhcp.is_none());
        assert!(read(&parse("networks: 3\n")).is_err());
        assert!(read(&parse("networks: [3]\n")).is_err());
        assert!(read(&parse("networks:\n  - {interface: eth1}\n")).is_err());
        assert!(read(&parse("networks:\n  - {name: a, mtu: x}\n")).is_err());
    }

    #[test]
    fn writing_replaces_the_older_settings_and_round_trips() {
        let mut settings = parse(OLD);
        let before = read(&settings).unwrap();
        write(&mut settings, &before.list);
        let text = settings.text();
        assert!(
            text.contains("networks:") && !text.contains("\ndns:"),
            "{text}"
        );
        assert!(
            !text.contains("internal_address") && !text.contains("internal_interface"),
            "{text}"
        );
        assert!(text.contains("nt4-domain"), "other sections stay: {text}");
        let after = read(&settings).unwrap();
        assert!(!after.legacy);
        assert_eq!(after.list, before.list);
    }

    #[test]
    fn writing_keeps_other_docker_keys() {
        let mut settings =
            parse("docker:\n  host_interface: eth0\n  internal_address: 10.1.0.1/24\n");
        write(&mut settings, &[network("lab", "eth2", "10.1.0.1/24")]);
        let text = settings.text();
        assert!(
            text.contains("host_interface: eth0") && !text.contains("internal_address"),
            "{text}"
        );
    }

    #[test]
    fn a_good_set_of_networks_is_fine() {
        let mut first = network("retronet", "eth1", "172.16.120.1/22");
        first.mtu = Some(1450);
        first.dhcp = Some(Dhcp {
            range_start: "172.16.123.1".into(),
            range_end: "172.16.123.254".into(),
            lease: "7d".into(),
            hosts: vec![StaticLease {
                name: "ws1".into(),
                mac: "02:00:00:00:00:01".into(),
                address: "172.16.121.1".into(),
            }],
        });
        let second = network("lab", "eth2", "192.168.50.1/24");
        assert_eq!(messages(&[first, second]), Vec::<String>::new());
    }

    #[test]
    fn bad_names_interfaces_and_addresses_are_refused() {
        let bad = [
            (
                network("Bad Name", "eth1", "10.0.0.1/24"),
                "networks[0].name",
            ),
            (network("a", "", "10.0.0.1/24"), "networks[0].interface"),
            (network("a", "eth0", "10.0.0.1/24"), "external network"),
            (network("a", "e t h", "10.0.0.1/24"), "plain interface"),
            (network("a", "eth1", "10.0.0.1"), "networks[0].address"),
            (network("a", "eth1", "10.0.0.1/31"), "networks[0].address"),
            (network("a", "eth1", "10.0.0.0/24"), "network or broadcast"),
            (
                network("a", "eth1", "127.0.0.1/8"),
                "cannot be a network's address",
            ),
        ];
        for (net, wanted) in bad {
            let found = messages(&[net]).join("\n");
            assert!(found.contains(wanted), "{wanted} in {found}");
        }
    }

    #[test]
    fn duplicates_and_overlaps_are_refused() {
        let found = messages(&[
            network("a", "eth1", "10.0.0.1/24"),
            network("A", "eth1", "10.0.0.129/25"),
        ])
        .join("\n");
        for wanted in [
            "networks[1].name",
            "used by two networks",
            "overlaps",
            "zone",
        ] {
            assert!(found.contains(wanted), "{wanted} in {found}");
        }
    }

    #[test]
    fn mtu_dhcp_and_leases_are_checked_inside_the_network() {
        let mut net = network("a", "eth1", "10.0.0.1/24");
        net.mtu = Some(100);
        net.dhcp = Some(Dhcp {
            range_start: "10.0.1.5".into(),
            range_end: "10.0.0.9".into(),
            lease: "forever".into(),
            hosts: vec![StaticLease {
                name: "x".into(),
                mac: "bad".into(),
                address: "10.0.0.1".into(),
            }],
        });
        let found = messages(&[net]).join("\n");
        for wanted in [
            "networks[0].mtu",
            "networks[0].dhcp.range[0]",
            "networks[0].dhcp.lease",
            "networks[0].dhcp.hosts[0].mac",
            "gateway's own address",
        ] {
            assert!(found.contains(wanted), "{wanted} in {found}");
        }
    }

    #[test]
    fn an_older_network_may_leave_its_interface_for_the_playbooks_to_find() {
        let legacy = read(&parse("dns: {}\n")).unwrap();
        assert!(legacy.list[0].interface.is_empty() && legacy.list[0].address.is_empty());
        assert!(validate(&legacy.list, None, false).is_empty());
        assert!(!validate(&legacy.list, None, true).is_empty());
    }

    #[test]
    fn networks_are_found_by_name_ignoring_case() {
        let list = [network("Lab", "eth2", "10.0.0.1/24")];
        assert!(find(&list, "lab").is_some() && find(&list, "x").is_none());
        let net = &list[0];
        assert_eq!(net.ip(), Some(Ipv4Addr::new(10, 0, 0, 1)));
        assert_eq!(net.subnet().unwrap().to_string(), "10.0.0.0/24");
        assert!(network("a", "eth1", "junk").subnet().is_none());
    }
}
