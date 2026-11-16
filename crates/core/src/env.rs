//! what kind of machine a server is and which features it has, so that lanlord can say what is
//! missing before it enables something that needs it. docker in an unprivileged lxc container is
//! the main case: it needs the proxmox features nesting and keyctl.
//!
//! the probe is one read-only script run as root that prints `key=value` lines. every check
//! tolerates missing tools and says `unknown` instead of failing, and lanlord only ever blocks
//! on a definite `no`.

use std::collections::HashMap;
use std::fmt::Write as _;

use anyhow::{bail, Result};

use crate::bootstrap::{capture_as_root, AskPassword};
use crate::log::{emit, Kind};
use crate::ssh::Session;

/// the modules that run in docker (or install it) and so need what docker needs.
pub const DOCKER_MODULES: &[&str] = &["docker", "nt4-domain", "web-proxy", "dns"];

/// the proxmox fix for docker in an unprivileged lxc container, also written in docker.yml.
pub const PROXMOX_FEATURES_FIX: &str = "on the proxmox host run `pct set <CTID> --features nesting=1,keyctl=1` (replace <CTID> with this container's number) and restart the container with `pct reboot <CTID>`; lanlord cannot do this from inside the container";

/// the proxmox fix for a missing /dev/net/tun in a container.
pub const PROXMOX_TUN_FIX: &str = "on the proxmox host add these lines to /etc/pve/lxc/<CTID>.conf and restart the container: `lxc.cgroup2.devices.allow: c 10:200 rwm` and `lxc.mount.entry: /dev/net/tun dev/net/tun none bind,create=file`";

/// yes, no, or not known (a tool was missing or the answer was unclear).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tri {
    Yes,
    No,
    #[default]
    Unknown,
}

