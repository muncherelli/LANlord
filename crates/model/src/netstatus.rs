//! the status of a server's network connections, the way Windows shows a connection's Status
//! and Network Connection Details boxes: link state and speed, addresses, gateway, dns, the
//! dhcp lease and the traffic counters. it is parsed from one read-only script's output (the
//! script lives in core); nothing here touches the system.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::routes;

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

/// one ipv6 address with its prefix, like `2001:db8::10/64`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ipv6 {
    pub address: String,
    /// a privacy address that changes over time.
    pub temporary: bool,
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
    /// the ipv6 addresses in use; deprecated ones and link-local ones are left out.
    pub ipv6: Vec<Ipv6>,
    pub gateway: Option<String>,
    pub gateway6: Option<String>,
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
        let (dns6, dns4): (Vec<&String>, Vec<&String>) =
            self.dns.iter().partition(|d| d.contains(':'));
        labelled(&mut rows, "IPv4 DNS Server", dns4.into_iter().cloned());
        let (temporary, fixed): (Vec<&Ipv6>, Vec<&Ipv6>) =
            self.ipv6.iter().partition(|ip| ip.temporary);
        labelled(
            &mut rows,
            "IPv6 Address",
            fixed.iter().map(|ip| ip.address.clone()),
        );
        labelled(
            &mut rows,
            "Temporary IPv6 Address",
            temporary.iter().map(|ip| ip.address.clone()),
        );
        if let Some(gateway) = &self.gateway6 {
            rows.push(("IPv6 Default Gateway".to_string(), gateway.clone()));
        }
        labelled(&mut rows, "IPv6 DNS Server", dns6.into_iter().cloned());
        rows.push(("MTU".to_string(), self.mtu.to_string()));
        rows
    }
}

/// the first value under `label`, the rest under a blank label, the way Windows lists them.
fn labelled(rows: &mut Vec<(String, String)>, label: &str, values: impl Iterator<Item = String>) {
    for (i, value) in values.enumerate() {
        let label = if i == 0 { label } else { "" };
        rows.push((label.to_string(), value));
    }
}

/// the dns servers resolved and the resolver files name, in the order they are tried for the
/// external network when its link has none of its own.
#[derive(Default)]
struct Fallback {
    /// resolved's global servers.
    global: Vec<String>,
    /// resolved's upstream file, /run/systemd/resolve/resolv.conf.
    resolved: Vec<String>,
    /// /etc/resolv.conf.
    resolv: Vec<String>,
}

