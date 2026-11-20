//! dns and dhcp as two lists of servers, each bound to a network, with the records and
//! reservations that belong to them. nothing here touches a machine and nothing in the app
//! uses it yet: a settings file without `dns_servers:` is read as one dns server per network
//! (and one dhcp server where the network has `dhcp:`), and [`Services::project`] gives the
//! per-network shape older gateways read back, or says why the lists do not fit it.
//! the plan is in the dns and dhcp component split report.

use std::collections::BTreeSet;
use std::fmt;
use std::net::Ipv4Addr;

use anyhow::{anyhow, Result};
use serde_yaml_ng::Value;

use crate::lan_networks::{self, LanNetwork};
use crate::settings::{
    valid_lease, valid_mac, Dhcp, Record, Settings, StaticLease, Subnet, ValidationError,
};

/// the dns cache size dnsmasq gets today; a server with another value cannot be projected.
pub const DEFAULT_CACHE_SIZE: u32 = 150;

const HOST_WORD: &str = "host";
const ALL_WORD: &str = "all";

/// which servers something applies to: every one on its network, or the listed ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Servers {
    All,
    Only(Vec<String>),
}

impl Servers {
    pub fn includes(&self, id: &str) -> bool {
        match self {
            Self::All => true,
            Self::Only(ids) => ids.iter().any(|i| i == id),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum ServersRaw {
    Word(String),
    List(Vec<String>),
}

impl TryFrom<ServersRaw> for Servers {
    type Error = String;

    fn try_from(raw: ServersRaw) -> Result<Self, String> {
        match raw {
            ServersRaw::Word(w) if w == ALL_WORD => Ok(Self::All),
            ServersRaw::Word(w) => Err(format!("{w:?} is not `all` or a list of servers")),
            ServersRaw::List(ids) => Ok(Self::Only(ids)),
        }
    }
}

impl From<Servers> for ServersRaw {
    fn from(servers: Servers) -> Self {
        match servers {
            Servers::All => Self::Word(ALL_WORD.into()),
            Servers::Only(ids) => Self::List(ids),
        }
    }
}

impl serde::Serialize for Servers {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        ServersRaw::from(self.clone()).serialize(s)
    }
}

impl<'de> serde::Deserialize<'de> for Servers {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = ServersRaw::deserialize(d)?;
        Self::try_from(raw).map_err(serde::de::Error::custom)
    }
}

/// where a service sits on its network: on the host's own address (the gateway, `.1`; the
/// default, and where every existing install is), or on an address of its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Placement {
    #[default]
    Host,
    Own(Ipv4Addr),
}

impl serde::Serialize for Placement {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Host => s.serialize_str(HOST_WORD),
            Self::Own(a) => s.serialize_str(&a.to_string()),
        }
    }
}

impl<'de> serde::Deserialize<'de> for Placement {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        if text == HOST_WORD {
            return Ok(Self::Host);
        }
        text.parse()
            .map(Self::Own)
            .map_err(|_| serde::de::Error::custom(format!("{text:?} is not `host` or an address")))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DnsServer {
    /// the default server of a network has the network's name as its id.
    pub id: String,
    pub network: String,
    #[serde(default)]
    pub placement: Placement,
    pub zone: String,
    /// empty, with `forward_to_gateway` off, means internal only.
    #[serde(default)]
    pub forwarders: Vec<Ipv4Addr>,
    /// names outside the zone go to the gateway's own resolvers, as on a network that has
    /// internet access today.
    #[serde(default)]
    pub forward_to_gateway: bool,
    #[serde(default = "default_cache_size")]
    pub cache_size: u32,
    #[serde(default)]
    pub log_queries: bool,
    #[serde(default)]
    pub local_ttl: u32,
}

fn default_cache_size() -> u32 {
    DEFAULT_CACHE_SIZE
}

fn yes() -> bool {
    true
}

/// the typed part of a record.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum DnsRecord {
    A {
        value: Ipv4Addr,
    },
    #[serde(rename = "AAAA")]
    Aaaa {
        value: std::net::Ipv6Addr,
    },
    #[serde(rename = "CNAME")]
    Cname {
        value: String,
    },
    #[serde(rename = "TXT")]
    Txt {
        value: String,
    },
    #[serde(rename = "MX")]
    Mx {
        preference: u16,
        value: String,
    },
    #[serde(rename = "SRV")]
    Srv {
        priority: u16,
        weight: u16,
        port: u16,
        value: String,
    },
    #[serde(rename = "PTR")]
    Ptr {
        value: String,
    },
    #[serde(rename = "NS")]
    Ns {
        value: String,
    },
}

impl DnsRecord {
    /// only these types carry a ttl in dnsmasq's `host-record` and `cname`.
    pub fn takes_ttl(&self) -> bool {
        matches!(
            self,
            Self::A { .. } | Self::Aaaa { .. } | Self::Cname { .. }
        )
    }
}

