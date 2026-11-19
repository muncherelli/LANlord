//! what a gateway can do for lanlord's networks, and how to fix what it cannot.
//!
//! the probe (in core) runs on the gateway and prints `key=value` lines; [`Report::parse`] turns
//! them into an environment and a list of checks, each with the exact command or setting that
//! fixes it. parsing lives here so every front end, and the tests, read the same answer.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// where the gateway runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Environment {
    BareMetal,
    Vm {
        hypervisor: String,
    },
    Container {
        /// the container technology, such as `lxc` (`lxc-libvirt` is a different thing).
        technology: String,
        privilege: Privilege,
        /// proxmox wrote its markers into the container's files.
        proxmox: bool,
    },
    Unknown,
}

/// whether root in a container is root on the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Privilege {
    /// the uid map is the identity over the whole range.
    Privileged,
    /// the container's ids are shifted into a range of unprivileged ids on the host.
    Unprivileged,
    Unknown,
}

/// what the machine calls itself.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct System {
    pub hostname: String,
    pub distro: String,
    pub distro_version: String,
    pub init: String,
}

/// the proxmox fix for docker in an unprivileged lxc container.
pub const PROXMOX_FEATURES_FIX: &str = "on the proxmox host run `pct set <CTID> --features nesting=1,keyctl=1` (replace <CTID> with this container's number) and restart the container with `pct reboot <CTID>`; lanlord cannot do this from inside the container";

/// the fix for nesting in an incus or lxd container (not proxmox); keyctl is allowed by default there.
pub const INCUS_NESTING_FIX: &str = "on the incus or lxd host run `incus config set <name> security.nesting=true` (replace <name> with this container's name; lxd uses `lxc config set`) and restart it with `incus restart <name>`; lanlord cannot do this from inside the container";

/// the fix for keyctl in an incus or lxd container (not proxmox).
pub const INCUS_KEYCTL_FIX: &str = "incus and lxd allow keyctl by default: on the host check `incus config show <name>` for a seccomp or `raw.lxc` setting that blocks it and remove it, then restart the container with `incus restart <name>`; lanlord cannot do this from inside the container";

/// the proxmox fix for a missing /dev/net/tun in a container.
pub const PROXMOX_TUN_FIX: &str = "on the proxmox host run `pct set <CTID> --dev0 /dev/net/tun` (proxmox ve 8.1 or newer; replace <CTID> with this container's number), or on proxmox ve 7 or newer add these two lines to /etc/pve/lxc/<CTID>.conf:\nlxc.cgroup2.devices.allow: c 10:200 rwm\nlxc.mount.entry: /dev/net/tun dev/net/tun none bind,create=file\nand then restart the container with `pct reboot <CTID>`";

/// the modules that run in docker (or install it) and so need what docker needs.
pub const DOCKER_MODULES: &[&str] = &["docker", "nt4-domain", "web-proxy", "dns", "time-server"];

impl Environment {
    /// a short description, such as "unprivileged lxc container on proxmox".
    pub fn describe(&self) -> String {
        match self {
            Self::BareMetal => "bare metal".into(),
            Self::Vm { hypervisor } => format!("virtual machine ({hypervisor})"),
            Self::Container {
                technology,
                privilege,
                proxmox,
            } => {
                let kind = match privilege {
                    Privilege::Privileged => "privileged ",
                    Privilege::Unprivileged => "unprivileged ",
                    Privilege::Unknown => "",
                };
                let on = if *proxmox { " on proxmox" } else { "" };
                format!("{kind}{technology} container{on}")
            }
            Self::Unknown => "unknown".into(),
        }
    }

    fn container(&self) -> bool {
        matches!(self, Self::Container { .. })
    }

    /// is this an lxc container (not lxc-libvirt, which is something else).
    pub fn is_lxc(&self) -> bool {
        matches!(self, Self::Container { technology, .. } if technology == "lxc")
    }

    /// does docker inside this container need the keyctl feature: only when the container's ids
    /// are shifted, or when it can't be told.
    pub fn needs_keyctl(&self) -> bool {
        matches!(
            self,
            Self::Container { technology, privilege, .. }
                if technology == "lxc" && *privilege != Privilege::Privileged
        )
    }

    fn proxmox(&self) -> bool {
        matches!(self, Self::Container { proxmox: true, .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Warn,
    Fail,
    Unknown,
}

impl Status {
    /// a one-character mark for text output.
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Ok => "+",
            Self::Warn => "!",
            Self::Fail => "x",
            Self::Unknown => "?",
        }
    }
}

/// one thing the probe looked at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub id: String,
    pub title: String,
    pub status: Status,
    pub detail: String,
    /// the exact command or setting that fixes it, where there is one.
    pub fix: Option<String>,
}

/// everything the probe found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub environment: Environment,
    pub system: System,
    pub kernel: String,
    pub checks: Vec<Check>,
}

