//! what the signed-in server is: its name, system, uptime and network interfaces, from one
//! read-only script that needs no sudo. the My Server and Networks windows show it.

use anyhow::Result;

use crate::ssh::Session;

pub use lanlord_model::server::*;

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
echo "---route6"
ip -6 route show default 2>/dev/null
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