/// a record: a name in an explicit zone, what it says, and which servers publish it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub name: String,
    pub zone: String,
    #[serde(flatten)]
    pub record: DnsRecord,
    #[serde(default)]
    pub ttl: Option<u32>,
    pub servers: Servers,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DhcpServer {
    pub id: String,
    pub network: String,
    #[serde(default)]
    pub placement: Placement,
    #[serde(default)]
    pub range: Option<(Ipv4Addr, Ipv4Addr)>,
    pub lease: String,
    #[serde(default = "yes")]
    pub authoritative: bool,
    /// option 6, in order: the dns servers clients are told about.
    #[serde(default)]
    pub dns_servers: Vec<String>,
    /// the dns servers that get the names of leases.
    #[serde(default)]
    pub register_into: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Reservation {
    pub mac: String,
    #[serde(default)]
    pub client_id: Option<String>,
    pub address: Ipv4Addr,
    #[serde(default)]
    pub name: String,
    pub server: Servers,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// the four lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Services {
    pub dns_servers: Vec<DnsServer>,
    pub dns_records: Vec<Entry>,
    pub dhcp_servers: Vec<DhcpServer>,
    pub dhcp_reservations: Vec<Reservation>,
}

fn address_of(text: &str, what: &str) -> Result<Ipv4Addr> {
    text.trim()
        .parse()
        .map_err(|_| anyhow!("{what} {text:?} is not an ipv4 address"))
}

fn list_of<T: serde::de::DeserializeOwned>(settings: &Settings, key: &str) -> Result<Vec<T>> {
    match settings.section(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(v) => serde_yaml_ng::from_value(v.clone()).map_err(|e| anyhow!("{key}: {e}")),
    }
}

impl Services {
    /// one dns server per network, and one dhcp server where the network has `dhcp:`. an
    /// address that does not parse is an error, as the lists cannot hold it.
    pub fn synthesise(networks: &[LanNetwork]) -> Result<Self> {
        let mut out = Self::default();
        for network in networks {
            let id = network.name.clone();
            out.dns_servers.push(DnsServer {
                id: id.clone(),
                network: id.clone(),
                placement: Placement::Host,
                zone: network.zone.clone(),
                forwarders: Vec::new(),
                forward_to_gateway: network.internet.any(),
                cache_size: DEFAULT_CACHE_SIZE,
                log_queries: false,
                local_ttl: 0,
            });
            for r in &network.records {
                out.dns_records.push(Entry {
                    name: r.name.clone(),
                    zone: network.zone.clone(),
                    record: DnsRecord::A {
                        value: address_of(&r.address, "record address")?,
                    },
                    ttl: None,
                    servers: Servers::Only(vec![id.clone()]),
                });
            }
            let Some(dhcp) = &network.dhcp else { continue };
            let range = if dhcp.range_start.is_empty() && dhcp.range_end.is_empty() {
                None
            } else {
                Some((
                    address_of(&dhcp.range_start, "range start")?,
                    address_of(&dhcp.range_end, "range end")?,
                ))
            };
            out.dhcp_servers.push(DhcpServer {
                id: id.clone(),
                network: id.clone(),
                placement: Placement::Host,
                range,
                lease: dhcp.lease.clone(),
                authoritative: true,
                dns_servers: vec![id.clone()],
                register_into: vec![id.clone()],
            });
            for h in &dhcp.hosts {
                out.dhcp_reservations.push(Reservation {
                    mac: h.mac.clone(),
                    client_id: None,
                    address: address_of(&h.address, "reservation address")?,
                    name: h.name.clone(),
                    server: Servers::Only(vec![id.clone()]),
                    tags: Vec::new(),
                });
            }
        }
        Ok(out)
    }

    /// the lists in the settings, or, when there is no `dns_servers:`, the ones the networks
    /// stand for.
    pub fn read(settings: &Settings) -> Result<Self> {
        if settings.section("dns_servers").is_none() {
            return Self::synthesise(&lan_networks::read(settings)?.list);
        }
        Ok(Self {
            dns_servers: list_of(settings, "dns_servers")?,
            dns_records: list_of(settings, "dns_records")?,
            dhcp_servers: list_of(settings, "dhcp_servers")?,
            dhcp_reservations: list_of(settings, "dhcp_reservations")?,
        })
    }

    /// the networks with `zone`, `records` and `dhcp` filled from each network's default pair,
    /// which is what an older gateway reads; or, when these lists say more than that shape can,
    /// every reason why not. a network that has no dns server keeps what it had.
    pub fn project(&self, networks: &[LanNetwork]) -> Result<Vec<LanNetwork>, NotExpressible> {
        let mut why = Vec::new();
        let mut out = networks.to_vec();
        for network in &mut out {
            self.project_one(network, &mut why);
        }
        self.unplaced(networks, &mut why);
        if why.is_empty() {
            Ok(out)
        } else {
            Err(NotExpressible(why))
        }
    }

    fn project_one(&self, network: &mut LanNetwork, why: &mut Vec<String>) {
        let name = &network.name;
        let dns: Vec<&DnsServer> = self
            .dns_servers
            .iter()
            .filter(|s| &s.network == name)
            .collect();
        if dns.len() > 1 {
            why.push(format!("{name} has {} dns servers", dns.len()));
        }
        let default = dns.iter().find(|s| s.placement == Placement::Host).copied();
        for s in &dns {
            if s.placement != Placement::Host {
                why.push(format!("dns server {} has its own address", s.id));
            }
            if !s.forwarders.is_empty() {
                why.push(format!("dns server {} has forwarders", s.id));
            }
            if s.forward_to_gateway != network.internet.any() {
                why.push(format!(
                    "dns server {} forwards other than the network's internet setting says",
                    s.id
                ));
            }
            if s.cache_size != DEFAULT_CACHE_SIZE || s.log_queries || s.local_ttl != 0 {
                why.push(format!("dns server {} has its own cache settings", s.id));
            }
        }
        if let Some(server) = default {
            network.zone.clone_from(&server.zone);
            network.records = self.legacy_records(server, why);
        }
        let dhcp: Vec<&DhcpServer> = self
            .dhcp_servers
            .iter()
            .filter(|s| &s.network == name)
            .collect();
        if dhcp.len() > 1 {
            why.push(format!("{name} has {} dhcp servers", dhcp.len()));
        }
        let subnet = network.subnet();
        network.dhcp = dhcp
            .first()
            .map(|s| self.legacy_dhcp(s, default, subnet, why));
    }

    fn legacy_records(&self, server: &DnsServer, why: &mut Vec<String>) -> Vec<Record> {
        let mut records = Vec::new();
        for e in &self.dns_records {
            if !e.servers.includes(&server.id) || e.zone != server.zone {
                continue;
            }
            match (&e.record, e.ttl) {
                (DnsRecord::A { value }, None) => records.push(Record {
                    name: e.name.clone(),
                    address: value.to_string(),
                }),
                _ => why.push(format!("record {} is not a plain A record", e.name)),
            }
        }
        records
    }

    /// does this reservation belong to this server? `all` means the servers of the network
    /// whose subnet holds the address.
    fn applies(r: &Reservation, s: &DhcpServer, subnet: Option<Subnet>) -> bool {
        r.server.includes(&s.id)
            && match r.server {
                Servers::All => subnet.is_some_and(|n| n.contains(r.address)),
                Servers::Only(_) => true,
            }
    }

    fn legacy_dhcp(
        &self,
        s: &DhcpServer,
        dns: Option<&DnsServer>,
        subnet: Option<Subnet>,
        why: &mut Vec<String>,
    ) -> Dhcp {
        let default_dns: Vec<String> = dns.map(|d| d.id.clone()).into_iter().collect();
        if s.dns_servers != default_dns {
            why.push(format!("dhcp server {} hands out other dns servers", s.id));
        }
        if s.register_into != default_dns {
            why.push(format!("dhcp server {} registers names elsewhere", s.id));
        }
        if s.placement != Placement::Host {
            why.push(format!("dhcp server {} has its own address", s.id));
        }
        if !s.authoritative {
            why.push(format!("dhcp server {} is not authoritative", s.id));
        }
        let mut hosts = Vec::new();
        for r in self
            .dhcp_reservations
            .iter()
            .filter(|r| Self::applies(r, s, subnet))
        {
            if r.client_id.is_some() || !r.tags.is_empty() || r.name.is_empty() {
                why.push(format!(
                    "reservation {} needs more than a name and a mac",
                    r.mac
                ));
            }
            hosts.push(StaticLease {
                name: r.name.clone(),
                mac: r.mac.clone(),
                address: r.address.to_string(),
            });
        }
        let (range_start, range_end) = s
            .range
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .unwrap_or_default();
        Dhcp {
            range_start,
            range_end,
            lease: s.lease.clone(),
            hosts,
        }
    }

    /// things that name a network, server or zone the projection never reached.
    fn unplaced(&self, networks: &[LanNetwork], why: &mut Vec<String>) {
        let known = |n: &str| networks.iter().any(|x| x.name == n);
        for s in &self.dns_servers {
            if !known(&s.network) {
                why.push(format!(
                    "dns server {} is on unknown network {}",
                    s.id, s.network
                ));
            }
        }
        for s in &self.dhcp_servers {
            if !known(&s.network) {
                why.push(format!(
                    "dhcp server {} is on unknown network {}",
                    s.id, s.network
                ));
            }
        }
        for e in &self.dns_records {
            let placed = self.dns_servers.iter().any(|s| {
                s.placement == Placement::Host && s.zone == e.zone && e.servers.includes(&s.id)
            });
            if !placed {
                why.push(format!("record {} is on no default dns server", e.name));
            }
        }
        for r in &self.dhcp_reservations {
            let placed = self.dhcp_servers.iter().any(|s| {
                let subnet = networks
                    .iter()
                    .find(|n| n.name == s.network)
                    .and_then(LanNetwork::subnet);
                Self::applies(r, s, subnet)
            });
            if !placed {
                why.push(format!("reservation {} is on no dhcp server", r.mac));
            }
        }
    }
}

/// the lists say more than the per-network shape of older gateways can hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotExpressible(pub Vec<String>);

impl fmt::Display for NotExpressible {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "an older gateway cannot take this: {}; update the gateway first",
            self.0.join("; ")
        )
    }
}