/// the link kinds the probe tries, with the host module that provides each and whether lanlord's
/// networks need it (the others are only for remote attachments and tunnels).
const LINKS: [(&str, &str, &str, bool); 9] = [
    ("bridge", "bridge", "bridges", true),
    ("dummy", "dummy", "dummy links", false),
    ("vlan", "8021q", "802.1q vlans", true),
    ("veth", "veth", "veth pairs", true),
    ("macvlan", "macvlan", "macvlan links", false),
    ("ipvlan", "ipvlan", "ipvlan links", false),
    ("vxlan", "vxlan", "vxlan tunnels", false),
    ("gretap", "ip_gre", "gretap tunnels", false),
    ("wireguard", "wireguard", "wireguard tunnels", false),
];

impl Report {
    /// read the probe's `key=value` lines.
    pub fn parse(text: &str) -> Self {
        let facts: BTreeMap<&str, &str> = text
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(k, v)| (k.trim(), v.trim()))
            .collect();
        let get = |key: &str| facts.get(key).copied().unwrap_or("");
        let environment = environment(&get);
        let mut checks = Vec::new();
        for (kind, module, title, needed) in LINKS {
            checks.push(link_check(
                &environment,
                kind,
                module,
                title,
                needed,
                get(&format!("link_{kind}")),
            ));
        }
        checks.push(tun_check(&environment, get("tun")));
        checks.push(forward_check(get("ip_forward")));
        checks.push(nft_check(get("nft")));
        if environment.is_lxc() {
            checks.push(host_fix(get("pve_marker"), INCUS_NESTING_FIX, feature_check(
                "lxc-nesting",
                "container nesting",
                "containers can run inside this one (docker needs this)",
                "the nesting feature of this lxc container is off, and docker needs it",
                "can't tell whether the nesting feature of this lxc container is on, and docker \
                 needs it",
                get("nesting"),
                true,
            )));
            let needed = environment.needs_keyctl();
            checks.push(host_fix(get("pve_marker"), INCUS_KEYCTL_FIX, feature_check(
                "lxc-keyctl",
                "container keyctl",
                if needed {
                    "the keyctl system call is allowed (docker needs this)"
                } else {
                    "the keyctl system call is allowed (not needed in a privileged container)"
                },
                "the keyctl feature of this lxc container is off, and docker needs it",
                "can't tell whether the keyctl feature of this lxc container is on, and docker \
                 needs it",
                get("keyctl"),
                needed,
            )));
        }
        if let Environment::Container { technology, .. } = &environment {
            if technology == "docker" || technology == "podman" {
                checks.push(Check {
                    id: "nested-container".into(),
                    title: "container manager".into(),
                    status: Status::Warn,
                    detail: format!(
                        "this server is itself a {technology} container; running docker inside it \
                         needs extra privileges"
                    ),
                    fix: Some(
                        "use a virtual machine or an lxc container for the lanlord server".into(),
                    ),
                });
            }
        }
        checks.push(fuse_check(get("fuse")));
        checks.push(docker_check(&get));
        checks.push(manager_check(&environment, &get));
        checks.push(vde_check(get("vde_switch"), get("vde_plug")));
        checks.push(ntp_port_check(get("ntp_port")));
        if environment.proxmox() {
            checks.push(Check {
                id: "proxmox-macfilter".into(),
                title: "proxmox mac filter".into(),
                status: Status::Unknown,
                detail: "can't be seen from inside; with the proxmox firewall on, a network card \
                         that carries remote pcs' traffic drops every frame from another mac"
                    .into(),
                fix: Some(
                    "on the host: macfilter: 0 in the container's firewall options, or firewall=0 \
                     on that network card"
                        .into(),
                ),
            });
        }
        let system = System {
            hostname: get("hostname").to_string(),
            distro: get("distro").to_string(),
            distro_version: get("distro_version").to_string(),
            init: get("init").to_string(),
        };
        Self {
            environment,
            system,
            kernel: get("kernel").to_string(),
            checks,
        }
    }

    /// the worst status of any check.
    pub fn worst(&self) -> Status {
        let rank = |s: Status| match s {
            Status::Ok => 0,
            Status::Unknown => 1,
            Status::Warn => 2,
            Status::Fail => 3,
        };
        self.checks
            .iter()
            .map(|c| c.status)
            .max_by_key(|s| rank(*s))
            .unwrap_or(Status::Ok)
    }
}

fn environment<'a>(get: &dyn Fn(&str) -> &'a str) -> Environment {
    let container = get("virt_container");
    let vm = get("virt_vm");
    let proxmox = get("pve_marker") == "1";
    if !container.is_empty() && container != "none" && container != "unknown" {
        return Environment::Container {
            technology: container.to_string(),
            privilege: privilege(get("uid_map")),
            proxmox,
        };
    }
    if !vm.is_empty() && vm != "none" {
        return Environment::Vm {
            hypervisor: vm.to_string(),
        };
    }
    if container == "none" && vm == "none" {
        return Environment::BareMetal;
    }
    Environment::Unknown
}

