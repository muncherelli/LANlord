//! `lanlord doctor`: what the gateway can do for lanlord, and how to fix the rest.
//!
//! this is the one environment probe. `lanlord apply` asks it before it starts docker-based
//! modules (through `env`), and `docker.yml` runs the same script, so the three always agree.
//!
//! the probe changes nothing that lasts. to see which kinds of link the kernel allows, it
//! creates throwaway links with unique names in the gateway's own network namespace and deletes
//! each one at once; a trap removes any that are left if the probe stops early.

use anyhow::Result;

use crate::bootstrap::{capture_as_root, AskPassword};
use crate::ssh::Session;

pub use lanlord_model::doctor::*;

/// the probe, as it is on the gateway; it prints `key=value` lines that [`Report::parse`] reads.
pub const PROBE: &str = include_str!("../../../ansible/playbooks/doctor-probe.sh");

/// probe `host`; sudo's password is asked for when needed.
pub fn run(host: &str, ask_password: AskPassword) -> Result<Report> {
    run_with(&Session::new(host)?, ask_password)
}

pub(crate) fn run_with(session: &Session, ask_password: AskPassword) -> Result<Report> {
    let admin = session.capture(&["id".into(), "-un".into()])?;
    probe_as(session, admin.trim(), ask_password)
}

