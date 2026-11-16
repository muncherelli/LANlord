//! what the signed-in server is: its name, system, uptime and network interfaces, from one
//! read-only script that needs no sudo. the My Server and Networks windows show it.

use anyhow::{bail, Result};

use crate::ssh::Session;

/// one network interface and what it is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Iface {
    pub name: String,
    pub mtu: u32,
    /// the kernel's state: UP, DOWN or UNKNOWN (loopback and some virtual links).
    pub state: String,
    /// the ipv4 addresses with their prefix, like `192.0.2.10/24`.
    pub addresses: Vec<String>,
    /// the default route leaves through this interface.
    pub default_route: bool,
}

impl Iface {
    pub fn loopback(&self) -> bool {
        self.name == "lo"
    }

    /// the state as a word for a list: Connected, Disconnected or Unknown.
    pub fn link(&self) -> &'static str {
        match self.state.as_str() {
            "UP" => "Connected",
            "DOWN" | "LOWERLAYERDOWN" => "Disconnected",
            _ => "Unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerFacts {
    pub hostname: String,
    pub os: String,
    pub kernel: String,
    pub uptime_secs: u64,
    pub user: String,
    pub is_root: bool,
    pub cpus: u32,
    pub mem_total_mb: u64,
    pub mem_available_mb: u64,
    /// lanlord's directory and service account are both there.
    pub lanlord_installed: bool,
    pub lanlord_version: Option<String>,
    pub last_applied: Option<String>,
    pub interfaces: Vec<Iface>,
}

impl ServerFacts {
    /// the uptime as `3 days, 4 hours, 12 minutes`.
    pub fn uptime(&self) -> String {
        let (d, h, m) = (
            self.uptime_secs / 86_400,
            self.uptime_secs % 86_400 / 3_600,
            self.uptime_secs % 3_600 / 60,
        );
        let part = |n: u64, one: &str| format!("{n} {one}{}", if n == 1 { "" } else { "s" });
        match (d, h) {
            (0, 0) => part(m, "minute"),
            (0, _) => format!("{}, {}", part(h, "hour"), part(m, "minute")),
            _ => format!(
                "{}, {}, {}",
                part(d, "day"),
                part(h, "hour"),
                part(m, "minute")
            ),
        }
    }
}

/// the shell that finds the distribution's name: the server's own os-release file (the second
/// place when the first is missing), then `uname`. nothing is guessed from other files.
fn os_snippet(etc: &str, usr: &str) -> String {
    format!(
        r#"os=
for f in {etc} {usr}; do
  if [ -r "$f" ]; then os=$(. "$f" 2>/dev/null; echo "${{PRETTY_NAME:-$NAME}}"); break; fi
done
if [ -z "$os" ]; then os=$(uname -s 2>/dev/null); fi
echo "os=$os"
"#
    )
}

/// the read-only script; sections are separated by `---` lines.
fn script(etc: &str, usr: &str) -> String {
    format!(
        r#"echo "hostname=$(hostname 2>/dev/null)"
{os}echo "kernel=$(uname -sr 2>/dev/null)"
echo "uptime=$(cut -d. -f1 /proc/uptime 2>/dev/null)"
echo "user=$(id -un)"
echo "uid=$(id -u)"
echo "cpus=$(grep -c '^processor' /proc/cpuinfo 2>/dev/null)"
echo "mem_total_kb=$(awk '/^MemTotal/{{print $2}}' /proc/meminfo 2>/dev/null)"
echo "mem_available_kb=$(awk '/^MemAvailable/{{print $2}}' /proc/meminfo 2>/dev/null)"
if [ -d /opt/lanlord ] && getent passwd lanlord >/dev/null 2>&1; then echo installed=1; else echo installed=0; fi
echo "version=$(cat /opt/lanlord/state/version 2>/dev/null)"
echo "applied=$(cat /opt/lanlord/state/applied 2>/dev/null)"
echo "---link"
ip -o link show 2>/dev/null
echo "---addr"
ip -o -4 addr show 2>/dev/null
echo "---route"
ip route show default 2>/dev/null
exit 0
"#,
        os = os_snippet(etc, usr)
    )
}

/// read the facts of the server `session` reaches.
pub fn read(session: &Session) -> Result<ServerFacts> {
    let script = script("/etc/os-release", "/usr/lib/os-release");
    parse(&session.capture(&["sh".into(), "-c".into(), script])?)
}

fn field<'a>(lines: &[&'a str], key: &str) -> &'a str {
    lines
        .iter()
        .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
        .unwrap_or("")
        .trim()
}

fn number(lines: &[&str], key: &str) -> u64 {
    field(lines, key).parse().unwrap_or(0)
}

