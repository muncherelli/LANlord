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

/// the shell that finds the processor's model: `model name`, then the `Hardware` and `Model`
/// lines of an arm board, then `lscpu`.
const CPU_SNIPPET: &str = r#"cpu=
for k in 'model name' Hardware Model; do
  cpu=$(sed -n "s/^$k[[:space:]]*:[[:space:]]*//p" /proc/cpuinfo 2>/dev/null | head -n 1)
  if [ -n "$cpu" ]; then break; fi
done
if [ -z "$cpu" ]; then
  cpu=$(lscpu 2>/dev/null | sed -n 's/^Model name:[[:space:]]*//p' | head -n 1)
fi
echo "cpu_model=$cpu"
"#;

/// the shell that counts the sockets and the physical cores (a socket and core id pair each)
/// from `/proc/cpuinfo`; both stay 0 where it has no ids, as on most arm boards.
const TOPOLOGY_SNIPPET: &str = r#"awk -F: '
/^physical id/ { p = $2; s[p] = 1 }
/^core id/ { c[p "/" $2] = 1 }
END { for (k in s) a++; for (k in c) b++; print "sockets=" a + 0; print "cores=" b + 0 }
' /proc/cpuinfo 2>/dev/null
"#;

/// the shell that reads what a container's cgroup allows: memory (v2, then v1), the cpu quota
/// and the cpus the container may run on. a value that is not there is left empty.
const CGROUP_SNIPPET: &str = r#"for f in /sys/fs/cgroup/memory.max \
    /sys/fs/cgroup/memory/memory.limit_in_bytes; do
  if [ -r "$f" ]; then echo "cgroup_mem_max=$(cat "$f" 2>/dev/null)"; break; fi
done
for f in /sys/fs/cgroup/memory.current \
    /sys/fs/cgroup/memory/memory.usage_in_bytes; do
  if [ -r "$f" ]; then echo "cgroup_mem_current=$(cat "$f" 2>/dev/null)"; break; fi
done
echo "cgroup_cpu_max=$(cat /sys/fs/cgroup/cpu.max 2>/dev/null)"
for d in /sys/fs/cgroup/cpu /sys/fs/cgroup/cpu,cpuacct; do
  if [ -r "$d/cpu.cfs_quota_us" ]; then
    echo "cgroup_cpu_v1=$(cat "$d/cpu.cfs_quota_us" 2>/dev/null) \
$(cat "$d/cpu.cfs_period_us" 2>/dev/null)"
    break
  fi
done
for f in /sys/fs/cgroup/cpuset.cpus.effective \
    /sys/fs/cgroup/cpuset/cpuset.effective_cpus; do
  if [ -r "$f" ]; then echo "cgroup_cpuset=$(cat "$f" 2>/dev/null)"; break; fi
done
"#;

