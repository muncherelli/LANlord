//! the status of each network connection on a server, read by one read-only script that needs
//! no sudo. the Networks window and `lanlord networks` show it.

use anyhow::Result;

use crate::ssh::Session;

pub use lanlord_model::netstatus::*;

/// every interface but loopback, whatever it is called: link state and counters from sysfs,
/// addresses from `ip -j`, dns from resolved when it runs, and the dhcp server and lease time
/// from whichever client keeps them (systemd-networkd, NetworkManager or dhcpcd). anything
/// missing is left blank and reads as unknown.
const SCRIPT: &str = r#"for d in /sys/class/net/*; do
  i=${d##*/}
  [ "$i" = lo ] && continue
  echo "---iface $i"
  echo "operstate=$(cat "$d/operstate" 2>/dev/null)"
  echo "carrier=$(cat "$d/carrier" 2>/dev/null)"
  echo "speed=$(cat "$d/speed" 2>/dev/null)"
  echo "duplex=$(cat "$d/duplex" 2>/dev/null)"
  for c in tx_bytes rx_bytes tx_packets rx_packets; do
    echo "$c=$(cat "$d/statistics/$c" 2>/dev/null)"
  done
  if [ -e "$d/device/driver" ]; then echo "driver=$(basename "$(readlink "$d/device/driver")")"; fi
  echo "kind=$(ip -j -d link show dev "$i" 2>/dev/null | grep -o '"info_kind":"[^"]*"' | head -n 1 | cut -d'"' -f4)"
  echo "addr=$(ip -j addr show dev "$i" 2>/dev/null)"
  if command -v resolvectl >/dev/null 2>&1; then
    echo "dns=$(resolvectl dns "$i" 2>/dev/null | sed 's/^[^:]*://')"
  fi
  lease="/run/systemd/netif/leases/$(cat "$d/ifindex" 2>/dev/null)"
  if [ -r "$lease" ]; then
    echo "dhcp_server=$(sed -n 's/^SERVER_ADDRESS=//p' "$lease")"
    echo "lease_time=$(sed -n 's/^LIFETIME=//p' "$lease")"
  fi
  if command -v nmcli >/dev/null 2>&1; then
    nmcli -t -f DHCP4 device show "$i" 2>/dev/null \
      | sed -n 's/.*dhcp_server_identifier = /dhcp_server=/p; s/.*dhcp_lease_time = /lease_time=/p'
  fi
  if command -v dhcpcd >/dev/null 2>&1; then
    dhcpcd -U "$i" 2>/dev/null \
      | sed -n 's/^dhcp_server_identifier=/dhcp_server=/p; s/^dhcp_lease_time=/lease_time=/p'
  fi
done
echo "---route"
ip -j route show default 2>/dev/null
echo "---resolv"
grep '^nameserver' /etc/resolv.conf 2>/dev/null
echo "---now"
date +%s
exit 0
"#;

/// read the status of every network connection on the server `session` reaches.
pub fn read(session: &Session) -> Result<Vec<InterfaceStatus>> {
    let text = session.capture(&["sh".into(), "-c".into(), SCRIPT.into()])?;
    Ok(parse(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_never_names_an_interface() {
        // the external network is whichever interface holds the default route
        for name in ["eth0", "eth1", "ens", "enp"] {
            assert!(!SCRIPT.contains(name), "{name}");
        }
        assert!(SCRIPT.contains("ip -j route show default"));
    }

    #[test]
    fn the_script_runs_and_its_output_parses() {
        // runs here against whatever this machine has; only the shape is checked
        let out = std::process::Command::new("sh")
            .args(["-c", SCRIPT])
            .output()
            .unwrap();
        assert!(out.status.success());
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("---route") && text.contains("---now"));
        let all = parse(&text);
        assert!(all.iter().all(|i| i.name != "lo"));
    }
}
