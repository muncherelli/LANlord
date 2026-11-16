//! finds the gateway's internal (retro) network over ssh with read-only
//! commands, the same way ansible/playbooks/network.yml picks it.

use std::net::Ipv4Addr;

use anyhow::{bail, Result};

use crate::settings::Subnet;

/// the internal interface and the address the gateway has on it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Internal {
    pub interface: String,
    pub address: Ipv4Addr,
    pub prefix: u8,
}

impl Internal {
    /// the network the internal interface is on.
    pub fn subnet(&self) -> Subnet {
        Subnet::new(self.address, self.prefix)
    }
}

/// interface names that are never the internal network (network.yml's list).
const VIRTUAL: &[&str] = &[
    "lo",
    "docker",
    "br-",
    "veth",
    "virbr",
    "tun",
    "tap",
    "wg",
    "tailscale",
    "gre",
    "erspan",
    "sit",
    "ip6",
    "tunl",
    "ifb",
    "dummy",
    "teql",
    "vti",
    "ip_vti",
];

#[doc(hidden)]
pub fn is_virtual(name: &str) -> bool {
    VIRTUAL.iter().any(|prefix| name.starts_with(prefix))
}

/// `ip -4 -o addr show` lines as (interface, address, prefix); the first address per interface.
pub fn parse_addresses(output: &str) -> Vec<(String, Ipv4Addr, u8)> {
    let mut found: Vec<(String, Ipv4Addr, u8)> = Vec::new();
    for line in output.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some(at) = words.iter().position(|w| *w == "inet") else {
            continue;
        };
        let (Some(name), Some(cidr)) = (words.get(1), words.get(at + 1)) else {
            continue;
        };
        let name = name.trim_end_matches(':');
        let name = name.split('@').next().unwrap_or(name);
        let Some((address, prefix)) = cidr.split_once('/') else {
            continue;
        };
        let (Ok(address), Ok(prefix)) = (address.parse::<Ipv4Addr>(), prefix.parse::<u8>()) else {
            continue;
        };
        if prefix <= 32 && !found.iter().any(|(n, _, _)| n == name) {
            found.push((name.to_string(), address, prefix));
        }
    }
    found
}

/// the interfaces named by `dev` in `ip route show default` output.
pub fn parse_default_devices(output: &str) -> Vec<String> {
    let mut devices = Vec::new();
    for line in output
        .lines()
        .filter(|l| l.trim_start().starts_with("default"))
    {
        let words: Vec<&str> = line.split_whitespace().collect();
        if let Some(dev) = words
            .iter()
            .position(|w| *w == "dev")
            .and_then(|at| words.get(at + 1))
        {
            if !devices.iter().any(|d| d == dev) {
                devices.push(dev.to_string());
            }
        }
    }
    devices
}

/// pick the one interface with an address that is neither virtual nor the default route's.
pub fn choose(addresses: &[(String, Ipv4Addr, u8)], defaults: &[String]) -> Result<Internal> {
    let candidates: Vec<&(String, Ipv4Addr, u8)> = addresses
        .iter()
        .filter(|(name, _, _)| !is_virtual(name) && !defaults.contains(name))
        .collect();
    match candidates.as_slice() {
        [(interface, address, prefix)] => Ok(Internal {
            interface: interface.clone(),
            address: *address,
            prefix: *prefix,
        }),
        [] => bail!(
            "no internal interface found: every interface with an address is virtual or carries the default route"
        ),
        many => {
            let names: Vec<&str> = many.iter().map(|(n, _, _)| n.as_str()).collect();
            bail!(
                "could not tell the internal interface apart (candidates: {}); set network.internal_interface in your settings",
                names.join(", ")
            )
        }
    }
}
