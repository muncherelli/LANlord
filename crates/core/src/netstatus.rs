//! the status of each network connection on a server, read by one read-only script that needs
//! no sudo. the Networks window and `lanlord networks` show it.

use anyhow::Result;

use crate::ssh::Session;

pub use lanlord_model::netstatus::*;

/// systemd-networkd's lease file: the dhcp server and the lease time.
const NETWORKD_SED: &str = "s/^SERVER_ADDRESS=/dhcp_server=/p; s/^LIFETIME=/lease_time=/p";

/// NetworkManager's `nmcli -t -f DHCP4 device show` lines.
const NM_SED: &str =
    "s/.*dhcp_server_identifier = /dhcp_server=/p; s/.*dhcp_lease_time = /lease_time=/p";

/// dhcpcd's `dhcpcd -U` lines.
const DHCPCD_SED: &str =
    "s/^dhcp_server_identifier=/dhcp_server=/p; s/^dhcp_lease_time=/lease_time=/p";

/// the last lease block for interface `ifc` in a dhclient leases file.
const DHCLIENT_AWK: &str = r#"/^lease *\{/ { inb = 1; cur = ""; hit = 0; next }
inb && index($0, "interface \"" ifc "\";") { hit = 1 }
inb && /^\}/ { inb = 0; if (hit) last = cur; next }
inb { cur = cur $0 "\n" }
END { printf "%s", last }"#;

/// the dhcp server, lease time and expiry in a dhclient lease block.
const DHCLIENT_SED: &str = "s/^ *option dhcp-server-identifier \\([^;]*\\);/dhcp_server=\\1/p; s/^ *option dhcp-lease-time \\([^;]*\\);/lease_time=\\1/p; s/^ *expire [0-9] \\([^;]*\\);/dhclient_expire=\\1/p";