impl std::error::Error for NotExpressible {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    pub error: ValidationError,
}

fn find(out: &mut Vec<Finding>, severity: Severity, path: &str, message: String) {
    out.push(Finding {
        severity,
        error: ValidationError {
            path: path.into(),
            message,
        },
    });
}

/// `_service._proto.name`, as a srv record's name must be.
fn valid_srv_name(name: &str) -> bool {
    let mut parts = name.split('.');
    let part = |p: Option<&str>| p.is_some_and(|p| p.len() > 1 && p.starts_with('_'));
    part(parts.next()) && part(parts.next()) && parts.next().is_some()
}

/// the length of a finite lease in minutes, so that dnsmasq's two minute floor can be checked.
fn lease_minutes(lease: &str) -> Option<u64> {
    let lease = lease.to_ascii_lowercase();
    let (digits, unit) = lease.split_at(lease.len().saturating_sub(1));
    let n: u64 = digits.parse().ok()?;
    match unit {
        "m" => Some(n),
        "h" => Some(n * 60),
        "d" => Some(n * 1440),
        _ => None,
    }
}

impl Services {
    /// everything wrong with these lists on these networks.
    pub fn validate(&self, networks: &[LanNetwork]) -> Vec<Finding> {
        let mut out = Vec::new();
        let subnet_of = |n: &str| {
            networks
                .iter()
                .find(|x| x.name == n)
                .and_then(LanNetwork::subnet)
        };
        self.check_dns_servers(networks, &subnet_of, &mut out);
        self.check_dhcp_servers(networks, &subnet_of, &mut out);
        self.check_ids(&mut out);
        self.check_reservations(networks, &subnet_of, &mut out);
        self.check_overlaps(&subnet_of, &mut out);
        self.check_records(&mut out);
        self.check_forwarders(networks, &mut out);
        out
    }

    /// an id names one server: no two dns servers share one, nor two dhcp servers.
    fn check_ids(&self, out: &mut Vec<Finding>) {
        let mut seen = BTreeSet::new();
        for (i, s) in self.dns_servers.iter().enumerate() {
            if !seen.insert(&s.id) {
                let message = format!("two dns servers are called {}", s.id);
                find(out, Severity::Error, &format!("dns_servers[{i}]"), message);
            }
        }
        let mut seen = BTreeSet::new();
        for (i, s) in self.dhcp_servers.iter().enumerate() {
            if !seen.insert(&s.id) {
                let message = format!("two dhcp servers are called {}", s.id);
                find(out, Severity::Error, &format!("dhcp_servers[{i}]"), message);
            }
        }
    }

    fn server_addresses(&self, network: &str, networks: &[LanNetwork]) -> Vec<Ipv4Addr> {
        let own = networks
            .iter()
            .find(|n| n.name == network)
            .and_then(LanNetwork::ip);
        let mut found: Vec<Ipv4Addr> = own.into_iter().collect();
        for s in self.dns_servers.iter().filter(|s| s.network == network) {
            if let Placement::Own(a) = s.placement {
                found.push(a);
            }
        }
        for s in self.dhcp_servers.iter().filter(|s| s.network == network) {
            if let Placement::Own(a) = s.placement {
                found.push(a);
            }
        }
        found
    }

    /// where every dns and dhcp server sits: an own address must be a host address of the
    /// subnet and not the gateway's. a dns and a dhcp server may share one, two of the same kind
    /// may not (two dns servers would both hold port 53 on it).
    fn check_dns_servers(
        &self,
        networks: &[LanNetwork],
        subnet_of: &dyn Fn(&str) -> Option<Subnet>,
        out: &mut Vec<Finding>,
    ) {
        let gateway = |n: &str| {
            networks
                .iter()
                .find(|x| x.name == n)
                .and_then(LanNetwork::ip)
        };
        // (network, address, is a dns server) of the own addresses claimed so far
        let mut own: Vec<(&str, Ipv4Addr, bool)> = Vec::new();
        let mut dns_held = BTreeSet::new();
        let services = self
            .dns_servers
            .iter()
            .enumerate()
            .map(|(i, s)| {
                (
                    format!("dns_servers[{i}]"),
                    &s.id,
                    &s.network,
                    s.placement,
                    true,
                )
            })
            .chain(self.dhcp_servers.iter().enumerate().map(|(i, s)| {
                (
                    format!("dhcp_servers[{i}]"),
                    &s.id,
                    &s.network,
                    s.placement,
                    false,
                )
            }));
        for (path, id, network, placement, is_dns) in services {
            let Some(subnet) = subnet_of(network) else {
                let message = if networks.iter().any(|n| &n.name == network) {
                    format!(
                        "{network} has no usable subnet: its address needs a prefix from /8 to /30"
                    )
                } else {
                    format!("{id} is on no known network")
                };
                find(out, Severity::Error, &path, message);
                continue;
            };
            let address = match placement {
                Placement::Host => gateway(network),
                Placement::Own(a) => Some(a),
            };
            let Some(a) = address else { continue };
            if let Placement::Own(a) = placement {
                if !subnet.contains(a) || a == subnet.network || a == subnet.broadcast() {
                    let message = format!("{a} is not a host address of {subnet}");
                    find(out, Severity::Error, &path, message);
                } else if Some(a) == gateway(network) {
                    let message = format!("{a} is the gateway's address; use the host placement");
                    find(out, Severity::Error, &path, message);
                }
                // one dns and one dhcp server may share an own address, two of a kind may not
                let taken = !is_dns
                    && own
                        .iter()
                        .any(|(n, x, dns)| *n == network.as_str() && *x == a && !*dns);
                if taken {
                    let message = format!("another dhcp server already holds {a}");
                    find(out, Severity::Error, &path, message);
                }
                own.push((network, a, is_dns));
            }
            if is_dns && !dns_held.insert((network, a)) {
                let message = format!("another dns server already holds {a}:53");
                find(out, Severity::Error, &path, message);
            }
        }
    }

