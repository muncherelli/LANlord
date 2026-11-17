#!/bin/sh
# lanlord's environment probe, for `lanlord doctor`, the apply gate and docker.yml.
#
# it prints key=value lines and changes nothing that lasts: to see which kinds of link the kernel
# allows it creates throwaway links with unique names and deletes each one at once, and a trap
# removes any that are left if it stops early. every check tolerates a missing tool and says
# unknown instead of failing, and lanlord only blocks anything on a definite no.
#
# lanlord feeds this script to `sh -s` on stdin, so nothing in it may read stdin: the whole body
# is one function, which the shell has parsed completely before it runs with stdin on /dev/null.

lanlord_doctor() {
  n="lld$$"
  cleanup() {
    for s in b d v e p m i x g w; do ip link del "$n$s" >/dev/null 2>&1; done
  }
  trap cleanup EXIT INT TERM HUP
  say() { key="$1"; value="$2"; printf '%s=%s\n' "$key" "$value"; }
  have() { command -v "$1" >/dev/null 2>&1; }
  first_line() { tr '\n' ' ' | sed 's/  */ /g; s/ $//' | cut -c1-200; }
  osr() { field="$1"; if [ -r /etc/os-release ]; then sed -n "s/^${field}=//p" /etc/os-release | head -n 1 | tr -d '"'; fi; }

  say kernel "$(uname -r 2>/dev/null)"
  say hostname "$(hostname 2>/dev/null)"
  say init "$(cat /proc/1/comm 2>/dev/null)"
  say distro "$(osr ID)"
  say distro_version "$(osr VERSION_ID)"

  # where we run
  vm=""
  container=""
  if have systemd-detect-virt; then
    c=$(systemd-detect-virt --container 2>/dev/null)
    container=${c:-none}
    v=$(systemd-detect-virt --vm 2>/dev/null)
    vm=${v:-none}
  fi
  if [ -z "$container" ]; then
    if tr '\0' '\n' < /proc/1/environ 2>/dev/null | grep -q '^container='; then
      container=$(tr '\0' '\n' < /proc/1/environ | sed -n 's/^container=//p' | head -n 1)
    elif grep -q lxc /proc/self/cgroup 2>/dev/null; then
      container=lxc
    elif [ -f /.dockerenv ]; then
      container=docker
    fi
  fi
  say virt_container "$container"
  say virt_vm "$vm"
  say uid_map "$(sed -n 1p /proc/self/uid_map 2>/dev/null | tr -s ' \t' ' ' | sed 's/^ //')"
  if grep -qs -- '--- BEGIN PVE ---' /etc/hosts /etc/hostname /etc/resolv.conf /etc/network/interfaces; then
    say pve_marker 1
  else
    say pve_marker 0
  fi

  # can containers run inside this one (the proxmox feature nesting)? a sysctl that is 0 only
  # says user namespaces are off, which is not the same as this container lacking the feature
  nesting=unknown
  if [ "$(cat /proc/sys/user/max_user_namespaces 2>/dev/null)" = 0 ]; then
    nesting=unknown
  elif have unshare; then
    err=$(unshare --user --map-root-user --mount --pid --fork --mount-proc true 2>&1)
    if [ $? -eq 0 ]; then
      nesting=yes
    else
      case "$err" in
        *"not permitted"*|*"ermission denied"*) nesting=no ;;
      esac
    fi
  fi
  say nesting "$nesting"

  # is the keyctl system call allowed (the proxmox feature keyctl)? only ENOSYS, which the
  # container's seccomp profile answers when the feature is off, is a definite no; a refusal
  # for permission could have other causes
  keyctl=unknown
  if have keyctl; then
    err=$(keyctl describe @u 2>&1)
    if [ $? -eq 0 ]; then
      keyctl=yes
    else
      case "$err" in
        *"not implemented"*) keyctl=no ;;
        *"not available"*|*"No such"*) keyctl=yes ;;
      esac
    fi
  elif have python3; then
    r=$(python3 -c '
import ctypes, errno, platform
nr = {"x86_64": 250, "aarch64": 219, "armv7l": 311, "i686": 288, "riscv64": 219}.get(platform.machine())
if nr is None:
    print("unknown")
else:
    libc = ctypes.CDLL(None, use_errno=True)
    ret = libc.syscall(nr, 0, -3, 0, 0, 0)
    e = ctypes.get_errno()
    if ret >= 0 or e == errno.ENOKEY:
        print("yes")
    elif e == errno.ENOSYS:
        print("no")
    else:
        print("unknown")
' 2>/dev/null)
    if [ -n "$r" ]; then keyctl=$r; fi
  fi
  say keyctl "$keyctl"

  # which kinds of link the kernel allows; each one is deleted straight away
  try() {
    kind=$1; name=$2; shift 2
    if ! have ip; then say "link_$kind" "err:the ip command is missing"; return; fi
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

  # the tun/tap device remote attachments need, and fuse
  if [ ! -c /dev/net/tun ]; then
    say tun missing
  elif out=$( (exec 3<>/dev/net/tun) 2>&1 ); then
    say tun ok
  else
    say tun "err:$(printf '%s' "$out" | first_line)"
  fi
  if [ -c /dev/fuse ]; then say fuse yes; else say fuse no; fi

  say ip_forward "$(cat /proc/sys/net/ipv4/ip_forward 2>/dev/null)"

  if ! have nft; then
    say nft missing
  elif out=$(nft list tables 2>&1); then
    say nft ok
  else
    say nft "err:$(printf '%s' "$out" | first_line)"
  fi

  # docker, its firewall backend and the FORWARD policy it may have set
  if have docker; then
    say docker_installed yes
    t=""
    if have timeout; then t="timeout 15"; fi
    if $t docker info >/dev/null 2>&1; then
      say docker_running yes
      say docker_version "$($t docker version --format '{{.Server.Version}}' 2>/dev/null)"
      say docker_storage "$($t docker info --format '{{.Driver}}' 2>/dev/null)"
      say docker_backend "$($t docker info 2>/dev/null | sed -n 's/^ *Firewall Backend: *//p' | head -n 1)"
    else
      say docker_running no
    fi
  else
    say docker_installed no
    say docker_running no
  fi
  if have iptables; then
    say forward_policy "$(iptables -S FORWARD 2>/dev/null | sed -n 's/^-P FORWARD //p' | head -n 1)"
  fi

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
  if have vde_switch; then say vde_switch "$(version vde_switch --version)"; else say vde_switch missing; fi
  if have vde_plug; then say vde_plug "$(version vde_plug --version)"; else say vde_plug missing; fi
}

lanlord_doctor </dev/null