/// a privileged container maps every id to itself; any other first line is a shifted range.
fn privilege(uid_map: &str) -> Privilege {
    let fields: Vec<&str> = uid_map.split_whitespace().collect();
    match fields.as_slice() {
        [] => Privilege::Unknown,
        ["0", "0", "4294967295"] => Privilege::Privileged,
        _ => Privilege::Unprivileged,
    }
}

/// how to load a kernel module, depending on whose kernel it is.
fn load_module(env: &Environment, module: &str) -> String {
    if env.container() {
        format!(
            "on the host: modprobe {module}, and add {module} to /etc/modules-load.d/lanlord.conf \
             so it loads at boot (a container can't load modules)"
        )
    } else {
        format!(
            "modprobe {module}, and add {module} to /etc/modules-load.d/lanlord.conf so it loads \
             at boot"
        )
    }
}

fn link_check(
    env: &Environment,
    kind: &str,
    module: &str,
    title: &str,
    needed: bool,
    said: &str,
) -> Check {
    let missing = if needed { Status::Fail } else { Status::Warn };
    let (status, detail, fix) = if said == "ok" {
        (Status::Ok, "can be created".to_string(), None)
    } else if said.is_empty() || said == "skipped" {
        (
            Status::Unknown,
            "not tried (it needs a dummy parent, which could not be made)".to_string(),
            None,
        )
    } else {
        let error = said.strip_prefix("err:").unwrap_or(said);
        let lower = error.to_ascii_lowercase();
        if lower.contains("not supported")
            || lower.contains("unknown device type")
            || lower.contains("no such device")
        {
            (
                missing,
                format!("the {module} kernel module isn't loaded ({error})"),
                Some(load_module(env, module)),
            )
        } else if lower.contains("not permitted") || lower.contains("permission denied") {
            (
                missing,
                format!("not allowed ({error}); the capability or an apparmor profile blocks it"),
                None,
            )
        } else {
            (missing, format!("failed: {error}"), None)
        }
    };
    Check {
        id: format!("link-{kind}"),
        title: title.into(),
        status,
        detail,
        fix,
    }
}

fn tun_check(env: &Environment, said: &str) -> Check {
    let pass_through = PROXMOX_TUN_FIX;
    let (status, detail, fix) = match said {
        "ok" => (Status::Ok, "/dev/net/tun opens".to_string(), None),
        "missing" if env.container() => (
            Status::Warn,
            "/dev/net/tun isn't in the container; remote attachments (vde) need it".to_string(),
            Some(pass_through.to_string()),
        ),
        "missing" => (
            Status::Warn,
            "/dev/net/tun is missing; remote attachments (vde) need it".to_string(),
            Some(load_module(env, "tun")),
        ),
        "" => (Status::Unknown, "not tried".to_string(), None),
        other => {
            let error = other.strip_prefix("err:").unwrap_or(other);
            let fix = if env.container() {
                Some(pass_through.to_string())
            } else {
                None
            };
            (
                Status::Warn,
                format!("/dev/net/tun won't open ({error})"),
                fix,
            )
        }
    };
    Check {
        id: "tun".into(),
        title: "tun/tap device".into(),
        status,
        detail,
        fix,
    }
}

fn forward_check(said: &str) -> Check {
    let (status, detail, fix) = match said {
        "1" => (Status::Ok, "ip forwarding is on".to_string(), None),
        "0" => (
            Status::Warn,
            "ip forwarding is off; lanlord turns it on when it sets up networks".to_string(),
            Some(
                "sysctl -w net.ipv4.ip_forward=1, and net.ipv4.ip_forward = 1 in \
                 /etc/sysctl.d/60-lanlord.conf"
                    .to_string(),
            ),
        ),
        _ => (
            Status::Unknown,
            "couldn't read net.ipv4.ip_forward".to_string(),
            None,
        ),
    };
    Check {
        id: "ip-forward".into(),
        title: "ip forwarding".into(),
        status,
        detail,
        fix,
    }
}

fn nft_check(said: &str) -> Check {
    let (status, detail, fix) = if said == "ok" {
        (Status::Ok, "nftables works".to_string(), None)
    } else if said == "missing" {
        (
            Status::Fail,
            "the nft command isn't installed".to_string(),
            Some("install nftables (pacman -S nftables, or apt install nftables)".to_string()),
        )
    } else if said.is_empty() {
        (Status::Unknown, "not tried".to_string(), None)
    } else {
        let error = said.strip_prefix("err:").unwrap_or(said);
        (
            Status::Fail,
            format!("nft can't list tables ({error})"),
            None,
        )
    };
    Check {
        id: "nftables".into(),
        title: "nftables".into(),
        status,
        detail,
        fix,
    }
}