impl Tri {
    fn parse(value: Option<&&str>) -> Self {
        match value.map(|v| v.trim()) {
            Some("yes") => Tri::Yes,
            Some("no") => Tri::No,
            _ => Tri::Unknown,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Container {
    /// not in a container (a virtual machine or bare metal).
    None,
    Lxc,
    Docker,
    Podman,
    Other(String),
    #[default]
    Unknown,
}

impl Container {
    fn parse(value: Option<&&str>) -> Self {
        match value.map(|v| v.trim()) {
            None | Some("") | Some("unknown") => Container::Unknown,
            Some("none") => Container::None,
            Some("lxc") | Some("lxc-libvirt") => Container::Lxc,
            Some("docker") => Container::Docker,
            Some("podman") => Container::Podman,
            Some(other) => Container::Other(other.to_string()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Privilege {
    /// root in the container is root on the host (the uid map is the identity).
    Privileged,
    /// the container's ids are shifted into a range of unprivileged ids on the host.
    Unprivileged,
    #[default]
    Unknown,
}

/// the kernel network features lanlord's links will use, tried in a throwaway namespace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Links {
    pub bridge: Tri,
    pub vxlan: Tri,
    pub gretap: Tri,
    pub wireguard: Tri,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DockerState {
    pub installed: Tri,
    pub running: Tri,
    pub version: String,
    pub storage: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Environment {
    pub hostname: String,
    pub kernel: String,
    pub init: String,
    pub distro: String,
    pub distro_version: String,
    /// what `systemd-detect-virt` says: kvm, lxc, none, and so on.
    pub virt: String,
    pub container: Container,
    pub privilege: Privilege,
    /// can containers run inside this one (the proxmox feature `nesting`).
    pub nesting: Tri,
    /// is the keyctl system call allowed (the proxmox feature `keyctl`).
    pub keyctl: Tri,
    pub tun: Tri,
    pub fuse: Tri,
    pub nft: Tri,
    pub links: Links,
    pub docker: DockerState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// stops the module.
    Error,
    /// worth knowing; does not stop anything.
    Warning,
}

/// something about this machine that matters to a module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub level: Level,
    /// what the problem is about: `nesting`, `keyctl`, `tun` or `container`.
    pub subject: &'static str,
    pub text: String,
    pub fix: String,
}

/// one line of the readable report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Ok,
    Info,
    Warn,
    Fail,
}

impl Mark {
    fn label(self) -> &'static str {
        match self {
            Mark::Ok => " OK ",
            Mark::Info => "INFO",
            Mark::Warn => "WARN",
            Mark::Fail => "FAIL",
        }
    }
}

/// the read-only probe. it prints `key=value` lines. nothing it runs reads stdin, because the
/// script itself arrives on stdin. the link types are tried inside a new network namespace that
/// disappears with the command, so no real interface is ever touched.
const PROBE_SCRIPT: &str = r#"say() { printf '%s=%s\n' "$1" "$2"; }
have() { command -v "$1" >/dev/null 2>&1; }
osr() { if [ -r /etc/os-release ]; then sed -n "s/^$1=//p" /etc/os-release | head -n 1 | tr -d '"'; fi; }

say hostname "$(hostname 2>/dev/null)"
say kernel "$(uname -r 2>/dev/null)"
say init "$(cat /proc/1/comm 2>/dev/null)"
say distro "$(osr ID)"
say distro_version "$(osr VERSION_ID)"

virt=unknown
if have systemd-detect-virt; then
  virt=$(systemd-detect-virt 2>/dev/null </dev/null)
  if [ -z "$virt" ]; then virt=none; fi
fi
say virt "$virt"

container=unknown
if have systemd-detect-virt; then
  c=$(systemd-detect-virt --container 2>/dev/null </dev/null)
  if [ -n "$c" ]; then container=$c; fi
fi
if [ "$container" = unknown ]; then
  if tr '\0' '\n' < /proc/1/environ 2>/dev/null | grep -q '^container='; then
    container=$(tr '\0' '\n' < /proc/1/environ | sed -n 's/^container=//p' | head -n 1)
  elif grep -q lxc /proc/self/cgroup 2>/dev/null; then
    container=lxc
  elif [ -f /.dockerenv ]; then
    container=docker
  elif [ "$virt" != unknown ]; then
    container=none
  fi
fi
say container "$container"

privilege=unknown
if [ -r /proc/self/uid_map ]; then
  first=$(sed -n 1p /proc/self/uid_map | tr -s ' \t' ' ' | sed 's/^ //')
  case "$first" in
    "0 0 4294967295") privilege=privileged ;;
    "") ;;
    *) privilege=unprivileged ;;
  esac
fi
say privilege "$privilege"

nesting=unknown
if [ "$(cat /proc/sys/user/max_user_namespaces 2>/dev/null)" = 0 ]; then
  nesting=no
elif have unshare; then
  err=$(unshare --user --map-root-user --mount --pid --fork --mount-proc true 2>&1 </dev/null)
  if [ $? -eq 0 ]; then
    nesting=yes
  else
    case "$err" in
      *"not permitted"*|*"ermission denied"*) nesting=no ;;
    esac
  fi
fi
say nesting "$nesting"

keyctl=unknown
if have keyctl; then
  err=$(keyctl describe @u 2>&1 </dev/null)
  if [ $? -eq 0 ]; then
    keyctl=yes
  else
    case "$err" in
      *"not permitted"*|*"not implemented"*|*"ermission denied"*) keyctl=no ;;
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
    elif e in (errno.EPERM, errno.ENOSYS, errno.EACCES):
        print("no")
    else:
        print("unknown")
' 2>/dev/null </dev/null)
  if [ -n "$r" ]; then keyctl=$r; fi
fi
say keyctl "$keyctl"

if [ -c /dev/net/tun ]; then say tun yes; else say tun no; fi
if [ -c /dev/fuse ]; then say fuse yes; else say fuse no; fi

nft=unknown
if have nft; then
  if nft list ruleset >/dev/null 2>&1 </dev/null; then nft=yes; else nft=no; fi
fi
say nft "$nft"

probe_link() {
  name=$1
  shift
  if ! have ip || ! have unshare; then say "link_$name" unknown; return; fi
  out=$(unshare --net ip link add lanlordprobe type "$@" 2>&1 </dev/null)
  if [ $? -eq 0 ]; then
    say "link_$name" yes
  else
    case "$out" in
      *"unshare failed"*) say "link_$name" unknown ;;
      *) say "link_$name" no ;;
    esac
  fi
}
probe_link bridge bridge
probe_link vxlan vxlan id 42 dstport 4789
probe_link gretap gretap remote 192.0.2.1
probe_link wireguard wireguard