impl Fallback {
    fn servers(&self) -> Vec<String> {
        [&self.global, &self.resolved, &self.resolv]
            .into_iter()
            .map(|list| {
                list.iter()
                    .filter(|s| !s.starts_with("127.") && *s != "::1")
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .find(|list| !list.is_empty())
            .unwrap_or_default()
    }
}

/// the interfaces in `text`, the output of the read script in core: an `---iface <name>` block
/// per interface with `key=value` lines and the interface's `ip -j addr` json, then `---route`
/// and `---route6` (`ip -j route show default`), the fallback dns in `---dnsglobal`,
/// `---resolved` and `---resolv`, and `---now` (unix time).
pub fn parse(text: &str) -> Vec<InterfaceStatus> {
    let mut blocks: Vec<(String, Vec<&str>)> = Vec::new();
    let mut route = String::new();
    let mut route6 = String::new();
    let mut fallback = Fallback::default();
    let mut now = None;
    let mut section = "";
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("---iface ") {
            section = "iface";
            blocks.push((name.trim().to_string(), Vec::new()));
        } else if let Some(rest) = line.strip_prefix("---") {
            section = match rest.trim() {
                "route" => "route",
                "route6" => "route6",
                "dnsglobal" => "dnsglobal",
                "resolved" => "resolved",
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
                "route6" => route6.push_str(line),
                "dnsglobal" => fallback
                    .global
                    .extend(line.split_whitespace().map(str::to_string)),
                "resolved" | "resolv" => {
                    if let Some(server) = line.trim().strip_prefix("nameserver") {
                        let list = if section == "resolved" {
                            &mut fallback.resolved
                        } else {
                            &mut fallback.resolv
                        };
                        list.push(server.trim().to_string());
                    }
                }
                "now" => now = line.trim().parse::<i64>().ok().or(now),
                _ => {}
            }
        }
    }
    let mut defaults = routes::from_json(&route, false);
    defaults.extend(routes::from_json(&route6, true));
    let external = routes::external(&defaults);
    blocks
        .iter()
        .map(|(name, lines)| {
            let mut status = one(name, lines, now);
            status.default_route = Some(name.as_str()) == external;
            status.gateway = routes::gateway_for(&defaults, name, false).map(str::to_string);
            status.gateway6 = routes::gateway_for(&defaults, name, true).map(str::to_string);
            if status.dns.is_empty() && status.default_route {
                status.dns = fallback.servers();
            }
            status
        })
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

/// dhclient writes lease times like `2026/11/16 12:00:00`, in utc.
fn dhclient_time(text: &str) -> Option<i64> {
    chrono::NaiveDateTime::parse_from_str(text.trim(), "%Y/%m/%d %H:%M:%S")
        .ok()
        .map(|t| t.and_utc().timestamp())
}

/// one interface from its block; the routes and the fallback dns are filled in by `parse`.
fn one(name: &str, lines: &[&str], now: Option<i64>) -> InterfaceStatus {
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
            Some("inet6")
                if info["scope"].as_str() != Some("link")
                    && info["deprecated"].as_bool() != Some(true) =>
            {
                ipv6.push(Ipv6 {
                    address: format!("{local}/{prefix}"),
                    temporary: info["temporary"].as_bool() == Some(true),
                });
            }
            _ => {}
        }
    }
    let operstate = value(lines, "operstate").or_else(|| addr["operstate"].as_str());
    let carrier = value(lines, "carrier");
    let connection = match (operstate, carrier) {
        (Some("lowerlayerdown"), _) | (_, Some("0")) => Connection::CableUnplugged,
        (Some("down"), _) => Connection::Disabled,
        (Some("up"), _) | (_, Some("1")) => Connection::Connected,
        _ => Connection::Unknown,
    };
    // an address the kernel counts down came from dhcp; so did one a dhcp client has a lease
    // for (dhclient leaves the address static)
    let server = value(lines, "dhcp_server").map(str::to_string);
    let lease_time = number::<i64>(lines, "lease_time");
    let dhclient_expires = value(lines, "dhclient_expire").and_then(dhclient_time);
    let leased = server.is_some() || lease_time.is_some() || dhclient_expires.is_some();
    let dhcp = (lease_left.is_some() || leased).then(|| {
        let expires = lease_left
            .zip(now)
            .map(|(left, n)| n + left)
            .or(dhclient_expires);
        Dhcp {
            server,
            obtained: expires.zip(lease_time).map(|(e, t)| e - t),
            expires,
        }
    });
    let dns: Vec<String> = value(lines, "dns")
        .map(|d| d.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    InterfaceStatus {
        name: name.to_string(),
        description: description(value(lines, "kind"), value(lines, "driver")),
        connection,
        default_route: false,
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
        gateway: None,
        gateway6: None,
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
            [Ipv6 {
                address: "2001:db8::10/64".into(),
                temporary: false
            }],
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

    /// an `---iface` block with `addr_info` entries, in the script's format.
    fn iface(name: &str, infos: &str) -> String {
        format!(
            "---iface {name}\noperstate=up\ncarrier=1\naddr=[{{\"ifname\":\"{name}\",\"mtu\":1500,\"address\":\"02:00:00:00:bb:01\",\"addr_info\":[{infos}]}}]\n"
        )
    }

    #[test]
    fn several_ipv4_addresses_each_get_a_mask() {
        let text = iface(
            "ens3",
            r#"{"family":"inet","local":"192.0.2.10","prefixlen":24},{"family":"inet","local":"198.51.100.7","prefixlen":25}"#,
        );
        let ens3 = &parse(&text)[0];
        assert!(
            ens3.dhcp.is_none(),
            "no dynamic flag and no lease means static"
        );
        let rows = ens3.details(|t| t.to_string());
        let masks: Vec<_> = rows
            .iter()
            .filter(|(l, _)| l == "IPv4 Subnet Mask")
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(masks, ["255.255.255.0", "255.255.255.128"]);
    }

    #[test]
    fn only_the_lowest_metric_multipath_default_is_external() {
        let text = format!(
            "{}{}---route\n{}\n",
            iface("wan1", ""),
            iface("wan2", ""),
            r#"[{"dst":"default","metric":200,"nexthops":[{"gateway":"198.51.100.1","dev":"wan2"}]},{"dst":"default","metric":100,"nexthops":[{"gateway":"192.0.2.1","dev":"wan1"},{"gateway":"192.0.2.2","dev":"wan2"}]}]"#
        );
        let all = parse(&text);
        assert!(all[0].default_route && !all[1].default_route);
        assert_eq!(all[0].gateway.as_deref(), Some("192.0.2.1"));
        assert_eq!(all[1].gateway.as_deref(), Some("192.0.2.2"));
    }

    #[test]
    fn an_ipv6_only_server_has_an_external_network_too() {
        let text = format!(
            "{}---route\n[]\n---route6\n{}\n---dnsglobal\n 2001:db8::53 198.51.100.53\n",
            iface(
                "uplink",
                r#"{"family":"inet6","local":"2001:db8::10","prefixlen":64,"scope":"global"},{"family":"inet6","local":"2001:db8::bad","prefixlen":64,"scope":"global","deprecated":true},{"family":"inet6","local":"2001:db8::1234","prefixlen":64,"scope":"global","temporary":true,"dynamic":true,"valid_life_time":7200}"#
            ),
            r#"[{"dst":"default","gateway":"fe80::1","dev":"uplink","metric":1024}]"#
        );
        let uplink = &parse(&text)[0];
        assert!(uplink.default_route);
        assert_eq!(uplink.gateway6.as_deref(), Some("fe80::1"));
        assert_eq!(uplink.ipv6.len(), 2, "the deprecated address is left out");
        let rows = uplink.details(|t| t.to_string());
        let find = |label: &str| {
            rows.iter()
                .find(|(l, _)| l == label)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(find("IPv6 Address").as_deref(), Some("2001:db8::10/64"));
        assert_eq!(
            find("Temporary IPv6 Address").as_deref(),
            Some("2001:db8::1234/64")
        );
        assert_eq!(find("IPv6 Default Gateway").as_deref(), Some("fe80::1"));
        assert_eq!(find("IPv6 DNS Server").as_deref(), Some("2001:db8::53"));
        assert_eq!(find("IPv4 DNS Server").as_deref(), Some("198.51.100.53"));
    }

    #[test]
    fn dns_falls_back_from_the_link_to_resolved_then_resolv_conf() {
        let base = format!(
            "{}---route\n{}\n",
            iface("ens3", ""),
            r#"[{"dst":"default","gateway":"192.0.2.1","dev":"ens3"}]"#
        );
        let both = format!(
            "{base}---resolved\nnameserver 198.51.100.1\n---resolv\nnameserver 127.0.0.53\nnameserver 198.51.100.2\n"
        );
        assert_eq!(parse(&both)[0].dns, ["198.51.100.1"]);
        let only_resolv =
            format!("{base}---resolv\nnameserver 127.0.0.53\nnameserver 198.51.100.2\n");
        assert_eq!(parse(&only_resolv)[0].dns, ["198.51.100.2"]);
        // the per-link line sits in the interface's block, before the route sections
        let with_link = base.replace("---route", "dns= 198.51.100.9\n---route");
        let link = format!("{with_link}---resolved\nnameserver 198.51.100.1\n");
        assert_eq!(
            parse(&link)[0].dns,
            ["198.51.100.9"],
            "the link's own servers come first"
        );
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