/// docker's state. whether the machine lets docker run at all is the container checks' job;
/// this one is about what docker does to lanlord's networks once it runs.
fn docker_check<'a>(get: &dyn Fn(&str) -> &'a str) -> Check {
    let backend = get("docker_backend");
    let policy = get("forward_policy");
    let (status, detail, fix) = match (get("docker_installed"), get("docker_running")) {
        ("yes", "yes") if policy == "DROP" => (
            Status::Warn,
            "docker set the iptables FORWARD policy to DROP, which silently drops forwarded \
             traffic that lanlord's own rules accept"
                .to_string(),
            Some(
                "add \"ip-forward-no-drop\": true to /etc/docker/daemon.json, then systemctl \
                 restart docker"
                    .to_string(),
            ),
        ),
        ("yes", "yes") if backend == "nftables" => (
            Status::Warn,
            "docker uses its experimental nftables backend; it doesn't turn ip forwarding on, so \
             lanlord sets it in sysctl.d"
                .to_string(),
            None,
        ),
        ("yes", "yes") => {
            let mut detail = format!("docker {} is running", or_unknown(get("docker_version")));
            if !get("docker_storage").is_empty() {
                detail.push_str(&format!(" (storage driver {}", get("docker_storage")));
                detail.push_str(&format!(
                    ", {} firewall backend)",
                    if backend.is_empty() {
                        "iptables"
                    } else {
                        backend
                    }
                ));
            }
            (Status::Ok, detail, None)
        }
        ("yes", "no") => (
            Status::Warn,
            "docker is installed but not running".to_string(),
            None,
        ),
        ("no", _) => (
            Status::Unknown,
            "docker isn't installed yet; lanlord's docker module installs it".to_string(),
            None,
        ),
        _ => (
            Status::Unknown,
            "couldn't tell about docker".to_string(),
            None,
        ),
    };
    Check {
        id: "docker".into(),
        title: "docker".into(),
        status,
        detail,
        fix,
    }
}

fn or_unknown(s: &str) -> &str {
    if s.is_empty() {
        "unknown"
    } else {
        s
    }
}

/// the fix for the host: the `pct` one on proxmox, the incus or lxd one when the probe saw no
/// proxmox marker, and both when it could not say.
fn host_fix(marker: &str, other: &str, mut check: Check) -> Check {
    if let Some(pct) = check.fix.take() {
        check.fix = Some(match marker {
            "1" => pct,
            "0" => other.to_string(),
            _ => format!("on proxmox: {pct}\non incus or lxd: {other}"),
        });
    }
    check
}

/// a container feature that docker needs, such as nesting or keyctl. only a definite no
/// fails; a probe that couldn't tell is unknown. `needed` is false when this machine doesn't
/// need the feature at all, which is always fine.
fn feature_check(
    id: &str,
    title: &str,
    yes: &str,
    no: &str,
    unknown: &str,
    said: &str,
    needed: bool,
) -> Check {
    let (status, detail, fix) = match said {
        _ if !needed => (Status::Ok, yes.to_string(), None),
        "yes" => (Status::Ok, yes.to_string(), None),
        "no" => (
            Status::Fail,
            no.to_string(),
            Some(PROXMOX_FEATURES_FIX.to_string()),
        ),
        _ => (
            Status::Unknown,
            unknown.to_string(),
            Some(PROXMOX_FEATURES_FIX.to_string()),
        ),
    };
    Check {
        id: id.into(),
        title: title.into(),
        status,
        detail,
        fix,
    }
}

/// /dev/fuse: some storage drivers inside containers use it; nothing in lanlord needs it yet.
fn fuse_check(said: &str) -> Check {
    let (status, detail) = match said {
        "yes" => (Status::Ok, "/dev/fuse is there"),
        "no" => (
            Status::Unknown,
            "/dev/fuse is missing; nothing needs it yet",
        ),
        _ => (Status::Unknown, "not tried"),
    };
    Check {
        id: "fuse".into(),
        title: "fuse".into(),
        status,
        detail: detail.into(),
        fix: None,
    }
}

fn manager_check<'a>(env: &Environment, get: &dyn Fn(&str) -> &'a str) -> Check {
    let manager = get("netmgr");
    let pve_interfaces = get("pve_interfaces") == "1";
    let ignored = get("pve_ignore") == "1";
    let (status, detail, fix) = match manager {
        "networkd" => (
            Status::Ok,
            "systemd-networkd manages the links".to_string(),
            None,
        ),
        "networkmanager" => (
            Status::Ok,
            "networkmanager manages the links".to_string(),
            None,
        ),
        "ifupdown" if env.proxmox() && pve_interfaces && !ignored => (
            Status::Warn,
            "ifupdown manages the links, and proxmox rewrites /etc/network/interfaces at every \
             start"
                .to_string(),
            Some("touch /etc/network/.pve-ignore.interfaces".to_string()),
        ),
        "ifupdown" => (Status::Ok, "ifupdown manages the links".to_string(), None),
        _ => (
            Status::Warn,
            "no known network manager is active; lanlord would create the links itself at boot"
                .to_string(),
            None,
        ),
    };
    Check {
        id: "network-manager".into(),
        title: "network manager".into(),
        status,
        detail,
        fix,
    }
}

