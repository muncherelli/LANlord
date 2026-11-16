//! the status of a server's network connections, the way Windows shows a connection's Status
//! and Network Connection Details boxes: link state and speed, addresses, gateway, dns, the
//! dhcp lease and the traffic counters. it is parsed from one read-only script's output (the
//! script lives in core); nothing here touches the system.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// whether a connection is usable, in the words Windows uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Connection {
    Connected,
    /// the link is up but nothing is plugged in (no carrier).
    CableUnplugged,
    /// the interface is turned off.
    Disabled,
    Unknown,
}

impl Connection {
    pub fn text(self) -> &'static str {
        match self {
            Self::Connected => "Connected",
            Self::CableUnplugged => "Network cable unplugged",
            Self::Disabled => "Disabled",
            Self::Unknown => "Unknown",
        }
    }
}

/// one ipv4 address with its prefix length.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ipv4 {
    pub address: String,
    pub prefix: u8,
}

impl Ipv4 {
    /// the prefix written as a subnet mask, like `255.255.255.0`.
    pub fn mask(&self) -> String {
        let bits = if self.prefix == 0 {
            0
        } else {
            u32::MAX << (32 - u32::from(self.prefix.min(32)))
        };
        let [a, b, c, d] = bits.to_be_bytes();
        format!("{a}.{b}.{c}.{d}")
    }
}

/// the dhcp lease an address came from. times are unix seconds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dhcp {
    pub server: Option<String>,
    pub obtained: Option<i64>,
    pub expires: Option<i64>,
}

/// bytes and packets through the interface since it came up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counters {
    pub sent_bytes: u64,
    pub received_bytes: u64,
    pub sent_packets: u64,
    pub received_packets: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterfaceStatus {
    pub name: String,
    /// what the adapter is, like `Virtual Ethernet (veth)` or `Ethernet (e1000e)`.
    pub description: String,
    pub connection: Connection,
    /// the default route leaves through this interface: it is the external network.
    pub default_route: bool,
    pub speed_mbps: Option<u32>,
    /// `full` or `half`, when the driver says.
    pub duplex: Option<String>,
    /// the physical address, like `02:00:00:00:aa:01`.
    pub mac: Option<String>,
    pub mtu: u32,
    pub ipv4: Vec<Ipv4>,
    /// the ipv6 addresses with their prefix, like `2001:db8::10/64`.
    pub ipv6: Vec<String>,
    pub gateway: Option<String>,
    pub dns: Vec<String>,
    /// set when an address was handed out by dhcp.
    pub dhcp: Option<Dhcp>,
    pub counters: Counters,
}

impl InterfaceStatus {
    /// the speed the way Windows writes it, like `1.0 Gbps` or `100.0 Mbps`.
    pub fn speed(&self) -> String {
        match self.speed_mbps {
            Some(m) if m >= 1000 => format!("{:.1} Gbps", f64::from(m) / 1000.0),
            Some(m) => format!("{m}.0 Mbps"),
            None => "Unknown".to_string(),
        }
    }

    /// the rows of the Network Connection Details box. `time` writes a unix time for people.
    pub fn details(&self, time: impl Fn(i64) -> String) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Description".to_string(), self.description.clone()),
            (
                "Physical Address".to_string(),
                self.mac
                    .clone()
                    .unwrap_or_default()
                    .to_uppercase()
                    .replace(':', "-"),
            ),
            (
                "DHCP Enabled".to_string(),
                if self.dhcp.is_some() { "Yes" } else { "No" }.to_string(),
            ),
        ];
        for ip in &self.ipv4 {
            rows.push(("IPv4 Address".to_string(), ip.address.clone()));
            rows.push(("IPv4 Subnet Mask".to_string(), ip.mask()));
        }
        if let Some(dhcp) = &self.dhcp {
            if let Some(at) = dhcp.obtained {
                rows.push(("Lease Obtained".to_string(), time(at)));
            }
            if let Some(at) = dhcp.expires {
                rows.push(("Lease Expires".to_string(), time(at)));
            }
        }
        rows.push((
            "IPv4 Default Gateway".to_string(),
            self.gateway.clone().unwrap_or_default(),
        ));
        if let Some(server) = self.dhcp.as_ref().and_then(|d| d.server.clone()) {
            rows.push(("IPv4 DHCP Server".to_string(), server));
        }
        for (i, dns) in self.dns.iter().enumerate() {
            let label = if i == 0 { "IPv4 DNS Server" } else { "" };
            rows.push((label.to_string(), dns.clone()));
        }
        for (i, ip) in self.ipv6.iter().enumerate() {
            let label = if i == 0 { "IPv6 Address" } else { "" };
            rows.push((label.to_string(), ip.clone()));
        }
        rows.push(("MTU".to_string(), self.mtu.to_string()));
        rows
    }
}