    fn check_dhcp_servers(
        &self,
        networks: &[LanNetwork],
        subnet_of: &dyn Fn(&str) -> Option<Subnet>,
        out: &mut Vec<Finding>,
    ) {
        let mut seen = BTreeSet::new();
        for (i, s) in self.dhcp_servers.iter().enumerate() {
            let path = format!("dhcp_servers[{i}]");
            if !seen.insert(&s.network) {
                let message = format!("two dhcp servers serve {}", s.network);
                find(out, Severity::Warning, &path, message);
            }
            if !valid_lease(&s.lease) {
                let message = format!("lease {:?} is not 30m, 12h, 7d or infinite", s.lease);
                find(out, Severity::Error, &path, message);
            } else if lease_minutes(&s.lease).is_some_and(|m| m < 2) {
                let message = "dnsmasq needs a lease of at least 2 minutes".into();
                find(out, Severity::Error, &path, message);
            }
            if let (Some((a, b)), Some(subnet)) = (s.range, subnet_of(&s.network)) {
                self.check_range(s, (a, b), subnet, networks, &path, out);
            }
            self.check_option6(s, &path, out);
            for id in &s.register_into {
                if !self.dns_servers.iter().any(|d| &d.id == id) {
                    let message = format!("register_into names {id}, which is not a dns server");
                    find(out, Severity::Error, &path, message);
                }
            }
        }
    }

    fn check_range(
        &self,
        s: &DhcpServer,
        (a, b): (Ipv4Addr, Ipv4Addr),
        subnet: Subnet,
        networks: &[LanNetwork],
        path: &str,
        out: &mut Vec<Finding>,
    ) {
        if a > b || !subnet.contains(a) || !subnet.contains(b) {
            let message = format!("range {a} to {b} is not inside {subnet}");
            find(out, Severity::Error, path, message);
        }
        for end in [a, b] {
            if subnet.prefix < 31 && (end == subnet.network || end == subnet.broadcast()) {
                let message = format!("{end} is the network or broadcast address of {subnet}");
                find(out, Severity::Error, path, message);
            }
        }
        for addr in self.server_addresses(&s.network, networks) {
            if a <= addr && addr <= b {
                let message = format!("range holds the server address {addr}");
                find(out, Severity::Error, path, message);
            }
        }
        for r in self
            .dhcp_reservations
            .iter()
            .filter(|r| Self::applies(r, s, Some(subnet)))
        {
            if a <= r.address && r.address <= b {
                let message = format!("range holds the reservation {}", r.address);
                find(out, Severity::Error, path, message);
            }
        }
    }

    fn check_option6(&self, s: &DhcpServer, path: &str, out: &mut Vec<Finding>) {
        if s.dns_servers.is_empty() {
            find(
                out,
                Severity::Warning,
                path,
                "clients get no dns server".into(),
            );
            return;
        }
        let mut internal_only = true;
        for id in &s.dns_servers {
            let Some(d) = self.dns_servers.iter().find(|d| &d.id == id) else {
                find(
                    out,
                    Severity::Error,
                    path,
                    format!("dns server {id} does not exist"),
                );
                continue;
            };
            internal_only &= d.forwarders.is_empty() && !d.forward_to_gateway;
            if d.network != s.network {
                let message = format!("dns server {id} is on another network and routing is off");
                find(out, Severity::Warning, path, message);
            }
        }
        if internal_only {
            find(
                out,
                Severity::Info,
                path,
                "clients get no internet names".into(),
            );
        }
    }

    fn check_reservations(
        &self,
        networks: &[LanNetwork],
        subnet_of: &dyn Fn(&str) -> Option<Subnet>,
        out: &mut Vec<Finding>,
    ) {
        for (i, r) in self.dhcp_reservations.iter().enumerate() {
            let path = format!("dhcp_reservations[{i}]");
            if !valid_mac(&r.mac) {
                let message = format!("{:?} is not a mac address", r.mac);
                find(out, Severity::Error, &path, message);
            }
            if let Servers::Only(ids) = &r.server {
                for id in ids
                    .iter()
                    .filter(|id| !self.dhcp_servers.iter().any(|s| &s.id == *id))
                {
                    let message = format!("{id} is not a dhcp server");
                    find(out, Severity::Error, &path, message);
                }
            }
            let mut placed = false;
            for s in self
                .dhcp_servers
                .iter()
                .filter(|s| r.server.includes(&s.id))
            {
                let subnet = subnet_of(&s.network);
                if matches!(r.server, Servers::All)
                    && !subnet.is_some_and(|n| n.contains(r.address))
                {
                    continue;
                }
                placed = true;
                let Some(subnet) = subnet else { continue };
                if !subnet.contains(r.address) {
                    let message = format!("{} is outside {subnet}", r.address);
                    find(out, Severity::Error, &path, message);
                } else if let Some(message) =
                    self.reserved_address(r.address, subnet, &s.network, networks)
                {
                    find(out, Severity::Error, &path, message);
                }
            }
            if !placed && matches!(r.server, Servers::All) && !self.dhcp_servers.is_empty() {
                let message = format!("{} is not inside a network with a dhcp server", r.address);
                find(out, Severity::Error, &path, message);
            }
        }
    }

    /// why a host may not be given `address` on the network: it is the gateway's, a server's
    /// own address, or the network or broadcast address.
    fn reserved_address(
        &self,
        address: Ipv4Addr,
        subnet: Subnet,
        network: &str,
        networks: &[LanNetwork],
    ) -> Option<String> {
        let gateway = networks
            .iter()
            .find(|n| n.name == network)
            .and_then(LanNetwork::ip);
        if Some(address) == gateway {
            Some(format!("{address} is the gateway's address"))
        } else if self.server_addresses(network, networks).contains(&address) {
            Some(format!("{address} is a server's address"))
        } else if subnet.prefix < 31 && (address == subnet.network || address == subnet.broadcast())
        {
            Some(format!(
                "{address} is the network or broadcast address of {subnet}"
            ))
        } else {
            None
        }
    }

