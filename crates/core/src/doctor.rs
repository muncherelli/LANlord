//! `lanlord doctor`: what the gateway can do for lanlord's networks.
//!
//! the probe changes nothing that lasts. to see which kinds of link the kernel allows, it
//! creates throwaway links with unique names in the gateway's own network namespace and deletes
//! each one at once; a trap removes any that are left if the probe stops early.

use anyhow::Result;

use crate::bootstrap::{capture_as_root, AskPassword};
use crate::config;
use crate::ssh::Session;

pub use lanlord_model::doctor::*;

/// the probe; it prints `key=value` lines that [`Report::parse`] reads.
pub const PROBE: &str = r#"
n="lld$$"
cleanup() {
  for s in b d v e p m i x g w; do ip link del "$n$s" >/dev/null 2>&1; done
}
trap cleanup EXIT INT TERM HUP
say() { printf '%s=%s\n' "$1" "$2"; }
first_line() { tr '\n' ' ' | sed 's/  */ /g; s/ $//' | cut -c1-200; }

say kernel "$(uname -r)"

# where we run
if command -v systemd-detect-virt >/dev/null 2>&1; then
  c=$(systemd-detect-virt -c 2>/dev/null); say virt_container "${c:-none}"
  v=$(systemd-detect-virt -v 2>/dev/null); say virt_vm "${v:-none}"
elif [ -r /run/systemd/container ]; then
  say virt_container "$(cat /run/systemd/container)"
fi
say uid_map "$(head -n 1 /proc/self/uid_map 2>/dev/null | sed 's/  */ /g; s/^ //')"
if grep -qs -- '--- BEGIN PVE ---' /etc/hosts /etc/hostname /etc/resolv.conf /etc/network/interfaces; then
  say pve_marker 1
else
  say pve_marker 0
fi

# which kinds of link the kernel allows; each one is deleted straight away
try() {
  kind=$1; name=$2; shift 2
  if ! command -v ip >/dev/null 2>&1; then say "link_$kind" "err:the ip command is missing"; return; fi
  if out=$(ip link add "$name" "$@" 2>&1); then
    say "link_$kind" ok
    ip link del "$name" >/dev/null 2>&1
  else
    say "link_$kind" "err:$(printf '%s' "$out" | first_line)"
  fi
}
try bridge "${n}b" type bridge
if ip link add "${n}d" type dummy >/dev/null 2>&1; then
  say link_dummy ok
  try vlan "${n}v" link "${n}d" type vlan id 99
  try macvlan "${n}m" link "${n}d" type macvlan mode bridge
  try ipvlan "${n}i" link "${n}d" type ipvlan mode l2
  ip link del "${n}d" >/dev/null 2>&1
else
  say link_dummy "err:$(ip link add "${n}d" type dummy 2>&1 | first_line)"
  say link_vlan skipped
  say link_macvlan skipped
  say link_ipvlan skipped
fi
try veth "${n}e" type veth peer name "${n}p"
try vxlan "${n}x" type vxlan id 4242 dstport 4789
try gretap "${n}g" type gretap local 192.0.2.1 remote 192.0.2.2
try wireguard "${n}w" type wireguard

# the tun/tap device remote attachments need
if [ ! -e /dev/net/tun ]; then
  say tun missing
elif out=$( (exec 3<>/dev/net/tun) 2>&1 ); then
  say tun ok
else
  say tun "err:$(printf '%s' "$out" | first_line)"
fi

say ip_forward "$(cat /proc/sys/net/ipv4/ip_forward 2>/dev/null)"

if ! command -v nft >/dev/null 2>&1; then
  say nft missing
elif out=$(nft list tables 2>&1); then
  say nft ok
else
  say nft "err:$(printf '%s' "$out" | first_line)"
fi