/// like `run`, when the signed-in user is already known.
pub fn probe_as(session: &Session, admin: &str, ask_password: AskPassword) -> Result<Report> {
    let out = capture_as_root(session, admin, PROBE.to_string(), ask_password)?;
    Ok(Report::parse(&out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_cleans_up_and_reads_no_stdin() {
        assert!(PROBE.starts_with("#!/bin/sh"));
        assert!(PROBE.contains("trap cleanup EXIT"));
        // the whole body is one function, parsed before it runs with stdin on /dev/null, so
        // nothing in it can eat the rest of a script that arrives on stdin
        assert!(
            PROBE.trim_end().ends_with("lanlord_doctor </dev/null"),
            "the probe must end by calling itself with stdin closed"
        );
        // names fit linux's 15 characters: "lld" + a pid of up to 7 digits + one letter
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

    #[test]
    fn the_semantics_the_review_asked_for_are_in_the_script() {
        // keyctl: only ENOSYS is a definite no
        let keyctl_case = PROBE
            .lines()
            .skip_while(|l| !l.contains("err=$(keyctl describe"))
            .take(10)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            keyctl_case.contains(r#"*"not implemented"*) keyctl=no"#),
            "{keyctl_case}"
        );
        assert!(!keyctl_case.contains("not permitted"), "{keyctl_case}");
        assert!(PROBE.contains("errno.ENOSYS"));
        assert!(!PROBE.contains("errno.EPERM"));
        // a sysctl that turns user namespaces off says nothing about this container
        assert!(PROBE.contains(
            r#"if [ "$(cat /proc/sys/user/max_user_namespaces 2>/dev/null)" = 0 ]; then
    nesting=unknown"#
        ));
    }

    #[cfg(unix)]
    mod run {
        use super::*;
        use crate::testgw::no_password;

        fn gateway(answer: &str) -> Session {
            Session::fake(&format!(
                r#"for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sudo -n true") exit 0 ;;
  *) cat > /dev/null; cat <<'ANSWER'
{answer}
ANSWER
  ;;
esac"#
            ))
        }

        #[test]
        fn the_probe_script_is_valid_sh() {
            let path =
                std::env::temp_dir().join(format!("lanlord-probe-{}.sh", std::process::id()));
            std::fs::write(&path, PROBE).unwrap();
            let out = std::process::Command::new("sh")
                .arg("-n")
                .arg(&path)
                .output()
                .unwrap();
            let _ = std::fs::remove_file(&path);
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        /// what the probe says about nesting with stub tools first on the path: `uid` is what
        /// `id -u` prints, `unshare` and `systemd_run` are the stubs' shell bodies.
        fn nesting_with(uid: &str, invocation: bool, unshare: &str, systemd_run: &str) -> String {
            use std::os::unix::fs::PermissionsExt;
            let dir = std::env::temp_dir().join(format!(
                "lanlord-nesting-{}-{:p}",
                std::process::id(),
                &uid
            ));
            std::fs::create_dir_all(&dir).unwrap();
            for (name, body) in [
                ("id", format!("echo {uid}")),
                ("unshare", unshare.to_string()),
                ("systemd-run", systemd_run.to_string()),
            ] {
                let path = dir.join(name);
                std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let path_var = format!("{}:{}", dir.display(), std::env::var("PATH").unwrap());
            let mut cmd = std::process::Command::new("sh");
            cmd.arg("-c").arg(PROBE).env("PATH", path_var);
            cmd.env_remove("INVOCATION_ID");
            if invocation {
                cmd.env("INVOCATION_ID", "0123456789abcdef");
            }
            let out = cmd.output().unwrap();
            let _ = std::fs::remove_dir_all(&dir);
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .find_map(|l| l.strip_prefix("nesting=").map(str::to_string))
                .unwrap_or_default()
        }

        const DENIED: &str = "echo 'unshare: unshare failed: Operation not permitted' >&2; exit 1";

        #[test]
        fn nesting_is_tested_outside_the_callers_sandbox_when_root_under_systemd() {
            // the service's sandbox would make a direct unshare fail; the transient unit works
            assert_eq!(nesting_with("0", true, DENIED, "exit 0"), "yes");
            // a refusal from the unit itself is a real no
            assert_eq!(nesting_with("0", true, DENIED, DENIED), "no");
        }

        #[test]
        fn a_refusal_that_could_be_the_sandbox_is_unknown_never_no() {
            // systemd-run couldn't start the unit: nothing was learned
            let broken = "echo 'Failed to connect to bus: Permission denied' >&2; exit 1";
            assert_eq!(nesting_with("0", true, DENIED, broken), "unknown");
            // not root, inside a service: the direct test may be the sandbox talking
            assert_eq!(nesting_with("1000", true, DENIED, "exit 1"), "unknown");
        }

        #[test]
        fn a_plain_refusal_outside_any_service_is_a_no_and_success_is_a_yes() {
            assert_eq!(nesting_with("1000", false, DENIED, "exit 1"), "no");
            assert_eq!(nesting_with("1000", false, "exit 0", "exit 1"), "yes");
        }

        #[test]
        fn the_probe_script_runs_here_from_stdin_and_prints_what_it_found() {
            use std::io::Write;
            let mut child = std::process::Command::new("sh")
                .arg("-s")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(PROBE.as_bytes())
                .unwrap();
            let out = child.wait_with_output().unwrap();
            assert!(out.status.success());
            let text = String::from_utf8_lossy(&out.stdout);
            // whatever this machine is, every key is present (so nothing ate the script) and
            // nothing panics
            for key in [
                "kernel",
                "hostname",
                "virt_container",
                "virt_vm",
                "uid_map",
                "nesting",
                "keyctl",
                "link_bridge",
                "link_wireguard",
                "tun",
                "fuse",
                "ip_forward",
                "nft",
                "docker_installed",
                "docker_running",
                "netmgr",
                "vde_switch",
                "vde_plug",
            ] {
                assert!(
                    text.lines().any(|l| l.starts_with(&format!("{key}="))),
                    "missing {key}\n{text}"
                );
            }
            let report = Report::parse(&text);
            assert!(!report.kernel.is_empty());
        }

        #[test]
        fn the_probe_runs_as_root_and_its_answer_is_parsed() {
            let session = gateway(
                "virt_container=lxc\nvirt_vm=none\nuid_map=0 100000 65536\npve_marker=1\nnesting=yes\nkeyctl=yes\nlink_bridge=ok",
            );
            let report = run_with(&session, &mut no_password).unwrap();
            assert!(matches!(
                report.environment,
                Environment::Container {
                    privilege: Privilege::Unprivileged,
                    proxmox: true,
                    ..
                }
            ));
            assert!(report
                .checks
                .iter()
                .any(|c| c.id == "link-bridge" && c.status == Status::Ok));
            let again = probe_as(&session, "tester", &mut no_password).unwrap();
            assert_eq!(again, report);
        }
    }
}