    /// dhcp servers on one network must not hand out the same addresses: their ranges must not
    /// overlap, and no address, mac or client id may be reserved twice.
    fn check_overlaps(&self, subnet_of: &dyn Fn(&str) -> Option<Subnet>, out: &mut Vec<Finding>) {
        for (i, a) in self.dhcp_servers.iter().enumerate() {
            for (j, b) in self.dhcp_servers.iter().enumerate().skip(i + 1) {
                if a.network != b.network {
                    continue;
                }
                if let (Some((a1, a2)), Some((b1, b2))) = (a.range, b.range) {
                    if a1 <= b2 && b1 <= a2 {
                        let message = format!("the ranges of {} and {} overlap", a.id, b.id);
                        find(out, Severity::Error, &format!("dhcp_servers[{j}]"), message);
                    }
                }
            }
        }
        for (i, a) in self.dhcp_reservations.iter().enumerate() {
            for (j, b) in self.dhcp_reservations.iter().enumerate().skip(i + 1) {
                let same = a.address == b.address
                    || a.mac.eq_ignore_ascii_case(&b.mac)
                    || (a.client_id.is_some() && a.client_id == b.client_id);
                // two servers of one network share its clients, so a clash across them counts
                let shared = self.dhcp_servers.iter().any(|s| {
                    let subnet = subnet_of(&s.network);
                    Self::applies(a, s, subnet)
                        && self
                            .dhcp_servers
                            .iter()
                            .any(|t| t.network == s.network && Self::applies(b, t, subnet))
                });
                if same && shared {
                    let message =
                        format!("{} or its mac or client id is reserved twice", a.address);
                    find(
                        out,
                        Severity::Error,
                        &format!("dhcp_reservations[{j}]"),
                        message,
                    );
                }
            }
        }
    }

    fn check_records(&self, out: &mut Vec<Finding>) {
        for (i, e) in self.dns_records.iter().enumerate() {
            let path = format!("dns_records[{i}]");
            if e.ttl.is_some() && !e.record.takes_ttl() {
                let message = format!("{} cannot have a ttl", e.name);
                find(out, Severity::Error, &path, message);
            }
            if matches!(e.record, DnsRecord::Srv { .. }) && !valid_srv_name(&e.name) {
                let message = format!("{} is not _service._proto.name", e.name);
                find(out, Severity::Error, &path, message);
            }
            if let DnsRecord::Cname { value } = &e.record {
                let local = self.dns_records.iter().any(|o| {
                    let full = format!("{}.{}", o.name, o.zone);
                    (&o.name == value || &full == value) && !std::ptr::eq(o, e)
                });
                if !local {
                    let message = format!("{value} is not a name these servers know");
                    find(out, Severity::Error, &path, message);
                }
            }
            if let Servers::Only(ids) = &e.servers {
                for id in ids
                    .iter()
                    .filter(|id| !self.dns_servers.iter().any(|d| &d.id == *id))
                {
                    find(
                        out,
                        Severity::Error,
                        &path,
                        format!("{id} is not a dns server"),
                    );
                }
            }
        }
    }

