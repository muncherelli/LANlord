//! asking the gateway where the connection we work over comes from, so that a change that closes
//! ssh to the server is not made from a computer it would cut off. the decision is
//! `lanlord_model::lockout`; this runs the probe and reads the settings it needs.

use anyhow::Result;
use lanlord_model::lockout::{after, lockout, Lockout, Spot};
use lanlord_model::ssh_access::Change;
use lanlord_model::{lan_networks, settings::Settings};

use crate::bootstrap::AskPassword;
use crate::ssh::Session;

/// what the gateway prints about the connection: the client's address from `$SSH_CONNECTION`, the
/// interface the route to it leaves by (for a link-local address, the one after its `%`), and the mac of the next hop on it (the client itself, or
/// the router in front of it), which is what the firewall sees as `ether saddr`. it runs as the
/// login user, in the same kind of session as the commands, so the variable is there.
pub const PROBE: &str = r#"set -- $SSH_CONNECTION
addr=$1
[ -n "$addr" ] || exit 0
dev=
via=
case $addr in
  *%*)
    # a link-local address names its interface after the %, and ip route get rejects the zone
    dev=${addr#*%}
    addr=${addr%%%*}
    ;;
  *)
    route=$(ip route get "$addr" 2>/dev/null | head -n 1)
    dev=$(printf '%s\n' "$route" | awk '{ for (i = 1; i < NF; i++) if ($i == "dev") { print $(i + 1); exit } }')
    via=$(printf '%s\n' "$route" | awk '{ for (i = 1; i < NF; i++) if ($i == "via") { print $(i + 1); exit } }')
    ;;
esac
hop=${via:-$addr}
mac=
if [ -n "$dev" ]; then
  mac=$(ip neigh show "$hop" dev "$dev" 2>/dev/null | awk '{ for (i = 1; i < NF; i++) if ($i == "lladdr") { print $(i + 1); exit } }')
fi
routed=
[ -n "$via" ] && routed=1
printf 'address=%s\ninterface=%s\nmac=%s\nrouted=%s\n' "$addr" "$dev" "$mac" "$routed"
"#;

/// where this connection to `session`'s gateway comes from; none when it cannot be told (no
/// `$SSH_CONNECTION`, as on the gateway's own console).
pub fn spot(session: &Session) -> Result<Option<Spot>> {
    let text = session.capture(&["sh".to_string(), "-c".to_string(), PROBE.to_string()])?;
    Ok(Spot::parse(&text))
}

fn networks(host: &str, ask: AskPassword) -> Result<Vec<lan_networks::LanNetwork>> {
    let document = crate::remote::document(host, ask)?;
    Ok(lan_networks::read(&Settings::parse(&document.plain)?)?.list)
}

/// would the firewall of the settings as they are now keep this connection out? asked after an
/// upgrade, before the firewall of the new version is applied.
pub fn of_settings(host: &str, ask: AskPassword) -> Result<Option<Lockout>> {
    let Some(spot) = spot(&Session::new(host)?)? else {
        return Ok(None);
    };
    Ok(lockout(
        &spot,
        &networks(host, ask)?,
        crate::internet::now(),
    ))
}

/// would `change` keep this connection out? none for a change that gives access.
pub fn of_change(host: &str, change: &Change, ask: AskPassword) -> Result<Option<Lockout>> {
    if !lanlord_model::lockout::takes_access(change) {
        return Ok(None);
    }
    let Some(spot) = spot(&Session::new(host)?)? else {
        return Ok(None);
    };
    after(&spot, &networks(host, ask)?, change, crate::internet::now())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    /// run the probe with a stand-in `ip` and the given `$SSH_CONNECTION`.
    fn probe(connection: &str, ip: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "lanlord-probe-{}-{}",
            std::process::id(),
            connection.len()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let tool = dir.join("ip");
        std::fs::write(&tool, format!("#!/bin/sh\n{ip}\n")).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = Command::new("sh")
            .arg("-c")
            .arg(PROBE)
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", dir.display()))
            .env("SSH_CONNECTION", connection)
            .output()
            .unwrap();
        std::fs::remove_dir_all(&dir).ok();
        String::from_utf8(out.stdout).unwrap()
    }

    const IP: &str = r#"case "$*" in
  "route get 192.0.2.50") echo "192.0.2.50 dev eth2 src 192.0.2.1 uid 0" ;;
  "route get 198.51.100.7") echo "198.51.100.7 via 192.0.2.9 dev eth2 src 192.0.2.1 uid 0" ;;
  "neigh show 192.0.2.50 dev eth2") echo "192.0.2.50 lladdr 02:00:00:00:00:01 REACHABLE" ;;
  "neigh show 192.0.2.9 dev eth2") echo "192.0.2.9 lladdr 02:00:00:00:00:09 STALE" ;;
esac"#;

    #[test]
    fn the_probe_finds_the_interface_and_the_mac_of_a_client_on_the_network() {
        let text = probe("192.0.2.50 40000 192.0.2.1 22", IP);
        assert_eq!(
            Spot::parse(&text),
            Some(Spot {
                address: "192.0.2.50".into(),
                interface: "eth2".into(),
                mac: Some("02:00:00:00:00:01".into()),
                routed: false,
            })
        );
    }

    #[test]
    fn behind_a_router_the_mac_is_the_routers() {
        let text = probe("198.51.100.7 40000 192.0.2.1 22", IP);
        let spot = Spot::parse(&text).unwrap();
        assert_eq!(spot.interface, "eth2");
        assert_eq!(spot.mac.as_deref(), Some("02:00:00:00:00:09"));
        assert!(spot.routed);
    }

    #[test]
    fn a_link_local_client_gives_its_zone_as_the_interface() {
        let ip = r#"case "$*" in
  "neigh show fe80::50 dev eth2") echo "fe80::50 lladdr 02:00:00:00:00:05 REACHABLE" ;;
  *) echo "route get must not be asked: $*" >&2; exit 2 ;;
esac"#;
        let text = probe("fe80::50%eth2 40000 fe80::1%eth2 22", ip);
        assert_eq!(
            Spot::parse(&text),
            Some(Spot {
                address: "fe80::50".into(),
                interface: "eth2".into(),
                mac: Some("02:00:00:00:00:05".into()),
                routed: false,
            })
        );
    }

    #[test]
    fn an_address_the_route_cannot_place_is_a_place_with_no_interface() {
        let spot = Spot::parse(&probe("2001:db8::7 40000 2001:db8::1 22", "exit 1")).unwrap();
        assert_eq!(spot.address, "2001:db8::7");
        assert!(spot.interface.is_empty() && spot.mac.is_none());
    }

    #[test]
    fn without_an_ssh_connection_there_is_no_place() {
        assert_eq!(Spot::parse(&probe("", IP)), None);
    }
}