/// the interfaces in `text`, the output of the read script in core: an `---iface <name>` block
/// per interface with `key=value` lines and the interface's `ip -j addr` json, then `---route`
/// (`ip -j route show default`), `---resolv` (the nameserver lines) and `---now` (unix time).
pub fn parse(text: &str) -> Vec<InterfaceStatus> {
    let mut blocks: Vec<(String, Vec<&str>)> = Vec::new();
    let mut route = String::new();
    let mut resolv = Vec::new();
    let mut now = None;
    let mut section = "";
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("---iface ") {
            section = "iface";
            blocks.push((name.trim().to_string(), Vec::new()));
        } else if let Some(rest) = line.strip_prefix("---") {
            section = match rest.trim() {
                "route" => "route",
                "resolv" => "resolv",
                "now" => "now",
                _ => "",
            };
        } else {
            match section {
                "iface" => {
                    if let Some((_, lines)) = blocks.last_mut() {
                        lines.push(line);
                    }
                }
                "route" => route.push_str(line),
                "resolv" => {
                    if let Some(server) = line.trim().strip_prefix("nameserver") {
                        resolv.push(server.trim().to_string());
                    }
                }
                "now" => now = line.trim().parse::<i64>().ok().or(now),
                _ => {}
            }
        }
    }
    let routes: Vec<Value> = serde_json::from_str(&route).unwrap_or_default();
    blocks
        .iter()
        .map(|(name, lines)| one(name, lines, &routes, &resolv, now))
        .collect()
}

fn value<'a>(lines: &[&'a str], key: &str) -> Option<&'a str> {
    lines.iter().rev().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key)
            .then(|| v.trim().trim_matches(['\'', '"']))
            .filter(|v| !v.is_empty())
    })
}

fn number<T: std::str::FromStr>(lines: &[&str], key: &str) -> Option<T> {
    value(lines, key)?.parse().ok()
}