/// every interface but loopback (and the files in /sys/class/net that are not interfaces),
/// whatever it is called: link state and counters from sysfs, addresses from `ip -j`, dns from
/// resolved when it runs, and the dhcp server and lease from whichever client keeps them
/// (systemd-networkd, NetworkManager, dhcpcd or dhclient). anything missing is left blank and
/// reads as unknown.
fn script() -> String {
    format!(
        r#"for d in /sys/class/net/*; do
  i=${{d##*/}}
  [ -e "$d/ifindex" ] || continue
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
  if [ -r "$lease" ]; then sed -n '{networkd}' "$lease"; fi
  if command -v nmcli >/dev/null 2>&1; then
    nmcli -t -f DHCP4 device show "$i" 2>/dev/null | sed -n '{nm}'
  fi
  if command -v dhcpcd >/dev/null 2>&1; then
    dhcpcd -U "$i" 2>/dev/null | sed -n '{dhcpcd}'
  fi
  for f in /var/lib/dhcp/dhclient*.leases /var/lib/dhclient/dhclient*.leases; do
    [ -r "$f" ] || continue
    awk -v ifc="$i" '{awk}' "$f" | sed -n '{dhclient}'
  done
done
echo "---route"
ip -j route show default 2>/dev/null
echo "---route6"
ip -j -6 route show default 2>/dev/null
echo "---dnsglobal"
if command -v resolvectl >/dev/null 2>&1; then resolvectl dns 2>/dev/null | sed -n 's/^Global://p'; fi
echo "---resolved"
grep '^nameserver' /run/systemd/resolve/resolv.conf 2>/dev/null
echo "---resolv"
grep '^nameserver' /etc/resolv.conf 2>/dev/null
echo "---now"
date +%s
exit 0
"#,
        networkd = NETWORKD_SED,
        nm = NM_SED,
        dhcpcd = DHCPCD_SED,
        awk = DHCLIENT_AWK,
        dhclient = DHCLIENT_SED,
    )
}

/// read the status of every network connection on the server `session` reaches.
pub fn read(session: &Session) -> Result<Vec<InterfaceStatus>> {
    let text = session.capture(&["sh".into(), "-c".into(), script()])?;
    Ok(parse(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// what `program` (sed -n or awk) prints for `input`.
    fn run(cmd: &str, program: &str, input: &str, args: &[&str]) -> String {
        use std::io::Write;
        let mut child = std::process::Command::new(cmd)
            .args(args)
            .arg(program)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
    }

    fn sed(program: &str, input: &str) -> String {
        run("sed", program, input, &["-n"])
    }

    #[test]
    fn the_script_never_names_an_interface() {
        // the external network is whichever interface holds the default route
        let script = script();
        for name in ["eth0", "eth1", "ens", "enp"] {
            assert!(!script.contains(name), "{name}");
        }
        assert!(script.contains("ip -j route show default"));
        assert!(script.contains("ip -j -6 route show default"));
        assert!(script.contains(r#"[ -e "$d/ifindex" ] || continue"#));
    }

    #[test]
    fn each_dhcp_client_reads_as_the_same_two_keys() {
        let want = "dhcp_server=192.0.2.1\nlease_time=86400\n";
        let networkd = "# This is private data. Do not parse.\nADDRESS=192.0.2.10\nSERVER_ADDRESS=192.0.2.1\nLIFETIME=86400\n";
        assert_eq!(sed(NETWORKD_SED, networkd), want);
        let nm = "DHCP4.OPTION[4]:dhcp_lease_time = 86400\nDHCP4.OPTION[5]:dhcp_server_identifier = 192.0.2.1\nDHCP4.OPTION[6]:domain_name_servers = 198.51.100.53\n";
        let got = sed(NM_SED, nm);
        assert!(got.contains("dhcp_server=192.0.2.1\n") && got.contains("lease_time=86400\n"));
        let dhcpcd = "broadcast_address='192.0.2.255'\ndhcp_lease_time='86400'\ndhcp_server_identifier='192.0.2.1'\n";
        let got = sed(DHCPCD_SED, dhcpcd);
        assert!(got.contains("dhcp_server='192.0.2.1'") && got.contains("lease_time='86400'"));
        // the quotes are taken off when the value is read
        let parsed = parse(&format!("---iface ens3\n{got}"));
        assert_eq!(
            parsed[0].dhcp.as_ref().unwrap().server.as_deref(),
            Some("192.0.2.1")
        );
    }

    #[test]
    fn dhclient_gives_the_last_lease_for_the_interface() {
        let leases = "lease {\n  interface \"ens3\";\n  fixed-address 192.0.2.9;\n  option dhcp-lease-time 3600;\n  option dhcp-server-identifier 192.0.2.254;\n  expire 1 2026/11/16 10:00:00;\n}\nlease {\n  interface \"ens4\";\n  option dhcp-server-identifier 198.51.100.1;\n}\nlease {\n  interface \"ens3\";\n  fixed-address 192.0.2.10;\n  option dhcp-lease-time 86400;\n  option dhcp-server-identifier 192.0.2.1;\n  renew 1 2026/11/16 18:00:00;\n  expire 2 2026/11/17 12:00:00;\n}\n";
        let block = run("awk", DHCLIENT_AWK, leases, &["-v", "ifc=ens3"]);
        let keys = sed(DHCLIENT_SED, &block);
        assert_eq!(
            keys,
            "lease_time=86400\ndhcp_server=192.0.2.1\ndhclient_expire=2026/11/17 12:00:00\n"
        );
        // dhclient leaves the address static, so the lease alone says dhcp is on
        let text = format!(
            "---iface ens3\noperstate=up\n{keys}addr=[{{\"ifname\":\"ens3\",\"mtu\":1500,\"addr_info\":[{{\"family\":\"inet\",\"local\":\"192.0.2.10\",\"prefixlen\":24}}]}}]\n"
        );
        let dhcp = parse(&text)[0].dhcp.clone().unwrap();
        assert_eq!(dhcp.expires, Some(1_794_916_800));
        assert_eq!(dhcp.obtained, Some(1_794_916_800 - 86_400));
    }

    #[test]
    fn the_script_runs_and_its_output_parses() {
        // runs here against whatever this machine has; only the shape is checked
        let out = std::process::Command::new("sh")
            .args(["-c", &script()])
            .output()
            .unwrap();
        assert!(out.status.success());
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("---route6") && text.contains("---now"));
        let all = parse(&text);
        assert!(all.iter().all(|i| i.name != "lo"));
        assert!(all.iter().filter(|i| i.default_route).count() <= 1);
    }
}