fn vde_check(switch: &str, plug: &str) -> Check {
    let (status, detail, fix) = match (switch, plug) {
        ("", "") | ("missing", "missing") => (
            Status::Warn,
            "vde isn't installed; only remote attachments need it".to_string(),
            Some("install vde2 (pacman -S vde2, or apt install vde2)".to_string()),
        ),
        (s, p) if s != "missing" && p != "missing" && !s.is_empty() && !p.is_empty() => {
            (Status::Ok, format!("vde_switch {s}, vde_plug {p}"), None)
        }
        (s, p) => (
            Status::Warn,
            format!("vde is partly installed (vde_switch: {s}, vde_plug: {p})"),
            Some("install vde2 (pacman -S vde2, or apt install vde2)".to_string()),
        ),
    };
    Check {
        id: "vde".into(),
        title: "vde (remote attachments)".into(),
        status,
        detail,
        fix,
    }
}

/// udp 123 on the host: the time server binds it, so a host chronyd or ntpd that already listens
/// on every address stops it. systemd-timesyncd only asks and is never reported.
fn ntp_port_check(said: &str) -> Check {
    let (status, detail, fix) = match said {
        "free" => (Status::Ok, "udp 123 is free".to_string(), None),
        busy if busy.starts_with("busy:") => {
            let who = busy.trim_start_matches("busy:");
            let who = if who.is_empty() || who == "unknown" {
                "another program".to_string()
            } else {
                who.to_string()
            };
            (
                Status::Fail,
                format!("udp 123 is already used by {who} on this server, so the time server cannot start"),
                Some(
                    "stop it (systemctl disable --now chrony, chronyd or ntpd), or leave the time server off; systemd-timesyncd does not get in the way"
                        .to_string(),
                ),
            )
        }
        _ => (Status::Unknown, "not tried".to_string(), None),
    };
    Check {
        id: "ntp-port".into(),
        title: "ntp port (udp 123)".into(),
        status,
        detail,
        fix,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNPRIVILEGED_PROXMOX: &str = "kernel=6.8.12-4-pve
hostname=gateway
init=systemd
distro=debian
distro_version=13
virt_container=lxc
virt_vm=none
uid_map=0 100000 65536
pve_marker=1
nesting=no
keyctl=no
link_bridge=ok
link_dummy=ok
link_vlan=err:RTNETLINK answers: Operation not supported
link_veth=ok
link_macvlan=ok
link_ipvlan=ok
link_vxlan=err:Error: Unknown device type.
link_gretap=err:Error: Unknown device type.
link_wireguard=ok
tun=missing
fuse=no
ip_forward=0
nft=ok
docker_installed=no
docker_running=no
netmgr=networkd
pve_interfaces=0
pve_ignore=0
vde_switch=missing
vde_plug=missing
";

    const PRIVILEGED: &str = "kernel=6.8.12-4-pve
virt_container=lxc
virt_vm=none
uid_map=0 0 4294967295
pve_marker=1
nesting=yes
keyctl=no
link_bridge=ok
link_dummy=ok
link_vlan=ok
link_veth=ok
link_macvlan=ok
link_ipvlan=ok
link_vxlan=ok
link_gretap=ok
link_wireguard=ok
tun=err:Operation not permitted
fuse=yes
ip_forward=1
nft=ok
docker_installed=yes
docker_running=yes
docker_version=29.1.2
docker_storage=overlayfs
docker_backend=iptables
forward_policy=DROP
netmgr=ifupdown
pve_interfaces=1
pve_ignore=0
vde_switch=2.3.3
vde_plug=4.0.1
";

    const VM: &str = "kernel=6.1.0-27-amd64
virt_container=none
virt_vm=kvm
uid_map=0 0 4294967295
pve_marker=0
nesting=yes
keyctl=yes
link_bridge=ok
link_dummy=ok
link_vlan=ok
link_veth=ok
link_macvlan=ok
link_ipvlan=ok
link_vxlan=ok
link_gretap=ok
link_wireguard=ok
tun=ok
fuse=yes
ip_forward=1
nft=ok
docker_installed=yes
docker_running=yes
docker_version=29.1.2
docker_storage=overlayfs
docker_backend=nftables
netmgr=networkmanager
vde_switch=2.3.3
vde_plug=missing
";

    const BARE_METAL: &str = "kernel=6.12.1-arch1-1
virt_container=none
virt_vm=none
uid_map=0 0 4294967295
pve_marker=0
nesting=yes
keyctl=yes
link_bridge=ok
link_dummy=err:RTNETLINK answers: Operation not permitted
link_vlan=skipped
link_veth=ok
link_macvlan=skipped
link_ipvlan=skipped
link_vxlan=ok
link_gretap=ok
link_wireguard=ok
tun=ok
fuse=yes
ip_forward=1
nft=missing
docker_installed=yes
docker_running=no
netmgr=none
vde_switch=missing
vde_plug=missing
";

    fn check<'a>(report: &'a Report, id: &str) -> &'a Check {
        report
            .checks
            .iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("no check {id}"))
    }

    fn has(report: &Report, id: &str) -> bool {
        report.checks.iter().any(|c| c.id == id)
    }

    /// a copy of the unprivileged sample with some keys changed.
    fn with(replace: &[(&str, &str)]) -> Report {
        let mut lines: Vec<String> = UNPRIVILEGED_PROXMOX.lines().map(str::to_string).collect();
        for (key, value) in replace {
            let prefix = format!("{key}=");
            lines.retain(|l| !l.starts_with(&prefix));
            lines.push(format!("{key}={value}"));
        }
        Report::parse(&lines.join("\n"))
    }

    #[test]
    fn an_unprivileged_proxmox_container_is_told_what_to_change_on_the_host() {
        let report = Report::parse(UNPRIVILEGED_PROXMOX);
        assert_eq!(
            report.environment,
            Environment::Container {
                technology: "lxc".into(),
                privilege: Privilege::Unprivileged,
                proxmox: true
            }
        );
        assert_eq!(
            report.environment.describe(),
            "unprivileged lxc container on proxmox"
        );
        assert_eq!(report.kernel, "6.8.12-4-pve");
        assert_eq!(
            report.system,
            System {
                hostname: "gateway".into(),
                distro: "debian".into(),
                distro_version: "13".into(),
                init: "systemd".into()
            }
        );
        let vlan = check(&report, "link-vlan");
        assert_eq!(vlan.status, Status::Fail);
        assert!(vlan
            .fix
            .as_deref()
            .unwrap()
            .starts_with("on the host: modprobe 8021q"));
        let vxlan = check(&report, "link-vxlan");
        assert_eq!(vxlan.status, Status::Warn, "tunnels are optional");
        assert!(vxlan.fix.as_deref().unwrap().contains("modprobe vxlan"));
        assert!(check(&report, "link-gretap")
            .fix
            .as_deref()
            .unwrap()
            .contains("ip_gre"));
        assert_eq!(check(&report, "link-bridge").status, Status::Ok);
        let tun = check(&report, "tun");
        assert_eq!(tun.fix.as_deref(), Some(PROXMOX_TUN_FIX));
        assert!(PROXMOX_TUN_FIX.contains("pct set <CTID> --dev0 /dev/net/tun"));
        assert!(PROXMOX_TUN_FIX.contains("lxc.cgroup2.devices.allow: c 10:200 rwm"));
        assert!(PROXMOX_TUN_FIX
            .contains("lxc.mount.entry: /dev/net/tun dev/net/tun none bind,create=file"));
        assert_eq!(check(&report, "ip-forward").status, Status::Warn);
        assert_eq!(check(&report, "docker").status, Status::Unknown);
        for id in ["lxc-nesting", "lxc-keyctl"] {
            let c = check(&report, id);
            assert_eq!(c.status, Status::Fail, "{id}");
            assert_eq!(c.fix.as_deref(), Some(PROXMOX_FEATURES_FIX), "{id}");
        }
        assert_eq!(check(&report, "network-manager").status, Status::Ok);
        assert_eq!(check(&report, "vde").status, Status::Warn);
        assert_eq!(check(&report, "fuse").status, Status::Unknown);
        assert!(check(&report, "proxmox-macfilter")
            .fix
            .as_deref()
            .unwrap()
            .contains("macfilter: 0"));
        assert_eq!(report.worst(), Status::Fail);
    }

    #[test]
    fn a_privileged_container_needs_no_keyctl_and_gets_the_daemon_fix_for_dockers_drop_policy() {
        let report = Report::parse(PRIVILEGED);
        assert!(matches!(
            report.environment,
            Environment::Container {
                privilege: Privilege::Privileged,
                proxmox: true,
                ..
            }
        ));
        let keyctl = check(&report, "lxc-keyctl");
        assert_eq!(
            keyctl.status,
            Status::Ok,
            "a privileged container needs none"
        );
        assert!(keyctl.detail.contains("not needed"));
        assert_eq!(check(&report, "lxc-nesting").status, Status::Ok);
        let docker = check(&report, "docker");
        assert_eq!(docker.status, Status::Warn);
        assert!(docker
            .fix
            .as_deref()
            .unwrap()
            .contains("\"ip-forward-no-drop\": true"));
        let tun = check(&report, "tun");
        assert_eq!(tun.status, Status::Warn);
        assert!(tun.fix.as_deref().unwrap().contains("c 10:200 rwm"));
        let manager = check(&report, "network-manager");
        assert_eq!(manager.status, Status::Warn);
        assert_eq!(
            manager.fix.as_deref(),
            Some("touch /etc/network/.pve-ignore.interfaces")
        );
        assert_eq!(check(&report, "fuse").status, Status::Ok);
        assert_eq!(
            check(&report, "vde").detail,
            "vde_switch 2.3.3, vde_plug 4.0.1"
        );
    }

    #[test]
    fn only_a_definite_no_fails_and_a_probe_that_could_not_tell_is_unknown() {
        let report = with(&[("nesting", "unknown"), ("keyctl", "unknown")]);
        for id in ["lxc-nesting", "lxc-keyctl"] {
            let c = check(&report, id);
            assert_eq!(c.status, Status::Unknown, "{id}");
            assert!(c.detail.starts_with("can't tell"), "{id}: {}", c.detail);
            assert!(c.fix.is_some());
        }
        let report = with(&[("nesting", "yes"), ("keyctl", "yes")]);
        assert_eq!(check(&report, "lxc-nesting").status, Status::Ok);
        assert_eq!(check(&report, "lxc-keyctl").status, Status::Ok);
        // a missing key is not a no either
        let report = Report::parse("virt_container=lxc\nvirt_vm=none\nuid_map=0 100000 65536\n");
        assert_eq!(check(&report, "lxc-nesting").status, Status::Unknown);
    }

    #[test]
    fn a_host_listening_on_udp_123_fails_the_time_server_check() {
        let free = with(&[("ntp_port", "free")]);
        assert_eq!(check(&free, "ntp-port").status, Status::Ok);
        let busy = with(&[("ntp_port", "busy:chronyd")]);
        let c = check(&busy, "ntp-port");
        assert_eq!(c.status, Status::Fail);
        assert!(c.detail.contains("already used by chronyd"), "{}", c.detail);
        assert!(c.fix.as_deref().unwrap().contains("timesyncd"));
        // a listener the probe could not name is still a listener
        let nameless = with(&[("ntp_port", "busy:unknown")]);
        assert!(check(&nameless, "ntp-port")
            .detail
            .contains("another program"));
        let empty = with(&[("ntp_port", "busy:")]);
        assert_eq!(check(&empty, "ntp-port").status, Status::Fail);
        // a probe that could not look is not a no
        for said in ["unknown", ""] {
            let report = with(&[("ntp_port", said)]);
            assert_eq!(check(&report, "ntp-port").status, Status::Unknown);
        }
    }

    #[test]
    fn the_feature_fix_names_incus_unless_the_container_is_on_proxmox() {
        let report = Report::parse(
            "virt_container=lxc\nvirt_vm=none\nuid_map=0 100000 65536\npve_marker=0\nnesting=no\nkeyctl=unknown\n",
        );
        let nesting = check(&report, "lxc-nesting");
        assert_eq!(nesting.fix.as_deref(), Some(INCUS_NESTING_FIX));
        assert!(INCUS_NESTING_FIX.contains("security.nesting=true"));
        assert_eq!(
            check(&report, "lxc-keyctl").fix.as_deref(),
            Some(INCUS_KEYCTL_FIX)
        );
        let report = Report::parse(
            "virt_container=lxc\nvirt_vm=none\nuid_map=0 100000 65536\npve_marker=1\nnesting=no\n",
        );
        assert_eq!(
            check(&report, "lxc-nesting").fix.as_deref(),
            Some(PROXMOX_FEATURES_FIX)
        );
        // no marker at all: both are listed
        let report =
            Report::parse("virt_container=lxc\nvirt_vm=none\nuid_map=0 100000 65536\nnesting=no\n");
        let fix = check(&report, "lxc-nesting").fix.clone().unwrap();
        assert!(
            fix.contains("pct set") && fix.contains("security.nesting=true"),
            "{fix}"
        );
    }

    #[test]
    fn keyctl_is_needed_unless_the_container_is_known_to_be_privileged() {
        // an unreadable uid map can't prove the container is privileged
        let report = with(&[("uid_map", ""), ("keyctl", "no")]);
        assert_eq!(
            report.environment,
            Environment::Container {
                technology: "lxc".into(),
                privilege: Privilege::Unknown,
                proxmox: true
            }
        );
        assert!(report.environment.needs_keyctl());
        assert_eq!(check(&report, "lxc-keyctl").status, Status::Fail);
        assert_eq!(report.environment.describe(), "lxc container on proxmox");
        let report = with(&[("uid_map", "0 0 4294967295"), ("keyctl", "no")]);
        assert!(!report.environment.needs_keyctl());
        assert_eq!(check(&report, "lxc-keyctl").status, Status::Ok);
    }

    #[test]
    fn lxc_libvirt_and_other_managers_get_no_proxmox_container_checks() {
        let report = with(&[("virt_container", "lxc-libvirt")]);
        assert!(!report.environment.is_lxc());
        assert!(!has(&report, "lxc-nesting") && !has(&report, "lxc-keyctl"));
        assert_eq!(
            report.environment.describe(),
            "unprivileged lxc-libvirt container on proxmox"
        );
        let report = with(&[("virt_container", "docker")]);
        let nested = check(&report, "nested-container");
        assert_eq!(nested.status, Status::Warn);
        assert!(nested.detail.contains("docker container"));
        assert!(!has(&report, "lxc-nesting"));
        let report = with(&[("virt_container", "podman")]);
        assert!(has(&report, "nested-container"));
        let report = with(&[("virt_container", "systemd-nspawn")]);
        assert!(!has(&report, "nested-container") && !has(&report, "lxc-nesting"));
    }

    #[test]
    fn a_vm_loads_its_own_modules_and_has_no_container_checks() {
        let report = Report::parse(VM);
        assert_eq!(
            report.environment,
            Environment::Vm {
                hypervisor: "kvm".into()
            }
        );
        assert!(!has(&report, "lxc-nesting") && !has(&report, "proxmox-macfilter"));
        let docker = check(&report, "docker");
        assert_eq!(docker.status, Status::Warn, "nftables backend");
        assert_eq!(check(&report, "tun").status, Status::Ok);
        let vde = check(&report, "vde");
        assert_eq!(vde.status, Status::Warn);
        assert!(vde.detail.contains("partly"));
    }

    #[test]
    fn bare_metal_reports_blocked_and_untried_links() {
        let report = Report::parse(BARE_METAL);
        assert_eq!(report.environment, Environment::BareMetal);
        assert_eq!(report.environment.describe(), "bare metal");
        let dummy = check(&report, "link-dummy");
        assert_eq!(dummy.status, Status::Warn);
        assert!(dummy.detail.contains("not allowed"));
        assert_eq!(dummy.fix, None);
        assert_eq!(check(&report, "link-vlan").status, Status::Unknown);
        let nft = check(&report, "nftables");
        assert_eq!(nft.status, Status::Fail);
        assert!(nft.fix.as_deref().unwrap().contains("nftables"));
        let docker = check(&report, "docker");
        assert_eq!(docker.status, Status::Warn);
        assert_eq!(docker.detail, "docker is installed but not running");
        assert_eq!(check(&report, "network-manager").status, Status::Warn);
    }

    #[test]
    fn a_running_docker_says_which_version_and_backend() {
        let report = Report::parse(&UNPRIVILEGED_PROXMOX.replace(
            "docker_installed=no\ndocker_running=no",
            "docker_installed=yes\ndocker_running=yes\ndocker_version=29.1.2\ndocker_storage=overlayfs",
        ));
        let docker = check(&report, "docker");
        assert_eq!(docker.status, Status::Ok);
        assert_eq!(
            docker.detail,
            "docker 29.1.2 is running (storage driver overlayfs, iptables firewall backend)"
        );
        let report = with(&[("docker_installed", "")]);
        assert_eq!(check(&report, "docker").status, Status::Unknown);
    }

    #[test]
    fn module_fixes_depend_on_whose_kernel_it_is() {
        let bare = load_module(&Environment::BareMetal, "8021q");
        assert!(bare.starts_with("modprobe 8021q"));
        let container = Environment::Container {
            technology: "lxc".into(),
            privilege: Privilege::Unprivileged,
            proxmox: false,
        };
        assert!(load_module(&container, "8021q").starts_with("on the host:"));
        assert_eq!(
            Environment::Vm {
                hypervisor: "kvm".into()
            }
            .describe(),
            "virtual machine (kvm)"
        );
        assert_eq!(Environment::Unknown.describe(), "unknown");
    }

    #[test]
    fn odd_or_missing_answers_are_unknown_not_ok() {
        let report = Report::parse("garbage\n");
        assert_eq!(report.environment, Environment::Unknown);
        assert_eq!(check(&report, "link-bridge").status, Status::Unknown);
        assert_eq!(check(&report, "ip-forward").status, Status::Unknown);
        assert_eq!(check(&report, "nftables").status, Status::Unknown);
        assert_eq!(check(&report, "tun").status, Status::Unknown);
        let failed = link_check(
            &Environment::BareMetal,
            "bridge",
            "bridge",
            "bridges",
            true,
            "err:File exists",
        );
        assert_eq!(failed.status, Status::Fail);
        assert!(failed.detail.contains("File exists"));
        let nft = nft_check("err:Operation not permitted");
        assert_eq!(nft.status, Status::Fail);
        assert!(Status::Unknown.glyph() != Status::Ok.glyph());
        // an unknown container manager is not a container
        let report = Report::parse("virt_container=unknown\nvirt_vm=none\n");
        assert_eq!(report.environment, Environment::Unknown);
    }

    #[test]
    fn a_report_survives_json() {
        let report = Report::parse(UNPRIVILEGED_PROXMOX);
        let json = serde_json::to_string(&report).unwrap();
        let back: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(back, report);
    }
}