    /// a server forwarding to itself, or a cycle among servers that have their own address
    /// (a default server's address is the network's, which only the caller's networks know).
    fn check_forwarders(&self, networks: &[LanNetwork], out: &mut Vec<Finding>) {
        // the gateway's own address answers as a lanlord dns server, so forwarding to it loops
        for (i, s) in self.dns_servers.iter().enumerate() {
            let gateway = networks
                .iter()
                .find(|n| n.name == s.network)
                .and_then(LanNetwork::ip);
            if gateway.is_some_and(|g| s.forwarders.contains(&g)) {
                let message = format!("{} forwards to the gateway's own address", s.id);
                find(out, Severity::Error, &format!("dns_servers[{i}]"), message);
            }
        }
        let owners: Vec<(usize, Ipv4Addr)> = self
            .dns_servers
            .iter()
            .enumerate()
            .filter_map(|(i, s)| match s.placement {
                Placement::Own(a) => Some((i, a)),
                Placement::Host => None,
            })
            .collect();
        for (i, s) in self.dns_servers.iter().enumerate() {
            let mut here = i;
            let mut steps = 0;
            // follow the first forwarder that is another of these servers
            while let Some(next) = self.dns_servers[here]
                .forwarders
                .iter()
                .find_map(|f| owners.iter().find(|(_, a)| a == f).map(|(j, _)| *j))
            {
                steps += 1;
                if next == i || steps > self.dns_servers.len() {
                    let message = format!("{} forwards in a loop", s.id);
                    find(out, Severity::Error, &format!("dns_servers[{i}]"), message);
                    break;
                }
                here = next;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(name: &str, prefix: &str) -> LanNetwork {
        let mut n = LanNetwork::new(name, "eth1", &format!("{prefix}.1/24"));
        n.records = vec![Record {
            name: "ntsrv1".into(),
            address: format!("{prefix}.10"),
        }];
        n.dhcp = Some(Dhcp {
            range_start: format!("{prefix}.100"),
            range_end: format!("{prefix}.200"),
            lease: "7d".into(),
            hosts: vec![StaticLease {
                name: "pc1".into(),
                mac: "02:00:00:00:00:01".into(),
                address: format!("{prefix}.20"),
            }],
        });
        n
    }

    fn two() -> Vec<LanNetwork> {
        let mut lab = net("lab", "203.0.113");
        lab.dhcp = None;
        vec![net("retronet", "192.0.2"), lab]
    }

    /// two networks that both have dhcp.
    fn both() -> Vec<LanNetwork> {
        vec![net("retronet", "192.0.2"), net("lab", "203.0.113")]
    }

    fn errors(s: &Services, n: &[LanNetwork]) -> Vec<String> {
        s.validate(n)
            .into_iter()
            .filter(|f| f.severity == Severity::Error)
            .map(|f| f.error.message)
            .collect()
    }

    fn extra(id: &str, addr: [u8; 4], to: [u8; 4]) -> DnsServer {
        DnsServer {
            id: id.into(),
            network: "retronet".into(),
            placement: Placement::Own(Ipv4Addr::from(addr)),
            zone: "retronet".into(),
            forwarders: vec![Ipv4Addr::from(to)],
            forward_to_gateway: false,
            cache_size: DEFAULT_CACHE_SIZE,
            log_queries: false,
            local_ttl: 0,
        }
    }

    fn entry(name: &str, record: DnsRecord) -> Entry {
        Entry {
            name: name.into(),
            zone: "retronet".into(),
            record,
            ttl: None,
            servers: Servers::All,
        }
    }

    #[test]
    fn networks_are_synthesised_into_default_servers() {
        let s = Services::synthesise(&two()).unwrap();
        let ids: Vec<&str> = s.dns_servers.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["retronet", "lab"]);
        assert_eq!(s.dhcp_servers.len(), 1);
        assert_eq!(s.dhcp_servers[0].id, "retronet");
        assert_eq!(s.dns_records.len(), 2);
        assert_eq!(s.dhcp_reservations.len(), 1);
        assert!(errors(&s, &two()).is_empty());
    }

    #[test]
    fn the_projection_gives_the_networks_back() {
        let networks = two();
        let s = Services::synthesise(&networks).unwrap();
        assert_eq!(s.project(&networks), Ok(networks));
    }

    #[test]
    fn read_synthesises_without_dns_servers_and_reads_the_lists_with_them() {
        let text = "networks:\n  - {name: alpha, interface: eth1, address: 192.0.2.1/24, zone: alpha, dhcp: {range: [192.0.2.100, 192.0.2.200], lease: 7d, hosts: [{name: pc1, mac: '02:00:00:00:00:01', address: 192.0.2.20}]}}\n";
        let s = Services::read(&Settings::parse(text).unwrap()).unwrap();
        assert_eq!(s.dns_servers.len(), 1);
        assert_eq!(s.dhcp_reservations[0].name, "pc1");
        let lists = format!(
            "{text}dns_servers:\n  - {{id: alpha, network: alpha, placement: host, zone: alpha}}\n  - {{id: extra, network: alpha, placement: 192.0.2.5, zone: alpha, forwarders: [198.51.100.53]}}\ndns_records:\n  - {{name: www, zone: alpha, type: A, value: 192.0.2.30, ttl: 60, servers: all}}\n  - {{name: web, zone: alpha, type: CNAME, value: www, servers: [alpha]}}\n  - {{name: m, zone: alpha, type: MX, preference: 10, value: mail.alpha, servers: all}}\ndhcp_reservations:\n  - {{mac: '02:00:00:00:00:02', address: 192.0.2.21, server: all, tags: [x]}}\n"
        );
        let s = Services::read(&Settings::parse(&lists).unwrap()).unwrap();
        assert_eq!(s.dns_servers.len(), 2);
        assert_eq!(
            s.dns_servers[1].placement,
            Placement::Own(Ipv4Addr::new(192, 0, 2, 5))
        );
        assert_eq!(s.dns_servers[0].cache_size, DEFAULT_CACHE_SIZE);
        assert_eq!(s.dns_records[0].servers, Servers::All);
        assert_eq!(s.dns_records[0].ttl, Some(60));
        assert!(matches!(
            s.dns_records[2].record,
            DnsRecord::Mx { preference: 10, .. }
        ));
        assert!(s.dhcp_servers.is_empty());
        let bad = format!("{text}dns_servers: []\ndns_records:\n  - {{name: a, zone: z, type: A, value: 192.0.2.1, servers: some}}\n");
        assert!(Services::read(&Settings::parse(&bad).unwrap()).is_err());
    }

    #[test]
    fn a_bad_address_cannot_be_held_in_the_lists() {
        let mut n = two();
        n[0].records[0].address = "nope".into();
        assert!(Services::synthesise(&n).is_err());
    }

    #[test]
    fn what_older_gateways_cannot_hold_is_listed() {
        let networks = two();
        let mut s = Services::synthesise(&networks).unwrap();
        let mut second = extra("extra", [192, 0, 2, 5], [198, 51, 100, 53]);
        second.cache_size = 10;
        s.dns_servers.push(second);
        s.dns_records[0].ttl = Some(60);
        s.dns_records
            .push(entry("mail", DnsRecord::Txt { value: "hi".into() }));
        let Err(NotExpressible(why)) = s.project(&networks) else {
            panic!("should not fit")
        };
        for part in [
            "2 dns servers",
            "own address",
            "forwarders",
            "cache",
            "not a plain A",
        ] {
            assert!(why.iter().any(|w| w.contains(part)), "{part}: {why:?}");
        }
        assert!(NotExpressible(why)
            .to_string()
            .ends_with("update the gateway first"));
    }

    #[test]
    fn a_second_dhcp_server_or_other_option_6_does_not_fit() {
        let networks = two();
        let mut s = Services::synthesise(&networks).unwrap();
        s.dhcp_servers[0].dns_servers = vec![];
        let mut second = s.dhcp_servers[0].clone();
        second.id = "again".into();
        s.dhcp_servers.push(second);
        let Err(NotExpressible(why)) = s.project(&networks) else {
            panic!("should not fit")
        };
        assert!(why.iter().any(|w| w.contains("2 dhcp servers")));
        assert!(why.iter().any(|w| w.contains("other dns servers")));
        let found = s.validate(&networks);
        assert!(found
            .iter()
            .any(|f| f.severity == Severity::Warning && f.error.message.contains("no dns")));
        assert!(found
            .iter()
            .any(|f| f.severity == Severity::Warning && f.error.message.contains("serve")));
        assert!(!errors(&s, &networks)
            .iter()
            .any(|m| m.contains("dhcp server")));
    }

    #[test]
    fn the_validation_table_flags_the_mistakes() {
        let networks = two();
        let mut s = Services::synthesise(&networks).unwrap();
        s.dhcp_servers[0].range =
            Some((Ipv4Addr::new(192, 0, 1, 250), Ipv4Addr::new(192, 0, 2, 30)));
        s.dhcp_reservations[0].address = Ipv4Addr::new(198, 51, 100, 9);
        s.dhcp_servers[0].lease = "1m".into();
        s.dns_records[0].ttl = Some(5);
        s.dns_records[0].record = DnsRecord::Txt { value: "x".into() };
        s.dns_records.push(entry(
            "alias",
            DnsRecord::Cname {
                value: "missing".into(),
            },
        ));
        s.dns_records.push(entry(
            "bad",
            DnsRecord::Srv {
                priority: 0,
                weight: 0,
                port: 1,
                value: "h".into(),
            },
        ));
        let msgs = errors(&s, &networks).join("|");
        for part in [
            "is not inside",
            "holds the server address",
            "is outside",
            "at least 2 minutes",
            "cannot have a ttl",
            "missing",
            "_service._proto",
        ] {
            assert!(msgs.contains(part), "{part}: {msgs}");
        }
    }

    #[test]
    fn dns_server_addresses_and_forward_loops_are_checked() {
        let networks = two();
        let mut s = Services::synthesise(&networks).unwrap();
        s.dns_servers
            .push(extra("a", [192, 0, 2, 5], [192, 0, 2, 6]));
        s.dns_servers
            .push(extra("b", [192, 0, 2, 6], [192, 0, 2, 5]));
        s.dns_servers
            .push(extra("c", [192, 0, 2, 5], [192, 0, 2, 255]));
        s.dns_servers
            .push(extra("d", [192, 0, 2, 255], [198, 51, 100, 1]));
        let msgs = errors(&s, &networks).join("|");
        assert!(msgs.contains("forwards in a loop"), "{msgs}");
        assert!(msgs.contains("already holds"), "{msgs}");
        assert!(msgs.contains("not a host address"), "{msgs}");
    }

    #[test]
    fn option_6_and_registration_must_name_dns_servers() {
        let networks = two();
        let mut s = Services::synthesise(&networks).unwrap();
        assert!(s
            .validate(&networks)
            .iter()
            .any(|f| f.severity == Severity::Info));
        s.dhcp_servers[0].dns_servers = vec!["lab".into(), "gone".into()];
        s.dhcp_servers[0].register_into = vec!["gone".into()];
        let found = s.validate(&networks);
        assert!(
            found
                .iter()
                .any(|f| f.severity == Severity::Warning
                    && f.error.message.contains("routing is off"))
        );
        let msgs = errors(&s, &networks).join("|");
        assert!(
            msgs.contains("gone does not exist") && msgs.contains("not a dns server"),
            "{msgs}"
        );
    }

    #[test]
    fn duplicate_reservations_are_errors() {
        let networks = two();
        let mut s = Services::synthesise(&networks).unwrap();
        s.dhcp_reservations.push(s.dhcp_reservations[0].clone());
        assert!(errors(&s, &networks)
            .iter()
            .any(|m| m.contains("reserved twice")));
    }

    fn dhcp(id: &str, range: ([u8; 4], [u8; 4]), placement: Placement) -> DhcpServer {
        DhcpServer {
            id: id.into(),
            network: "retronet".into(),
            placement,
            range: Some((Ipv4Addr::from(range.0), Ipv4Addr::from(range.1))),
            lease: "7d".into(),
            authoritative: true,
            dns_servers: vec!["retronet".into()],
            register_into: vec!["retronet".into()],
        }
    }

    #[test]
    fn a_service_sits_on_the_host_unless_it_says_otherwise() {
        assert_eq!(Placement::default(), Placement::Host);
        let s = Services::synthesise(&two()).unwrap();
        assert!(s.dns_servers.iter().all(|d| d.placement == Placement::Host));
        assert_eq!(s.dhcp_servers[0].placement, Placement::Host);
        let text = "networks:\n  - {name: alpha, interface: eth1, address: 192.0.2.1/24}\ndns_servers:\n  - {id: alpha, network: alpha, zone: alpha}\ndhcp_servers:\n  - {id: alpha, network: alpha, lease: 7d}\n";
        let s = Services::read(&Settings::parse(text).unwrap()).unwrap();
        assert_eq!(s.dns_servers[0].placement, Placement::Host);
        assert_eq!(s.dhcp_servers[0].placement, Placement::Host);
    }

    #[test]
    fn two_dhcp_servers_warn_and_overlapping_ranges_are_errors() {
        let networks = two();
        let mut s = Services::synthesise(&networks).unwrap();
        s.dhcp_servers.push(dhcp(
            "b",
            ([192, 0, 2, 150], [192, 0, 2, 250]),
            Placement::Own(Ipv4Addr::new(192, 0, 2, 6)),
        ));
        let found = s.validate(&networks);
        assert!(found.iter().any(
            |f| f.severity == Severity::Warning && f.error.message.contains("two dhcp servers")
        ));
        assert!(errors(&s, &networks).iter().any(|m| m.contains("overlap")));
        s.dhcp_servers[1].range =
            Some((Ipv4Addr::new(192, 0, 2, 201), Ipv4Addr::new(192, 0, 2, 250)));
        assert!(
            errors(&s, &networks).is_empty(),
            "{:?}",
            errors(&s, &networks)
        );
    }

    #[test]
    fn a_reservation_cannot_be_held_by_two_servers_of_a_network() {
        let networks = two();
        let mut s = Services::synthesise(&networks).unwrap();
        s.dhcp_servers.push(dhcp(
            "b",
            ([192, 0, 2, 201], [192, 0, 2, 250]),
            Placement::Host,
        ));
        let mut again = s.dhcp_reservations[0].clone();
        again.server = Servers::Only(vec!["b".into()]);
        s.dhcp_reservations.push(again);
        assert!(errors(&s, &networks)
            .iter()
            .any(|m| m.contains("reserved twice")));
    }

    #[test]
    fn own_addresses_must_be_free_and_inside_the_subnet() {
        let networks = two();
        let own = |a: [u8; 4]| Placement::Own(Ipv4Addr::from(a));
        let mut s = Services::synthesise(&networks).unwrap();
        s.dhcp_servers[0].placement = own([192, 0, 2, 6]);
        s.dns_servers
            .push(extra("x", [192, 0, 2, 6], [198, 51, 100, 1]));
        assert!(
            errors(&s, &networks).is_empty(),
            "{:?}",
            errors(&s, &networks)
        );
        let mut again = s.dhcp_servers[0].clone();
        again.id = "again".into();
        again.range = None;
        s.dhcp_servers.push(again);
        assert!(errors(&s, &networks)
            .iter()
            .any(|m| m.contains("another dhcp server already holds")));
        let mut s = Services::synthesise(&networks).unwrap();
        s.dns_servers
            .push(extra("g", [192, 0, 2, 1], [198, 51, 100, 1]));
        assert!(errors(&s, &networks)
            .iter()
            .any(|m| m.contains("gateway's address")));
        let mut s = Services::synthesise(&networks).unwrap();
        s.dns_servers
            .push(extra("o", [198, 51, 100, 5], [198, 51, 100, 1]));
        s.dhcp_servers[0].placement = own([192, 0, 2, 0]);
        let msgs = errors(&s, &networks);
        assert_eq!(
            msgs.iter()
                .filter(|m| m.contains("not a host address"))
                .count(),
            2,
            "{msgs:?}"
        );
    }

    #[test]
    fn an_own_dhcp_address_does_not_fit_older_gateways() {
        let networks = two();
        let mut s = Services::synthesise(&networks).unwrap();
        s.dhcp_servers[0].placement = Placement::Own(Ipv4Addr::new(192, 0, 2, 6));
        let Err(NotExpressible(why)) = s.project(&networks) else {
            panic!("should not fit")
        };
        assert!(why
            .iter()
            .any(|w| w.contains("dhcp server retronet has its own address")));
    }

    fn texts<T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + fmt::Debug>(v: &T) {
        let text = serde_yaml_ng::to_string(v).unwrap();
        let back: T = serde_yaml_ng::from_str(&text).unwrap();
        assert_eq!(&back, v, "{text}");
    }

    #[test]
    fn what_is_written_reads_back() {
        texts(&Placement::Host);
        texts(&Placement::Own(Ipv4Addr::new(192, 0, 2, 5)));
        texts(&Servers::All);
        texts(&Servers::Only(vec!["a".into(), "b".into()]));
        let records = [
            DnsRecord::A {
                value: Ipv4Addr::new(192, 0, 2, 9),
            },
            DnsRecord::Aaaa {
                value: "2001:db8::1".parse().unwrap(),
            },
            DnsRecord::Cname {
                value: "www".into(),
            },
            DnsRecord::Txt {
                value: "hello".into(),
            },
            DnsRecord::Mx {
                preference: 10,
                value: "mail".into(),
            },
            DnsRecord::Srv {
                priority: 1,
                weight: 2,
                port: 389,
                value: "dc".into(),
            },
            DnsRecord::Ptr {
                value: "host".into(),
            },
            DnsRecord::Ns {
                value: "ns1".into(),
            },
        ];
        for record in records {
            for (ttl, servers) in [
                (None, Servers::All),
                (Some(60), Servers::Only(vec!["x".into()])),
            ] {
                texts(&Entry {
                    name: "n".into(),
                    zone: "z".into(),
                    record: record.clone(),
                    ttl,
                    servers,
                });
            }
        }
        let mut dns = extra("a", [192, 0, 2, 5], [198, 51, 100, 53]);
        dns.forward_to_gateway = true;
        texts(&dns);
        texts(&dhcp(
            "b",
            ([192, 0, 2, 100], [192, 0, 2, 150]),
            Placement::Host,
        ));
    }

    #[test]
    fn an_internet_network_forwards_to_the_gateway() {
        let mut networks = two();
        networks[0].internet.enabled = true;
        let s = Services::synthesise(&networks).unwrap();
        assert!(s.dns_servers[0].forward_to_gateway);
        assert!(!s.dns_servers[1].forward_to_gateway);
        let found = s.validate(&networks);
        assert!(!found
            .iter()
            .any(|f| f.error.message.contains("no internet names")
                && f.error.path == "dhcp_servers[0]"));
        assert_eq!(s.project(&networks), Ok(networks.clone()));
        // a server that no longer forwards does not fit the network's internet setting
        let mut s = s;
        s.dns_servers[0].forward_to_gateway = false;
        let Err(NotExpressible(why)) = s.project(&networks) else {
            panic!("should not fit")
        };
        assert!(why.iter().any(|w| w.contains("internet setting")));
        let off = Services::synthesise(&two()).unwrap();
        assert!(off
            .validate(&two())
            .iter()
            .any(|f| f.error.message.contains("no internet names")));
    }

    #[test]
    fn ids_are_unique_per_kind() {
        let networks = two();
        let mut s = Services::synthesise(&networks).unwrap();
        s.dns_servers
            .push(extra("retronet", [192, 0, 2, 5], [198, 51, 100, 1]));
        s.dhcp_servers.push(dhcp(
            "retronet",
            ([192, 0, 2, 201], [192, 0, 2, 250]),
            Placement::Host,
        ));
        let msgs = errors(&s, &networks).join("|");
        assert!(
            msgs.contains("two dns servers are called retronet"),
            "{msgs}"
        );
        assert!(
            msgs.contains("two dhcp servers are called retronet"),
            "{msgs}"
        );
        // a dns and a dhcp server share the default id on purpose
        let ok = Services::synthesise(&networks).unwrap();
        assert!(errors(&ok, &networks).is_empty());
    }

    #[test]
    fn a_reservation_for_all_servers_stays_on_its_own_network() {
        let networks = both();
        let mut s = Services::synthesise(&networks).unwrap();
        s.dhcp_reservations.push(Reservation {
            mac: "02:00:00:00:00:09".into(),
            client_id: None,
            address: Ipv4Addr::new(203, 0, 113, 30),
            name: "shared".into(),
            server: Servers::All,
            tags: Vec::new(),
        });
        assert!(
            errors(&s, &networks).is_empty(),
            "{:?}",
            errors(&s, &networks)
        );
        let projected = s.project(&networks).unwrap();
        let hosts = |n: usize| projected[n].dhcp.as_ref().unwrap().hosts.len();
        assert_eq!((hosts(0), hosts(1)), (1, 2));
        s.dhcp_reservations[2].address = Ipv4Addr::new(198, 51, 100, 7);
        assert!(errors(&s, &networks)
            .iter()
            .any(|m| m.contains("not inside a network with a dhcp server")));
    }

    #[test]
    fn reservations_keep_off_the_special_addresses() {
        let networks = two();
        let mut s = Services::synthesise(&networks).unwrap();
        let put = |s: &mut Services, address: [u8; 4]| {
            s.dhcp_reservations[0].address = Ipv4Addr::from(address);
            errors(s, &networks).join("|")
        };
        assert!(put(&mut s, [192, 0, 2, 1]).contains("gateway's address"));
        s.dns_servers
            .push(extra("x", [192, 0, 2, 5], [198, 51, 100, 1]));
        assert!(put(&mut s, [192, 0, 2, 5]).contains("a server's address"));
        assert!(put(&mut s, [192, 0, 2, 0]).contains("network or broadcast"));
        assert!(put(&mut s, [192, 0, 2, 255]).contains("network or broadcast"));
        let mut other = s.dhcp_reservations[0].clone();
        other.mac = "02:00:00:00:00:02".into();
        other.address = Ipv4Addr::new(192, 0, 2, 21);
        other.client_id = Some("01:02".into());
        s.dhcp_reservations[0].address = Ipv4Addr::new(192, 0, 2, 20);
        s.dhcp_reservations[0].client_id = Some("01:02".into());
        s.dhcp_reservations.push(other);
        assert!(errors(&s, &networks)
            .iter()
            .any(|m| m.contains("client id is reserved twice")));
        s.dhcp_reservations[0].server = Servers::Only(vec!["nope".into()]);
        assert!(errors(&s, &networks)
            .iter()
            .any(|m| m.contains("nope is not a dhcp server")));
    }

    #[test]
    fn range_ends_keep_off_the_network_and_broadcast_addresses() {
        let networks = two();
        let mut s = Services::synthesise(&networks).unwrap();
        s.dhcp_servers[0].range =
            Some((Ipv4Addr::new(192, 0, 2, 0), Ipv4Addr::new(192, 0, 2, 255)));
        let found = s.validate(&networks);
        let hits: Vec<_> = found
            .iter()
            .filter(|f| f.error.message.contains("network or broadcast"))
            .collect();
        assert_eq!(hits.len(), 2);
        // the path names the list position, not the id
        assert!(hits.iter().all(|f| f.error.path == "dhcp_servers[0]"));
    }

    #[test]
    fn zero_hour_and_zero_day_leases_fail_the_floor() {
        let networks = two();
        for lease in ["0h", "0d", "0m", "1m"] {
            let mut s = Services::synthesise(&networks).unwrap();
            s.dhcp_servers[0].lease = lease.into();
            assert!(
                errors(&s, &networks)
                    .iter()
                    .any(|m| m.contains("2 minutes")),
                "{lease}"
            );
        }
        for lease in ["2m", "1h", "infinite"] {
            let mut s = Services::synthesise(&networks).unwrap();
            s.dhcp_servers[0].lease = lease.into();
            assert!(errors(&s, &networks).is_empty(), "{lease}");
        }
    }

    #[test]
    fn a_server_must_not_forward_to_the_gateway() {
        let networks = two();
        let mut s = Services::synthesise(&networks).unwrap();
        s.dns_servers[0].forwarders = vec![Ipv4Addr::new(192, 0, 2, 1)];
        assert!(errors(&s, &networks)
            .iter()
            .any(|m| m.contains("forwards to the gateway's own address")));
        s.dns_servers[0].forwarders = vec![Ipv4Addr::new(198, 51, 100, 53)];
        assert!(errors(&s, &networks).is_empty());
    }

    #[test]
    fn a_network_too_small_for_a_server_is_said_so() {
        for address in ["192.0.2.1/31", "192.0.2.1/32"] {
            let mut networks = two();
            networks[0].address = address.into();
            // the lists still synthesise, and validation names the real problem
            let s = Services::synthesise(&networks).unwrap();
            let msgs = errors(&s, &networks);
            assert!(
                msgs.iter().any(|m| m.contains("no usable subnet")),
                "{address}: {msgs:?}"
            );
            assert!(!msgs.iter().any(|m| m.contains("no known network")));
        }
    }
}