if have docker; then
  say docker_installed yes
  t=""
  if have timeout; then t="timeout 15"; fi
  if $t docker info >/dev/null 2>&1 </dev/null; then
    say docker_running yes
    say docker_version "$($t docker version --format '{{.Server.Version}}' 2>/dev/null </dev/null)"
    say docker_storage "$($t docker info --format '{{.Driver}}' 2>/dev/null </dev/null)"
  else
    say docker_running no
  fi
else
  say docker_installed no
  say docker_running no
fi
"#;

impl Environment {
    /// read the `key=value` lines the probe printed; anything missing stays unknown.
    pub fn parse(output: &str) -> Environment {
        let map: HashMap<&str, &str> = output
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(k, v)| (k.trim(), v.trim()))
            .collect();
        let text = |key: &str| map.get(key).copied().unwrap_or("").to_string();
        let tri = |key: &str| Tri::parse(map.get(key));
        let privilege = match map.get("privilege").copied() {
            Some("privileged") => Privilege::Privileged,
            Some("unprivileged") => Privilege::Unprivileged,
            _ => Privilege::Unknown,
        };
        Environment {
            hostname: text("hostname"),
            kernel: text("kernel"),
            init: text("init"),
            distro: text("distro"),
            distro_version: text("distro_version"),
            virt: text("virt"),
            container: Container::parse(map.get("container")),
            privilege,
            nesting: tri("nesting"),
            keyctl: tri("keyctl"),
            tun: tri("tun"),
            fuse: tri("fuse"),
            nft: tri("nft"),
            links: Links {
                bridge: tri("link_bridge"),
                vxlan: tri("link_vxlan"),
                gretap: tri("link_gretap"),
                wireguard: tri("link_wireguard"),
            },
            docker: DockerState {
                installed: tri("docker_installed"),
                running: tri("docker_running"),
                version: text("docker_version"),
                storage: text("docker_storage"),
            },
        }
    }

    /// is this an lxc container whose ids are shifted (what proxmox makes by default).
    pub fn is_unprivileged_lxc(&self) -> bool {
        self.container == Container::Lxc && self.privilege == Privilege::Unprivileged
    }

    /// a short description of the machine, like "unprivileged lxc container".
    pub fn kind(&self) -> String {
        match &self.container {
            Container::Lxc => match self.privilege {
                Privilege::Unprivileged => "unprivileged lxc container".into(),
                Privilege::Privileged => "privileged lxc container".into(),
                Privilege::Unknown => "lxc container".into(),
            },
            Container::Docker => "docker container".into(),
            Container::Podman => "podman container".into(),
            Container::Other(name) => format!("{name} container"),
            Container::None => match self.virt.as_str() {
                "" | "none" | "unknown" => "bare metal".into(),
                vm => format!("{vm} virtual machine"),
            },
            Container::Unknown => "unknown".into(),
        }
    }

    /// what stops `module` here, or is worth knowing. a definite `no` is an error; `unknown` is
    /// only a warning, so a missing tool never blocks anything.
    pub fn problems_for(&self, module: &str) -> Vec<Problem> {
        match module {
            m if DOCKER_MODULES.contains(&m) => self.docker_problems(),
            "vde" | "wireguard" | "link" => self.tun_problems(),
            _ => Vec::new(),
        }
    }

    fn docker_problems(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        match &self.container {
            Container::Lxc => {
                out.extend(feature_problem(
                    self.nesting,
                    "nesting",
                    "docker needs the nesting feature of this lxc container, and it is off",
                    "lanlord could not tell whether the nesting feature of this lxc container is on, and docker needs it",
                ));
                // keyctl is only needed when the container's ids are shifted
                if self.privilege != Privilege::Privileged {
                    out.extend(feature_problem(
                        self.keyctl,
                        "keyctl",
                        "docker needs the keyctl feature of this lxc container, and it is off",
                        "lanlord could not tell whether the keyctl feature of this lxc container is on, and docker needs it",
                    ));
                }
            }
            Container::Docker | Container::Podman => out.push(Problem {
                level: Level::Warning,
                subject: "container",
                text: format!(
                    "this server is itself a {} container; running docker inside it needs extra privileges",
                    self.kind().trim_end_matches(" container")
                ),
                fix: "use a virtual machine or an lxc container for the lanlord server".into(),
            }),
            _ => {}
        }
        out
    }

    fn tun_problems(&self) -> Vec<Problem> {
        if self.tun == Tri::No {
            vec![Problem {
                level: Level::Warning,
                subject: "tun",
                text: "/dev/net/tun is missing, which tunnels and vpns need".into(),
                fix: PROXMOX_TUN_FIX.into(),
            }]
        } else {
            Vec::new()
        }
    }

    /// the lines of the readable report, in the manner of nt setup's hardware list.
    pub fn checks(&self) -> Vec<(Mark, String)> {
        let mut lines = Vec::new();
        let docker = self.docker_problems();
        let mark_for = |subject: &str, value: Tri, needed: bool| -> Mark {
            if let Some(p) = docker.iter().find(|p| p.subject == subject) {
                return if p.level == Level::Error {
                    Mark::Fail
                } else {
                    Mark::Warn
                };
            }
            match (value, needed) {
                (Tri::Yes, _) => Mark::Ok,
                (Tri::No, _) | (Tri::Unknown, _) => Mark::Info,
            }
        };
        let word = |t: Tri| match t {
            Tri::Yes => "yes",
            Tri::No => "no",
            Tri::Unknown => "could not tell",
        };
        if self.container == Container::Lxc {
            lines.push((
                mark_for("nesting", self.nesting, true),
                format!(
                    "nesting (containers inside this one): {}",
                    word(self.nesting)
                ),
            ));
            let keyctl_needed = self.privilege != Privilege::Privileged;
            let keyctl = if keyctl_needed {
                word(self.keyctl).to_string()
            } else {
                format!(
                    "{} (not needed in a privileged container)",
                    word(self.keyctl)
                )
            };
            lines.push((
                mark_for("keyctl", self.keyctl, keyctl_needed),
                format!("keyctl (key management calls): {keyctl}"),
            ));
        }
        let tun_mark = match self.tun {
            Tri::Yes => Mark::Ok,
            Tri::No => Mark::Warn,
            Tri::Unknown => Mark::Info,
        };
        lines.push((tun_mark, format!("/dev/net/tun: {}", word(self.tun))));
        lines.push((Mark::Info, format!("/dev/fuse: {}", word(self.fuse))));
        let nft_mark = if self.nft == Tri::Yes {
            Mark::Ok
        } else {
            Mark::Warn
        };
        lines.push((nft_mark, format!("nftables: {}", word(self.nft))));
        for (name, value) in [
            ("bridge", self.links.bridge),
            ("vxlan", self.links.vxlan),
            ("gretap", self.links.gretap),
            ("wireguard", self.links.wireguard),
        ] {
            let mark = if value == Tri::Yes {
                Mark::Ok
            } else {
                Mark::Info
            };
            lines.push((
                mark,
                format!("{name} links can be created: {}", word(value)),
            ));
        }
        let docker_line = match (self.docker.installed, self.docker.running) {
            (Tri::Yes, Tri::Yes) => format!(
                "docker {} is running (storage driver {})",
                or_unknown(&self.docker.version),
                or_unknown(&self.docker.storage)
            ),
            (Tri::Yes, _) => "docker is installed but not running".into(),
            (Tri::No, _) => "docker is not installed yet".into(),
            (Tri::Unknown, _) => "docker: could not tell".into(),
        };
        lines.push((Mark::Info, docker_line));
        lines
    }

    /// the whole report as text.
    pub fn render(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "lanlord has determined that this server contains the following:\n"
        );
        let _ = writeln!(s, "  machine   {}", or_unknown(&self.hostname));
        let _ = writeln!(s, "  type      {}", self.kind());
        let _ = writeln!(
            s,
            "  system    {} {}, kernel {}, init {}",
            or_unknown(&self.distro),
            self.distro_version,
            or_unknown(&self.kernel),
            or_unknown(&self.init)
        );
        s.push('\n');
        for (mark, text) in self.checks() {
            let _ = writeln!(s, "  [{}]  {text}", mark.label());
        }
        let problems = self.docker_problems();
        if !problems.is_empty() {
            s.push_str("\nwhat to fix for docker and the modules that run in it:\n");
            for p in &problems {
                let tag = if p.level == Level::Error {
                    "FAIL"
                } else {
                    "WARN"
                };
                let _ = writeln!(s, "  [{tag}]  {}\n          fix: {}", p.text, p.fix);
            }
        }
        if self.tun == Tri::No {
            let _ = writeln!(
                s,
                "\nfor tunnels and vpns later:\n          fix: {PROXMOX_TUN_FIX}"
            );
        }
        s
    }

    /// true when something definite is wrong for docker.
    pub fn has_errors(&self) -> bool {
        self.docker_problems()
            .iter()
            .any(|p| p.level == Level::Error)
    }
}