/// the read-only script; sections are separated by `---` lines.
fn script(etc: &str, usr: &str) -> String {
    format!(
        r#"echo "hostname=$(hostname 2>/dev/null)"
{os}echo "kernel=$(uname -sr 2>/dev/null)"
echo "uptime=$(cut -d. -f1 /proc/uptime 2>/dev/null)"
echo "user=$(id -un)"
echo "uid=$(id -u)"
echo "cpus=$(grep -c '^processor' /proc/cpuinfo 2>/dev/null)"
{topology}{cpu}{cgroup}echo "mem_total_kb=$(awk '/^MemTotal/{{print $2}}' /proc/meminfo 2>/dev/null)"
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
        os = os_snippet(etc, usr),
        topology = TOPOLOGY_SNIPPET,
        cpu = CPU_SNIPPET,
        cgroup = CGROUP_SNIPPET
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

    fn limited(extra: &str) -> ServerFacts {
        let head = "hostname=h\ncpus=8\nmem_total_kb=8388608\nmem_available_kb=6291456\n";
        parse(&format!("{head}{extra}---link\n---addr\n---route\n")).unwrap()
    }

    #[test]
    fn a_container_limit_below_the_host_is_what_the_server_has() {
        let free = limited("");
        assert_eq!((free.cpus, free.mem_total_mb), (8, 8192));
        assert!(!free.cpus_limited && !free.mem_limited);
        assert_eq!(free.cpu_rows(), [("Cores", "8".to_string())]);
        // no limit: cgroup v2 says `max`, v1 a number past the host's memory
        let open = limited("cgroup_mem_max=max\ncgroup_cpu_max=max 100000\ncgroup_cpuset=0-7\n");
        assert!(!open.cpus_limited && !open.mem_limited);
        let huge = limited("cgroup_mem_max=9223372036854771712\n");
        assert_eq!(huge.mem_total_mb, 8192);
        let box_ = limited(concat!(
            "cgroup_mem_max=2147483648\ncgroup_mem_current=1073741824\n",
            "cgroup_cpu_max=150000 100000\ncgroup_cpuset=0-3\n"
        ));
        assert_eq!((box_.mem_total_mb, box_.mem_available_mb), (2048, 1024));
        assert_eq!(
            box_.memory(),
            "2048 MB, 1024 MB free (limited by container)"
        );
        // the lower of the quota (1.5 rounds up to 2) and the cpuset (4)
        assert_eq!((box_.cpus, box_.cpus_limited), (2, true));
        assert_eq!(
            box_.cpu_rows(),
            [("Threads", "2 (limited by container)".to_string())]
        );
        // cgroup v1: a quota over a period, and -1 for none
        let v1 = limited("cgroup_cpu_v1=200000 100000\n");
        assert_eq!((v1.cpus, v1.cpus_limited), (2, true));
        assert!(!limited("cgroup_cpu_v1=-1 100000\n").cpus_limited);
        // a cpuset that cannot be counted limits nothing, and does not panic
        assert!(!limited("cgroup_cpuset=0-4294967295\n").cpus_limited);
        assert!(!limited("cgroup_cpuset=5-1\n").cpus_limited);
        let one = limited("cgroup_cpuset=3\n");
        assert_eq!(
            one.cpu_rows(),
            [("Threads", "1 (limited by container)".to_string())]
        );
        let set = limited("cgroup_cpuset=0-1,4,6-7\n");
        assert_eq!(set.cpus, 5);
        // a limit above the host's is no limit
        let big = limited("cgroup_mem_max=17179869184\ncgroup_cpu_max=1600000 100000\n");
        assert!(!big.mem_limited && !big.cpus_limited);
    }

    #[test]
    fn last_applied_shows_in_the_zone_it_is_asked_for() {
        use chrono::FixedOffset;
        let utc = FixedOffset::east_opt(0).unwrap();
        let cst = FixedOffset::west_opt(6 * 3600).unwrap();
        let raw = "2026-11-18T19:42:07Z";
        assert_eq!(format_applied(raw, &utc), "11/18/2026 7:42:07 PM");
        assert_eq!(format_applied(raw, &cst), "11/18/2026 1:42:07 PM");
        // a date line carrying its own offset converts too
        assert_eq!(
            format_applied("2026-11-15T20:41:07-06:00\n", &utc),
            "11/16/2026 2:41:07 AM"
        );
        assert_eq!(format_applied("last tuesday", &cst), "last tuesday");
        let mut facts = demo();
        facts.last_applied = None;
        assert_eq!(facts.last_applied_local(), None);
        facts.last_applied = Some("last tuesday".into());
        assert_eq!(facts.last_applied_local().as_deref(), Some("last tuesday"));
        facts.last_applied = Some(raw.into());
        assert!(facts.last_applied_local().unwrap().ends_with('M'));
    }

    #[test]
    fn sockets_cores_and_threads_show_by_the_owners_rule() {
        let rows = |extra: &str| limited(extra).cpu_rows();
        let n = |s: &str| s.to_string();
        // one socket, four cores, eight threads
        assert_eq!(
            rows("sockets=1\ncores=4\n"),
            [("Cores", n("4")), ("Threads", n("8"))]
        );
        // two sockets, one thread per core
        assert_eq!(
            rows("sockets=2\ncores=8\n"),
            [("Processors", n("2")), ("Cores", n("8"))]
        );
        assert_eq!(
            rows("sockets=2\ncores=4\n"),
            [
                ("Processors", n("2")),
                ("Cores", n("4")),
                ("Threads", n("8"))
            ]
        );
        // one core, one thread
        let one = parse("cpus=1\nsockets=1\ncores=1\n---link\n---addr\n---route\n").unwrap();
        assert_eq!(one.cpu_rows(), [("Cores", n("1"))]);
        // no ids (an arm board): as many cores as threads
        assert_eq!(rows(""), [("Cores", n("8"))]);
        // nothing at all is not a number
        let none = parse("hostname=h\n---link\n---addr\n---route\n").unwrap();
        assert_eq!(none.cpu_rows(), [("Cores", n("Unknown"))]);
    }

    #[test]
    fn old_facts_without_sockets_and_cores_still_parse() {
        let json = serde_json::to_value(demo()).unwrap();
        let mut old = json.as_object().unwrap().clone();
        old.remove("sockets");
        old.remove("cores");
        let facts: ServerFacts = serde_json::from_value(old.into()).unwrap();
        assert_eq!((facts.sockets, facts.cores, facts.cpus), (0, 0, 4));
    }

    #[test]
    fn the_topology_script_counts_sockets_and_cores() {
        let cpuinfo = "processor : 0\nphysical id : 0\ncore id : 0\n\n\
                       processor : 1\nphysical id : 0\ncore id : 0\n\n\
                       processor : 2\nphysical id : 1\ncore id : 0\n\n\
                       processor : 3\nphysical id : 1\ncore id : 1\n";
        let dir = std::env::temp_dir().join(format!("lanlord-topo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("cpuinfo");
        std::fs::write(&file, cpuinfo).unwrap();
        let script = TOPOLOGY_SNIPPET.replace("/proc/cpuinfo", file.to_str().unwrap());
        let out = std::process::Command::new("sh")
            .args(["-c", &script])
            .output();
        std::fs::remove_dir_all(&dir).ok();
        let out = String::from_utf8(out.unwrap().stdout).unwrap();
        assert_eq!(out, "sockets=2\ncores=3\n");
    }

    #[test]
    fn the_processor_model_is_read_from_the_facts() {
        assert_eq!(demo().cpu_model, "Intel(R) Core(TM) i7-4790K CPU @ 4.00GHz");
        assert_eq!(limited("cpu_model=Cortex-A72\n").cpu_model, "Cortex-A72");
        assert_eq!(limited("").cpu_model, "");
    }

    #[test]
    fn the_cpu_and_cgroup_snippets_read_a_model_and_limits() {
        // the real files of this machine: whatever they say, the script still runs and ends well
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(script("/etc/os-release", "/usr/lib/os-release"))
            .output()
            .unwrap();
        assert!(out.status.success());
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("cpu_model="), "{text}");
        assert!(parse(&text).is_ok());
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