fn one(
    name: &str,
    lines: &[&str],
    routes: &[Value],
    resolv: &[String],
    now: Option<i64>,
) -> InterfaceStatus {
    let addr: Value = value(lines, "addr")
        .and_then(|j| serde_json::from_str::<Vec<Value>>(j).ok())
        .and_then(|mut v| (!v.is_empty()).then(|| v.remove(0)))
        .unwrap_or(Value::Null);
    let infos = addr["addr_info"].as_array().cloned().unwrap_or_default();
    let mut ipv4 = Vec::new();
    let mut ipv6 = Vec::new();
    let mut lease_left = None;
    for info in &infos {
        let (Some(local), Some(prefix)) = (info["local"].as_str(), info["prefixlen"].as_u64())
        else {
            continue;
        };
        match info["family"].as_str() {
            Some("inet") => {
                ipv4.push(Ipv4 {
                    address: local.to_string(),
                    prefix: u8::try_from(prefix).unwrap_or(32),
                });
                if info["dynamic"].as_bool() == Some(true) {
                    lease_left = lease_left.or(info["valid_life_time"].as_i64());
                }
            }
            Some("inet6") if info["scope"].as_str() != Some("link") => {
                ipv6.push(format!("{local}/{prefix}"));
            }
            _ => {}
        }
    }
    let default = routes
        .iter()
        .find(|r| r["dev"].as_str() == Some(name) && r["dst"].as_str() == Some("default"));
    let operstate = value(lines, "operstate").or_else(|| addr["operstate"].as_str());
    let carrier = value(lines, "carrier");
    let connection = match (operstate, carrier) {
        (Some("lowerlayerdown"), _) | (_, Some("0")) => Connection::CableUnplugged,
        (Some("down"), _) => Connection::Disabled,
        (Some("up"), _) | (_, Some("1")) => Connection::Connected,
        _ => Connection::Unknown,
    };
    let dhcp = lease_left.map(|left| {
        let lease_time = number::<i64>(lines, "lease_time");
        let expires = now.map(|n| n + left);
        Dhcp {
            server: value(lines, "dhcp_server").map(str::to_string),
            obtained: expires.zip(lease_time).map(|(e, t)| e - t),
            expires,
        }
    });
    let dns: Vec<String> = value(lines, "dns")
        .map(|d| d.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    let dns = if dns.is_empty() && default.is_some() {
        resolv
            .iter()
            .filter(|s| !s.starts_with("127."))
            .cloned()
            .collect()
    } else {
        dns
    };
    InterfaceStatus {
        name: name.to_string(),
        description: description(value(lines, "kind"), value(lines, "driver")),
        connection,
        default_route: default.is_some(),
        speed_mbps: number::<i64>(lines, "speed")
            .filter(|s| *s > 0)
            .and_then(|s| u32::try_from(s).ok()),
        duplex: value(lines, "duplex")
            .filter(|d| *d != "unknown")
            .map(str::to_string),
        mac: addr["address"].as_str().map(str::to_string),
        mtu: addr["mtu"]
            .as_u64()
            .and_then(|m| u32::try_from(m).ok())
            .unwrap_or(0),
        ipv4,
        ipv6,
        gateway: default
            .and_then(|r| r["gateway"].as_str())
            .map(str::to_string),
        dns,
        dhcp,
        counters: Counters {
            sent_bytes: number(lines, "tx_bytes").unwrap_or(0),
            received_bytes: number(lines, "rx_bytes").unwrap_or(0),
            sent_packets: number(lines, "tx_packets").unwrap_or(0),
            received_packets: number(lines, "rx_packets").unwrap_or(0),
        },
    }
}

fn description(kind: Option<&str>, driver: Option<&str>) -> String {
    match (kind, driver) {
        (Some("veth"), _) => "Virtual Ethernet (veth)".to_string(),
        (Some("vlan"), _) => "VLAN".to_string(),
        (Some("bridge"), _) => "Bridge".to_string(),
        (Some(kind), _) => format!("Virtual ({kind})"),
        (None, Some(driver)) => format!("Ethernet ({driver})"),
        (None, None) => "Ethernet".to_string(),
    }
}

/// what the made-up demo server answers, in the script's own format.
pub const DEMO: &str = r#"---iface eth0
operstate=up
carrier=1
speed=10000
duplex=full
kind=veth
tx_bytes=48211933
rx_bytes=913442071
tx_packets=402113
rx_packets=771904
lease_time=86400
dhcp_server=192.0.2.1
addr=[{"ifindex":2,"ifname":"eth0","mtu":1500,"operstate":"UP","address":"02:00:00:00:aa:01","addr_info":[{"family":"inet","local":"192.0.2.10","prefixlen":24,"dynamic":true,"valid_life_time":61200},{"family":"inet6","local":"2001:db8::10","prefixlen":64,"scope":"global"},{"family":"inet6","local":"fe80::ff:fe00:aa01","prefixlen":64,"scope":"link"}]}]
---iface eth1
operstate=up
carrier=1
speed=10000
duplex=full
kind=veth
tx_bytes=5213307
rx_bytes=2290011
tx_packets=30122
rx_packets=28890
addr=[{"ifindex":3,"ifname":"eth1","mtu":1450,"operstate":"UP","address":"02:00:00:00:aa:02","addr_info":[{"family":"inet","local":"172.16.120.1","prefixlen":22}]}]
---route
[{"dst":"default","gateway":"192.0.2.1","dev":"eth0","protocol":"dhcp","prefsrc":"192.0.2.10"}]
---resolv
nameserver 127.0.0.53
nameserver 198.51.100.53
---now
1795000000
"#;

/// the demo server's interfaces.
pub fn demo() -> Vec<InterfaceStatus> {
    parse(DEMO)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_external_network_reads_like_a_windows_status_box() {
        let all = demo();
        assert_eq!(all.len(), 2);
        let eth0 = &all[0];
        assert!(eth0.default_route);
        assert_eq!(eth0.connection, Connection::Connected);
        assert_eq!(eth0.speed(), "10.0 Gbps");
        assert_eq!(eth0.description, "Virtual Ethernet (veth)");
        assert_eq!(eth0.mac.as_deref(), Some("02:00:00:00:aa:01"));
        assert_eq!(eth0.ipv4[0].mask(), "255.255.255.0");
        assert_eq!(eth0.gateway.as_deref(), Some("192.0.2.1"));
        assert_eq!(
            eth0.dns,
            ["198.51.100.53"],
            "the local stub resolver is left out"
        );
        assert_eq!(
            eth0.ipv6,
            ["2001:db8::10/64"],
            "link-local addresses are left out"
        );
        let dhcp = eth0.dhcp.as_ref().unwrap();
        assert_eq!(dhcp.server.as_deref(), Some("192.0.2.1"));
        assert_eq!(dhcp.expires, Some(1_795_061_200));
        assert_eq!(dhcp.obtained, Some(1_795_061_200 - 86_400));
        assert_eq!(eth0.counters.received_packets, 771_904);
    }

    #[test]
    fn a_static_internal_network_has_no_lease_or_gateway() {
        let eth1 = &demo()[1];
        assert!(!eth1.default_route && eth1.dhcp.is_none() && eth1.gateway.is_none());
        assert!(eth1.dns.is_empty());
        assert_eq!(
            (eth1.mtu, eth1.ipv4[0].mask().as_str()),
            (1450, "255.255.252.0")
        );
    }

    #[test]
    fn details_list_the_rows_windows_shows() {
        let rows = demo()[0].details(|t| t.to_string());
        let labels: Vec<_> = rows.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Description",
                "Physical Address",
                "DHCP Enabled",
                "IPv4 Address",
                "IPv4 Subnet Mask",
                "Lease Obtained",
                "Lease Expires",
                "IPv4 Default Gateway",
                "IPv4 DHCP Server",
                "IPv4 DNS Server",
                "IPv6 Address",
                "MTU",
            ]
        );
        assert_eq!(rows[1].1, "02-00-00-00-AA-01");
    }

    #[test]
    fn missing_pieces_read_as_unknown_not_errors() {
        let text = "---iface ens3\noperstate=down\ncarrier=\naddr=\n---route\n\n---now\nx\n";
        let ens3 = &parse(text)[0];
        assert_eq!(ens3.connection, Connection::Disabled);
        assert_eq!((ens3.speed(), ens3.mtu), ("Unknown".to_string(), 0));
        assert!(ens3.ipv4.is_empty() && ens3.mac.is_none());
        let unplugged = &parse("---iface ens4\noperstate=up\ncarrier=0\n")[0];
        assert_eq!(unplugged.connection, Connection::CableUnplugged);
        assert_eq!(description(None, Some("e1000e")), "Ethernet (e1000e)");
    }

    #[test]
    fn masks_cover_the_edges() {
        let ip = |prefix| Ipv4 {
            address: String::new(),
            prefix,
        };
        assert_eq!(ip(0).mask(), "0.0.0.0");
        assert_eq!(ip(22).mask(), "255.255.252.0");
        assert_eq!(ip(32).mask(), "255.255.255.255");
    }

    #[test]
    fn statuses_survive_the_wire() {
        let all = demo();
        let text = serde_json::to_string(&all).unwrap();
        let back: Vec<InterfaceStatus> = serde_json::from_str(&text).unwrap();
        assert_eq!(back, all);
    }
}