# docker, its firewall backend and the FORWARD policy it may have set
if command -v docker >/dev/null 2>&1; then
  say docker present
  a=$(systemctl is-active docker 2>/dev/null); say docker_active "${a:-unknown}"
  backend=$(docker info 2>/dev/null | sed -n 's/^ *Firewall Backend: *//p' | head -n 1)
  say docker_backend "$backend"
else
  say docker missing
fi
if command -v iptables >/dev/null 2>&1; then
  say forward_policy "$(iptables -S FORWARD 2>/dev/null | sed -n 's/^-P FORWARD //p' | head -n 1)"
fi

# proxmox makes these when a container has nesting
if [ -d /dev/.lxc/proc ] && [ -d /dev/.lxc/sys ]; then say lxc_nesting 1; else say lxc_nesting 0; fi

# who manages the links
if systemctl is-active --quiet systemd-networkd 2>/dev/null; then say netmgr networkd
elif systemctl is-active --quiet NetworkManager 2>/dev/null; then say netmgr networkmanager
elif [ -f /etc/network/interfaces ] && systemctl is-active --quiet networking 2>/dev/null; then say netmgr ifupdown
else say netmgr none
fi
if grep -qs -- '--- BEGIN PVE ---' /etc/network/interfaces; then say pve_interfaces 1; else say pve_interfaces 0; fi
if [ -e /etc/network/.pve-ignore.interfaces ]; then say pve_ignore 1; else say pve_ignore 0; fi

# vde, for remote attachments
version() { "$@" 2>&1 | grep -oE '[0-9]+\.[0-9]+(\.[0-9]+)?' | head -n 1; }
if command -v vde_switch >/dev/null 2>&1; then say vde_switch "$(version vde_switch --version)"; else say vde_switch missing; fi
if command -v vde_plug >/dev/null 2>&1; then say vde_plug "$(version vde_plug --version)"; else say vde_plug missing; fi
"#;

/// probe `host`; sudo's password is asked for when needed.
pub fn run(host: &str, ask_password: AskPassword) -> Result<Report> {
    run_with(&Session::new(host)?, ask_password)
}

pub(crate) fn run_with(session: &Session, ask_password: AskPassword) -> Result<Report> {
    let admin = config::admin(session)?;
    let text = capture_as_root(session, &admin, PROBE.to_string(), ask_password)?;
    Ok(Report::parse(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_cleans_up_and_names_fit_the_interface_limit() {
        assert!(PROBE.contains("trap cleanup EXIT"));
        // "lld" + a pid of up to 7 digits + one letter stays within linux's 15 characters
        let longest = format!("lld{}w", 9_999_999);
        assert!(longest.len() <= 15, "{longest}");
        for kind in [
            "bridge",
            "vlan",
            "veth",
            "macvlan",
            "ipvlan",
            "vxlan",
            "gretap",
            "wireguard",
        ] {
            assert!(PROBE.contains(&format!("try {kind} ")), "{kind}");
        }
        // throwaway tunnels use documentation addresses, never real ones
        assert!(PROBE.contains("192.0.2.1"));
    }

    #[cfg(unix)]
    #[test]
    fn the_probe_runs_as_root_and_its_answer_is_parsed() {
        use crate::testgw::no_password;
        let session = Session::fake(
            r#"for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sudo -n true") exit 0 ;;
  *) cat > /dev/null; printf 'virt_container=lxc\nvirt_vm=none\nuid_map=0 100000 65536\npve_marker=1\nlink_bridge=ok\n' ;;
esac"#,
        );
        let report = run_with(&session, &mut no_password).unwrap();
        assert!(matches!(
            report.environment,
            Environment::Container {
                privileged: false,
                proxmox: true,
                ..
            }
        ));
        assert!(report
            .checks
            .iter()
            .any(|c| c.id == "link-bridge" && c.status == Status::Ok));
    }

    #[cfg(unix)]
    #[test]
    fn the_probe_script_is_valid_shell() {
        let status = std::process::Command::new("sh")
            .args(["-n", "-c", PROBE])
            .status()
            .unwrap();
        assert!(status.success());
    }
}
