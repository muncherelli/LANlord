//! finds the gateway's internal (retro) network over ssh with read-only
//! commands, the same way ansible/playbooks/network.yml picks it.

use std::net::Ipv4Addr;

use anyhow::{anyhow, bail, Result};

use crate::settings::Subnet;
use crate::ssh::Session;

/// the internal interface and the address the gateway has on it.
#[derive(Clone, Debug, PartialEq, Eq)]
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

fn is_virtual(name: &str) -> bool {
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

/// ask the gateway which interface and address the internal network uses.
pub fn probe(session: &Session) -> Result<Internal> {
    let addresses = session.capture(&words(&["ip", "-4", "-o", "addr", "show"]))?;
    let routes = session
        .capture(&words(&["ip", "route", "show", "default"]))
        .map_err(|err| anyhow!("could not read the default route: {err}"))?;
    choose(
        &parse_addresses(&addresses),
        &parse_default_devices(&routes),
    )
}

fn words(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    const ADDR: &str = "\
1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever preferred_lft forever
2: eth0    inet 192.168.1.50/24 brd 192.168.1.255 scope global dynamic eth0\\       valid_lft 86000sec preferred_lft 86000sec
3: eth1    inet 172.16.120.1/24 brd 172.16.120.255 scope global eth1\\       valid_lft forever preferred_lft forever
4: docker0    inet 172.17.0.1/16 brd 172.17.255.255 scope global docker0\\       valid_lft forever preferred_lft forever
9: veth1a2b@if8    inet 10.9.9.1/30 scope global veth1a2b
";
    const ROUTE: &str = "default via 192.168.1.1 dev eth0 proto dhcp src 192.168.1.50 metric 100\n";

    #[test]
    fn parses_addresses_and_skips_noise() {
        let found = parse_addresses(ADDR);
        assert_eq!(found.len(), 5);
        assert_eq!(
            found[2],
            ("eth1".to_string(), Ipv4Addr::new(172, 16, 120, 1), 24)
        );
        assert_eq!(found[4].0, "veth1a2b");
        assert!(
            parse_addresses("garbage\n3: eth9 inet nope/24\n4: eth8 inet 1.2.3.4/40\n").is_empty()
        );
    }

    #[test]
    fn keeps_the_first_address_of_an_interface() {
        let out = "2: eth1 inet 10.0.0.1/24 scope global eth1\n2: eth1 inet 10.0.1.1/24 scope global secondary eth1\n";
        assert_eq!(parse_addresses(out).len(), 1);
        assert_eq!(parse_addresses(out)[0].1, Ipv4Addr::new(10, 0, 0, 1));
    }

    #[test]
    fn parses_default_routes() {
        let two = format!("{ROUTE}default via 10.0.0.1 dev wlan0 metric 600\nignored dev x\n");
        assert_eq!(parse_default_devices(&two), ["eth0", "wlan0"]);
        assert!(parse_default_devices("").is_empty());
        assert!(parse_default_devices("default via 1.1.1.1\n").is_empty());
    }

    #[test]
    fn picks_the_only_real_non_default_interface() {
        let internal = choose(&parse_addresses(ADDR), &parse_default_devices(ROUTE)).unwrap();
        assert_eq!(internal.interface, "eth1");
        assert_eq!(internal.address, Ipv4Addr::new(172, 16, 120, 1));
        assert_eq!(internal.prefix, 24);
        let subnet = internal.subnet();
        assert_eq!(subnet.network, Ipv4Addr::new(172, 16, 120, 0));
    }

    #[test]
    fn refuses_when_nothing_or_too_much_is_left() {
        let one = parse_addresses("2: eth0 inet 192.168.1.5/24 scope global eth0\n");
        let none = choose(&one, &["eth0".to_string()]).unwrap_err();
        assert!(none.to_string().contains("no internal interface"));
        let two = parse_addresses(
            "2: eth0 inet 192.168.1.5/24 x\n3: eth1 inet 10.0.0.1/24 x\n4: eth2 inet 10.0.1.1/24 x\n",
        );
        let err = choose(&two, &["eth0".to_string()]).unwrap_err().to_string();
        assert!(err.contains("eth1, eth2"), "{err}");
    }

    #[test]
    fn every_virtual_prefix_is_ignored() {
        for name in [
            "lo",
            "docker0",
            "br-1234",
            "veth0",
            "virbr0",
            "tun0",
            "tap1",
            "wg0",
            "tailscale0",
            "gre0",
            "erspan0",
            "sit0",
            "ip6tnl0",
            "tunl0",
            "ifb0",
            "dummy0",
            "teql0",
            "vti0",
            "ip_vti0",
        ] {
            assert!(is_virtual(name), "{name}");
        }
        assert!(!is_virtual("eth1") && !is_virtual("ens19"));
    }

    fn gateway(addr: &str, route: &str, exit: i32) -> Session {
        Session::fake(&format!(
            r#"for a; do last=$a; done
case "$last" in
  "ip -4 -o addr show") cat <<'EOF'
{addr}
EOF
  ;;
  "ip route show default") printf '%s' '{route}'; exit {exit} ;;
esac"#
        ))
    }

    #[test]
    fn probe_reads_both_commands_over_ssh() {
        let session = gateway(ADDR, ROUTE, 0);
        let internal = probe(&session).unwrap();
        assert_eq!(internal.interface, "eth1");
    }

    #[test]
    fn probe_reports_a_failing_route_command() {
        let err = probe(&gateway(ADDR, "", 3)).unwrap_err().to_string();
        assert!(err.contains("default route"), "{err}");
    }
}
