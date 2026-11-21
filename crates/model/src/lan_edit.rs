//! changes to the lanlord networks as data: add, change and remove a network, and set its
//! static leases. the cli, the windows and the operations service all describe a change as an
//! [`Edit`]; `lanlord_core::lan` stores it and applies the modules.

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_yaml_ng::Value;

use crate::lan_networks::{self, find, LanNetwork, Networks};
use crate::settings::{Dhcp, Settings, StaticLease};

/// the parts of a network a `set` may change; what is `None` stays as it is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

/// the interface of the normal network, when the settings say which it is.
pub fn external_interface(settings: &Settings) -> Option<String> {
    for section in ["network", "docker"] {
        if let Some(Value::Mapping(map)) = settings.section(section) {
            if let Some(Value::String(name)) = map.get("host_interface") {
                return Some(name.clone());
            }
        }
    }
    None
}

/// what in the settings still names the network `name`: the nt4 domain's network and the
/// networks the components attach to.
pub fn network_users(settings: &Settings, name: &str) -> Vec<String> {
    let names = |value: Option<&Value>| matches!(value, Some(Value::String(n)) if n.eq_ignore_ascii_case(name));
    let mut users = Vec::new();
    if let Some(Value::Mapping(domain)) = settings.section("nt4-domain") {
        if names(domain.get("network")) {
            users.push("the nt4-domain network".to_string());
        }
    }
    if let Some(Value::Sequence(servers)) = settings.section("nt4-servers") {
        for server in servers {
            if names(server.get("network")) {
                let id = server.get("name").and_then(Value::as_str).unwrap_or("?");
                users.push(format!("the nt4 server {id}"));
            }
        }
    }
    if let Some(Value::Mapping(components)) = settings.section("components") {
        for (component, entry) in components {
            let Some(Value::Sequence(attach)) = entry.get("attach") else {
                continue;
            };
            let attached = attach.iter().any(|a| names(a.get("network")));
            if let (true, Value::String(component)) = (attached, component) {
                users.push(format!("the component {component}"));
            }
        }
    }
    users
}

/// the settings text after `edit`, checked. the networks are always written as a `networks:`
/// list, so a server with the older single-network settings is migrated by its first change.
pub fn edit_settings(plain: &str, edit: &Edit) -> Result<String> {
    edit_settings_on(plain, edit, None)
}

/// like `edit_settings`, for a server whose default-route card is `default_card`: when the
/// settings do not name the external interface, that card is taken as it, so no network may
/// use it.
pub fn edit_settings_on(plain: &str, edit: &Edit, default_card: Option<&str>) -> Result<String> {
    let mut settings = Settings::parse(plain)?;
    let Networks { mut list, .. } = lan_networks::read(&settings)?;
    let before = list.clone();
    if let Edit::Remove(name) = edit {
        let users = network_users(&settings, name);
        if !users.is_empty() {
            bail!(
                "cannot remove the network {name}: {} still uses it; change that first",
                users.join(" and ")
            );
        }
    }
    apply_edit(&mut list, edit)?;
    let external = external_interface(&settings).or_else(|| {
        default_card
            .filter(|card| !before.iter().any(|n| n.interface == *card))
            .map(String::from)
    });
    let errors = lan_networks::validate(&list, external.as_deref(), true);
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