fn or_unknown(s: &str) -> &str {
    if s.is_empty() {
        "unknown"
    } else {
        s
    }
}

fn feature_problem(value: Tri, subject: &'static str, no: &str, unknown: &str) -> Option<Problem> {
    let (level, text) = match value {
        Tri::Yes => return None,
        Tri::No => (Level::Error, no),
        Tri::Unknown => (Level::Warning, unknown),
    };
    Some(Problem {
        level,
        subject,
        text: text.to_string(),
        fix: PROXMOX_FEATURES_FIX.to_string(),
    })
}

/// the probe script, for the tests.
#[cfg(test)]
pub(crate) fn probe_script() -> &'static str {
    PROBE_SCRIPT
}

/// look at the server `session` points to.
pub fn probe(session: &Session, ask_password: AskPassword) -> Result<Environment> {
    let admin = session.capture(&["id".into(), "-un".into()])?;
    probe_as(session, admin.trim(), ask_password)
}

/// like `probe`, when the signed-in user is already known.
pub fn probe_as(session: &Session, admin: &str, ask_password: AskPassword) -> Result<Environment> {
    let out = capture_as_root(session, admin, PROBE_SCRIPT.to_string(), ask_password)?;
    Ok(Environment::parse(&out))
}

/// what stops these modules on this machine, as one message; warnings do not count.
pub fn blocking(env: &Environment, modules: &[&str]) -> Option<String> {
    let mut seen: Vec<Problem> = Vec::new();
    for module in modules {
        for problem in env.problems_for(module) {
            if problem.level == Level::Error && !seen.contains(&problem) {
                seen.push(problem);
            }
        }
    }
    if seen.is_empty() {
        return None;
    }
    let mut msg = String::from("this server cannot run docker yet:");
    for p in &seen {
        let _ = write!(msg, "\n  - {}\n    fix: {}", p.text, p.fix);
    }
    Some(msg)
}

