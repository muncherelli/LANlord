//! finds the gateway's internal (retro) network over ssh with read-only
//! commands, the same way ansible/playbooks/network.yml picks it.

use anyhow::{anyhow, Result};

use crate::ssh::Session;

pub use lanlord_model::network::*;

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
    use std::net::Ipv4Addr;

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