/// the words after `key` in `line`, like the number after `mtu`.
fn after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let mut words = line.split_whitespace();
    words.find(|w| *w == key)?;
    words.next()
}

fn parse_links(text: &str) -> Vec<Iface> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            words.next()?; // the index, like `2:`
            let name = words.next()?.trim_end_matches(':');
            let name = name.split('@').next().unwrap_or(name);
            (!name.is_empty()).then(|| Iface {
                name: name.to_string(),
                mtu: after(line, "mtu").and_then(|m| m.parse().ok()).unwrap_or(0),
                state: after(line, "state").unwrap_or("UNKNOWN").to_string(),
                addresses: Vec::new(),
                default_route: false,
            })
        })
        .collect()
}

/// the interface a `default via ... dev NAME` line leaves through.
fn default_interface(route: &str) -> Option<&str> {
    route.lines().find_map(|l| after(l, "dev"))
}

/// the facts in the output of the read script.
pub fn parse(output: &str) -> Result<ServerFacts> {
    let mut parts = output.split("---link");
    let head: Vec<&str> = parts.next().unwrap_or("").lines().collect();
    let Some(rest) = parts.next() else {
        bail!("the server did not answer with its facts");
    };
    let (links, rest) = rest.split_once("---addr").unwrap_or((rest, ""));
    let (addrs, route) = rest.split_once("---route").unwrap_or((rest, ""));
    let mut interfaces = parse_links(links);
    for line in addrs.lines() {
        let mut words = line.split_whitespace();
        words.next();
        let (Some(name), Some("inet"), Some(address)) = (words.next(), words.next(), words.next())
        else {
            continue;
        };
        if let Some(iface) = interfaces.iter_mut().find(|i| i.name == name) {
            iface.addresses.push(address.to_string());
        }
    }
    if let Some(name) = default_interface(route) {
        if let Some(iface) = interfaces.iter_mut().find(|i| i.name == name) {
            iface.default_route = true;
        }
    }
    let version = field(&head, "version");
    let applied = field(&head, "applied");
    Ok(ServerFacts {
        hostname: field(&head, "hostname").to_string(),
        os: field(&head, "os").to_string(),
        kernel: field(&head, "kernel").to_string(),
        uptime_secs: number(&head, "uptime"),
        user: field(&head, "user").to_string(),
        is_root: field(&head, "uid") == "0",
        cpus: number(&head, "cpus") as u32,
        mem_total_mb: number(&head, "mem_total_kb") / 1024,
        mem_available_mb: number(&head, "mem_available_kb") / 1024,
        lanlord_installed: field(&head, "installed") == "1",
        lanlord_version: (!version.is_empty()).then(|| version.to_string()),
        last_applied: (!applied.is_empty()).then(|| applied.to_string()),
        interfaces,
    })
}

/// made-up facts for trying the windows without a server (`LANLORD_FAKE_SERVER=1`).
pub fn demo() -> ServerFacts {
    parse(DEMO).expect("the demo facts parse")
}