/// the modules to check for an apply: an empty list means everything.
pub(crate) fn needs_docker(modules: &[&str]) -> bool {
    modules.is_empty() || modules.iter().any(|m| DOCKER_MODULES.contains(m))
}

/// refuse a docker-dependent apply on a machine that definitely cannot run docker. a probe
/// that fails or cannot tell never blocks; it is only noted in the log.
pub(crate) fn gate(
    session: &Session,
    admin: &str,
    modules: &[&str],
    ask_password: AskPassword,
) -> Result<()> {
    if !needs_docker(modules) {
        return Ok(());
    }
    let env = match probe_as(session, admin, ask_password) {
        Ok(env) => env,
        Err(err) => {
            emit(
                Kind::Info,
                format!("could not check the environment: {err}"),
            );
            return Ok(());
        }
    };
    let names: Vec<&str> = if modules.is_empty() {
        DOCKER_MODULES.to_vec()
    } else {
        modules.to_vec()
    };
    for module in &names {
        for p in env.problems_for(module) {
            if p.level == Level::Warning {
                emit(Kind::Info, format!("warning: {} ({})", p.text, p.fix));
            }
        }
    }
    match blocking(&env, &names) {
        Some(msg) => bail!(msg),
        None => Ok(()),
    }
}

/// after a bootstrap: say what this server can and cannot do, without stopping anything, since
/// bootstrap does not install docker.
pub(crate) fn note(session: &Session, ask_password: AskPassword) {
    match probe(session, ask_password) {
        Ok(env) => {
            emit(Kind::Info, format!("this server is a {}", env.kind()));
            for problem in env.docker_problems() {
                emit(
                    Kind::Info,
                    format!("note: {} ({})", problem.text, problem.fix),
                );
            }
        }
        Err(err) => emit(
            Kind::Info,
            format!("could not check the environment: {err}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNPRIVILEGED_LXC: &str = "hostname=lanlord\nkernel=6.8.12-4-pve\ninit=systemd\ndistro=debian\ndistro_version=13\nvirt=lxc\ncontainer=lxc\nprivilege=unprivileged\nnesting=yes\nkeyctl=yes\ntun=no\nfuse=no\nnft=yes\nlink_bridge=yes\nlink_vxlan=yes\nlink_gretap=yes\nlink_wireguard=yes\ndocker_installed=yes\ndocker_running=yes\ndocker_version=29.1.2\ndocker_storage=overlayfs\n";

    fn with(replace: &[(&str, &str)]) -> Environment {
        let mut lines: Vec<String> = UNPRIVILEGED_LXC.lines().map(str::to_string).collect();
        for (key, value) in replace {
            let prefix = format!("{key}=");
            lines.retain(|l| !l.starts_with(&prefix));
            lines.push(format!("{key}={value}"));
        }
        Environment::parse(&lines.join("\n"))
    }

    #[test]
    fn an_unprivileged_container_with_both_features_is_fine() {
        let env = Environment::parse(UNPRIVILEGED_LXC);
        assert_eq!(env.container, Container::Lxc);
        assert_eq!(env.privilege, Privilege::Unprivileged);
        assert!(env.is_unprivileged_lxc());
        assert_eq!(env.kind(), "unprivileged lxc container");
        assert_eq!(env.distro, "debian");
        assert_eq!(env.docker.version, "29.1.2");
        assert!(env.problems_for("docker").is_empty());
        assert!(!env.has_errors());
        assert!(blocking(&env, &["docker", "dns"]).is_none());
    }

    #[test]
    fn a_missing_nesting_feature_stops_docker_and_what_runs_in_it() {
        let env = with(&[("nesting", "no")]);
        let problems = env.problems_for("docker");
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].level, Level::Error);
        assert_eq!(problems[0].subject, "nesting");
        assert!(problems[0]
            .fix
            .contains("pct set <CTID> --features nesting=1,keyctl=1"));
        assert!(problems[0].fix.contains("restart the container"));
        for module in ["nt4-domain", "web-proxy", "dns"] {
            assert_eq!(env.problems_for(module), problems, "{module}");
        }
        assert!(env.problems_for("gateway").is_empty());
        let msg = blocking(&env, &["dns"]).unwrap();
        assert!(msg.contains("nesting") && msg.contains("fix:"), "{msg}");
    }

    #[test]
    fn a_missing_keyctl_feature_stops_an_unprivileged_container_only() {
        let env = with(&[("keyctl", "no")]);
        assert_eq!(env.problems_for("docker")[0].subject, "keyctl");
        assert!(env.has_errors());
        let privileged = with(&[("keyctl", "no"), ("privilege", "privileged")]);
        assert!(privileged.problems_for("docker").is_empty());
        assert_eq!(privileged.kind(), "privileged lxc container");
        // nesting still matters in a privileged container
        let no_nesting = with(&[("nesting", "no"), ("privilege", "privileged")]);
        assert!(no_nesting.has_errors());
    }

    #[test]
    fn what_cannot_be_told_only_warns() {
        let env = with(&[("nesting", "unknown"), ("keyctl", "unknown")]);
        let problems = env.problems_for("docker");
        assert_eq!(problems.len(), 2);
        assert!(problems.iter().all(|p| p.level == Level::Warning));
        assert!(
            blocking(&env, &["docker"]).is_none(),
            "unknown never blocks"
        );
        let blank = Environment::parse("");
        assert_eq!(blank.nesting, Tri::Unknown);
        assert_eq!(blank.container, Container::Unknown);
        assert!(
            blank.problems_for("docker").is_empty(),
            "not known to be lxc"
        );
    }

    #[test]
    fn virtual_machines_and_bare_metal_have_no_container_problems() {
        let kvm = Environment::parse(
            "virt=kvm\ncontainer=none\nprivilege=privileged\nnesting=no\nkeyctl=no\ntun=yes\n",
        );
        assert_eq!(kvm.kind(), "kvm virtual machine");
        assert!(kvm.problems_for("docker").is_empty());
        let metal = Environment::parse("virt=none\ncontainer=none\nprivilege=privileged\n");
        assert_eq!(metal.kind(), "bare metal");
        assert!(metal.problems_for("dns").is_empty());
        let unknown_tools = Environment::parse("hostname=x\n");
        assert_eq!(unknown_tools.kind(), "unknown");
    }

    #[test]
    fn running_inside_another_container_manager_warns() {
        let env = Environment::parse("container=docker\nvirt=docker\n");
        let problems = env.problems_for("docker");
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].level, Level::Warning);
        assert_eq!(problems[0].subject, "container");
        assert_eq!(
            Environment::parse("container=podman\n").kind(),
            "podman container"
        );
        assert_eq!(
            Environment::parse("container=systemd-nspawn\n").kind(),
            "systemd-nspawn container"
        );
    }

    #[test]
    fn a_missing_tun_device_only_warns_tunnel_modules() {
        let env = Environment::parse(UNPRIVILEGED_LXC);
        assert_eq!(env.tun, Tri::No);
        for module in ["vde", "wireguard", "link"] {
            let problems = env.problems_for(module);
            assert_eq!(problems.len(), 1, "{module}");
            assert_eq!(problems[0].level, Level::Warning);
            assert!(problems[0]
                .fix
                .contains("lxc.cgroup2.devices.allow: c 10:200 rwm"));
            assert!(problems[0]
                .fix
                .contains("lxc.mount.entry: /dev/net/tun dev/net/tun none bind,create=file"));
        }
        assert!(with(&[("tun", "yes")]).problems_for("wireguard").is_empty());
    }

    #[test]
    fn the_report_lists_the_machine_and_marks_what_is_wrong() {
        let good = Environment::parse(UNPRIVILEGED_LXC).render();
        assert!(good.contains("machine   lanlord"), "{good}");
        assert!(good.contains("unprivileged lxc container"));
        assert!(good.contains("[ OK ]  nesting"));
        assert!(good.contains("[WARN]  /dev/net/tun: no"));
        assert!(good.contains("docker 29.1.2 is running (storage driver overlayfs)"));
        assert!(!good.contains("[FAIL]"));
        let bad = with(&[("nesting", "no"), ("keyctl", "unknown")]).render();
        assert!(bad.contains("[FAIL]  nesting"), "{bad}");
        assert!(bad.contains("[WARN]  keyctl"), "{bad}");
        assert!(
            bad.contains("fix: on the proxmox host run `pct set"),
            "{bad}"
        );
        let vm = Environment::parse("virt=kvm\ncontainer=none\ndocker_installed=no\n").render();
        assert!(!vm.contains("nesting"), "only containers show it");
        assert!(vm.contains("docker is not installed yet"));
    }

    #[test]
    fn docker_states_are_described() {
        let stopped = with(&[("docker_running", "no")]).render();
        assert!(stopped.contains("docker is installed but not running"));
        let unknown = Environment::parse("container=none\n").render();
        assert!(unknown.contains("docker: could not tell"));
    }

    #[test]
    fn needs_docker_means_everything_when_no_module_is_named() {
        assert!(needs_docker(&[]));
        assert!(needs_docker(&["gateway", "dns"]));
        assert!(!needs_docker(&["gateway"]));
        assert!(!needs_docker(&["remote-commands"]));
    }

    #[test]
    fn the_docker_playbook_gives_the_same_fix() {
        let playbook = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../ansible/playbooks/docker.yml"),
        )
        .unwrap();
        assert!(playbook.contains("pct set <CTID> --features nesting=1,keyctl=1"));
        assert!(playbook.contains("pct reboot <CTID>"));
        assert!(PROXMOX_FEATURES_FIX.contains("nesting=1,keyctl=1"));
    }

    #[cfg(unix)]
    mod session {
        use super::*;
        use crate::ssh::Session;

        fn no_password(_: &str) -> Result<String> {
            bail!("no password expected")
        }

        /// a fake server that says `id -un` and prints `answer` for the probe.
        fn server(answer: &str) -> Session {
            Session::fake(&format!(
                r#"for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sudo -n true") exit 0 ;;
  *) cat >/dev/null; cat <<'ANSWER'
{answer}
ANSWER
  ;;
esac"#
            ))
        }

        #[test]
        fn the_probe_script_is_valid_sh_and_reads_no_stdin() {
            use std::io::Write;
            let mut child = std::process::Command::new("sh")
                .arg("-n")
                .stdin(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(probe_script().as_bytes())
                .unwrap();
            let out = child.wait_with_output().unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            // every external command must keep stdin away from the script that is feeding it
            for needle in ["unshare ", "keyctl describe", "docker info", "nft list"] {
                let line = probe_script().lines().find(|l| l.contains(needle)).unwrap();
                assert!(line.contains("</dev/null"), "{line}");
            }
        }

        #[test]
        fn the_probe_script_runs_here_and_prints_what_it_found() {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(probe_script())
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap();
            assert!(out.status.success());
            let text = String::from_utf8_lossy(&out.stdout);
            let env = Environment::parse(&text);
            // whatever this machine is, every key is present and nothing panics
            for key in [
                "hostname",
                "kernel",
                "virt",
                "container",
                "privilege",
                "nesting",
                "keyctl",
                "tun",
                "nft",
                "link_bridge",
                "docker_installed",
            ] {
                assert!(
                    text.lines().any(|l| l.starts_with(&format!("{key}="))),
                    "missing {key}\n{text}"
                );
            }
            assert!(!env.kernel.is_empty());
        }

        #[test]
        fn probing_reads_the_answer_through_the_root_plumbing() {
            let env = probe(&server(UNPRIVILEGED_LXC), &mut no_password).unwrap();
            assert_eq!(env.container, Container::Lxc);
            assert_eq!(env.nesting, Tri::Yes);
        }

        #[test]
        fn the_gate_blocks_only_on_a_definite_no() {
            let blocked = server("container=lxc\nprivilege=unprivileged\nnesting=no\nkeyctl=yes\n");
            let err = gate(&blocked, "tester", &["docker"], &mut no_password).unwrap_err();
            assert!(err.to_string().contains("nesting"), "{err}");
            // everything is checked when no module is named
            assert!(gate(&blocked, "tester", &[], &mut no_password).is_err());
            // a module that does not use docker is not checked
            assert!(gate(&blocked, "tester", &["gateway"], &mut no_password).is_ok());
            let unsure =
                server("container=lxc\nprivilege=unprivileged\nnesting=unknown\nkeyctl=unknown\n");
            assert!(gate(&unsure, "tester", &["docker"], &mut no_password).is_ok());
            let fine = server(UNPRIVILEGED_LXC);
            assert!(gate(&fine, "tester", &["dns"], &mut no_password).is_ok());
        }

        #[test]
        fn a_probe_that_fails_never_blocks() {
            let broken = Session::fake(
                r#"for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sudo -n true") exit 0 ;;
  *) cat >/dev/null; echo boom >&2; exit 5 ;;
esac"#,
            );
            assert!(probe_as(&broken, "tester", &mut no_password).is_err());
            assert!(gate(&broken, "tester", &["docker"], &mut no_password).is_ok());
            note(&broken, &mut no_password);
        }

        #[test]
        fn a_note_after_bootstrap_does_not_stop_anything() {
            let blocked = server("container=lxc\nprivilege=unprivileged\nnesting=no\nkeyctl=no\n");
            note(&blocked, &mut no_password);
            note(&server(UNPRIVILEGED_LXC), &mut no_password);
        }
    }
}