const DEMO: &str = "hostname=demo\nos=Arch Linux\nkernel=Linux 6.12.7-arch1-1\nuptime=273900\nuser=root\nuid=0\ncpus=4\nmem_total_kb=4194304\nmem_available_kb=3407872\ninstalled=1\nversion=0.1.0\napplied=2026-11-15T20:41:07-06:00\n---link\n1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536 qdisc noqueue state UNKNOWN mode DEFAULT group default qlen 1000\\    link/loopback 00:00:00:00:00:00 brd 00:00:00:00:00:00\n2: eth0@if48: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc noqueue state UP mode DEFAULT group default qlen 1000\\    link/ether 02:00:00:00:aa:01 brd ff:ff:ff:ff:ff:ff link-netnsid 0\n3: eth1@if49: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1450 qdisc noqueue state UP mode DEFAULT group default qlen 1000\\    link/ether 02:00:00:00:aa:02 brd ff:ff:ff:ff:ff:ff link-netnsid 0\n---addr\n1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever preferred_lft forever\n2: eth0    inet 192.0.2.10/24 metric 1024 brd 192.0.2.255 scope global dynamic eth0\\       valid_lft 3600sec preferred_lft 3600sec\n3: eth1    inet 172.16.120.1/22 brd 172.16.123.255 scope global eth1\\       valid_lft forever preferred_lft forever\n---route\ndefault via 192.0.2.1 dev eth0 proto dhcp src 192.0.2.10 metric 1024\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_facts_describe_a_server() {
        let facts = demo();
        assert_eq!(facts.hostname, "demo");
        assert!(facts.is_root && facts.lanlord_installed);
        assert_eq!(
            (facts.cpus, facts.mem_total_mb, facts.mem_available_mb),
            (4, 4096, 3328)
        );
        assert_eq!(facts.lanlord_version.as_deref(), Some("0.1.0"));
        let names: Vec<_> = facts.interfaces.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["lo", "eth0", "eth1"]);
        let eth0 = &facts.interfaces[1];
        assert_eq!(
            (eth0.mtu, eth0.link(), eth0.default_route),
            (1500, "Connected", true)
        );
        assert_eq!(eth0.addresses, ["192.0.2.10/24"]);
        let eth1 = &facts.interfaces[2];
        assert_eq!((eth1.mtu, eth1.default_route), (1450, false));
        assert!(facts.interfaces[0].loopback() && facts.interfaces[0].link() == "Unknown");
    }

    #[test]
    fn uptime_reads_like_a_sentence() {
        let mut facts = demo();
        assert_eq!(facts.uptime(), "3 days, 4 hours, 5 minutes");
        facts.uptime_secs = 3_660;
        assert_eq!(facts.uptime(), "1 hour, 1 minute");
        facts.uptime_secs = 120;
        assert_eq!(facts.uptime(), "2 minutes");
    }

    #[test]
    fn a_server_that_lacks_tools_still_answers() {
        let facts =
            parse("hostname=h\nuid=1000\nuser=olive\ninstalled=0\n---link\n---addr\n---route\n")
                .unwrap();
        assert_eq!((facts.user.as_str(), facts.is_root), ("olive", false));
        assert!(!facts.lanlord_installed && facts.interfaces.is_empty());
        assert_eq!(
            (facts.cpus, facts.mem_total_mb, facts.last_applied),
            (0, 0, None)
        );
        assert!(parse("garbage").is_err());
        let down =
            parse_links("4: tap0: <BROADCAST> mtu 1500 qdisc noop state DOWN mode DEFAULT\n");
        assert_eq!(down[0].link(), "Disconnected");
    }

    #[test]
    fn the_script_is_valid_sh_and_read_only() {
        use std::io::Write;
        let mut child = std::process::Command::new("sh")
            .arg("-n")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let script = script("/etc/os-release", "/usr/lib/os-release");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success());
        for forbidden in [
            "rm ",
            "tee ",
            "> /",
            "useradd",
            "docker",
            "lsb_release",
            "debian",
        ] {
            assert!(!script.contains(forbidden), "{forbidden}");
        }
    }

    /// what the os snippet prints for these two os-release files (either may be missing).
    #[cfg(unix)]
    fn os_of(name: &str, etc: Option<&str>, usr: Option<&str>) -> String {
        let dir = std::env::temp_dir().join(format!("lanlord-os-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = |file: &str, text: Option<&str>| {
            let path = dir.join(file);
            if let Some(text) = text {
                std::fs::write(&path, text).unwrap();
            }
            path.display().to_string()
        };
        let (etc, usr) = (path("etc-os-release", etc), path("usr-os-release", usr));
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(os_snippet(&etc, &usr))
            .output()
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[cfg(unix)]
    const ARCH: &str =
        "NAME=\"Arch Linux\"\nPRETTY_NAME=\"Arch Linux\"\nID=arch\nBUILD_ID=rolling\n";
    #[cfg(unix)]
    const DEBIAN: &str ="PRETTY_NAME=\"Debian GNU/Linux 13 (trixie)\"\nNAME=\"Debian GNU/Linux\"\nVERSION_ID=\"13\"\nID=debian\n";

    #[cfg(unix)]
    #[test]
    fn the_distribution_comes_from_the_servers_own_os_release() {
        assert_eq!(os_of("arch", Some(ARCH), None), "os=Arch Linux");
        assert_eq!(
            os_of("debian", Some(DEBIAN), None),
            "os=Debian GNU/Linux 13 (trixie)"
        );
        // a file with only NAME still names the system
        assert_eq!(
            os_of("name", Some("NAME=\"Alpine Linux\"\n"), None),
            "os=Alpine Linux"
        );
        // the first file wins, and the second stands in when the first is missing
        assert_eq!(os_of("both", Some(ARCH), Some(DEBIAN)), "os=Arch Linux");
        assert_eq!(os_of("usr", None, Some(ARCH)), "os=Arch Linux");
        // neither: the kernel's name, never a guess
        let none = os_of("none", None, None);
        assert!(none.starts_with("os=") && !none.contains("Debian") && !none.contains("Arch"));
        // what is parsed is what was printed
        let line = os_of("parse", Some(ARCH), None);
        let facts = parse(&format!("{line}\n---link\n---addr\n---route\n")).unwrap();
        assert_eq!(facts.os, "Arch Linux");
    }

    #[test]
    fn the_demo_server_is_arch() {
        let facts = demo();
        assert_eq!(facts.os, "Arch Linux");
        assert!(facts.kernel.contains("arch"), "{}", facts.kernel);
    }
}
