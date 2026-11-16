//! what stops a module on this machine, read from the doctor report, so that lanlord can say what
//! is missing before it enables something that needs it. docker in an unprivileged lxc container
//! is the main case: it needs the proxmox features nesting and keyctl.
//!
//! there is one probe, `doctor` (`lanlord doctor`, and `docker.yml` runs the same script). every
//! check tolerates a missing tool and says unknown instead of failing, and lanlord only ever
//! blocks on a definite failure: a probe that fails or cannot tell never stops an apply.

use std::fmt::Write as _;

use anyhow::{bail, Result};

use crate::bootstrap::AskPassword;
use crate::doctor::{self, Check, Report, Status};
use crate::log::{emit, Kind};
use crate::ssh::Session;

pub use lanlord_model::doctor::{DOCKER_MODULES, PROXMOX_FEATURES_FIX, PROXMOX_TUN_FIX};

/// the checks of the report that `module` needs, so that `problems_for` can pick them out.
fn needed_checks(module: &str) -> &'static [&'static str] {
    match module {
        // keyctl is only needed when the container's ids are shifted; the report says a
        // privileged container needs none by marking that check ok
        m if DOCKER_MODULES.contains(&m) => &["lxc-nesting", "lxc-keyctl", "nested-container"],
        "vde" | "wireguard" | "link" => &["tun"],
        _ => &[],
    }
}

/// the checks `module` needs that are not ok on this machine. a `Fail` stops the module; the
/// rest (`Unknown`, `Warn`) are only worth saying, so a missing tool never blocks anything.
pub fn problems_for<'a>(report: &'a Report, module: &str) -> Vec<&'a Check> {
    let ids = needed_checks(module);
    report
        .checks
        .iter()
        .filter(|c| ids.contains(&c.id.as_str()) && c.status != Status::Ok)
        .collect()
}

/// what stops these modules on this machine, as one message; warnings do not count.
pub fn blocking(report: &Report, modules: &[&str]) -> Option<String> {
    let mut seen: Vec<&Check> = Vec::new();
    for module in modules {
        for check in problems_for(report, module) {
            if check.status == Status::Fail && !seen.iter().any(|s| s.id == check.id) {
                seen.push(check);
            }
        }
    }
    if seen.is_empty() {
        return None;
    }
    let mut msg = String::from("this server cannot run docker yet:");
    for c in &seen {
        let _ = write!(msg, "\n  - {}", c.detail);
        if let Some(fix) = &c.fix {
            let _ = write!(msg, "\n    fix: {fix}");
        }
    }
    Some(msg)
}

/// the modules an apply will run, as far as docker is concerned. modules named on the command
/// line are checked as named. a plain apply (none named) runs the modules the settings configure:
/// the ones whose top-level key is in `settings`, and none at all without settings, so a gateway
/// that only has the baseline is never held up by docker.
pub(crate) fn modules_to_check<'a>(modules: &[&'a str], settings: Option<&[u8]>) -> Vec<&'a str> {
    if !modules.is_empty() {
        return modules.to_vec();
    }
    let Some(settings) = settings else {
        return Vec::new();
    };
    let Ok(serde_yaml_ng::Value::Mapping(map)) = serde_yaml_ng::from_slice(settings) else {
        return Vec::new();
    };
    DOCKER_MODULES
        .iter()
        .copied()
        .filter(|m| map.contains_key(serde_yaml_ng::Value::String((*m).to_string())))
        .collect()
}

/// refuse a docker-dependent apply on a machine that definitely cannot run docker. a probe
/// that fails or cannot tell never blocks; it is only noted in the log. `settings` are the
/// merged settings the apply will use, which say what a plain apply runs.
pub(crate) fn gate(
    session: &Session,
    admin: &str,
    modules: &[&str],
    settings: Option<&[u8]>,
    ask_password: AskPassword,
) -> Result<()> {
    let names = modules_to_check(modules, settings);
    if !names.iter().any(|m| DOCKER_MODULES.contains(m)) {
        return Ok(());
    }
    let report = match doctor::probe_as(session, admin, ask_password) {
        Ok(report) => report,
        Err(err) => {
            emit(
                Kind::Info,
                format!("could not check the environment: {err}"),
            );
            return Ok(());
        }
    };
    let mut said: Vec<&str> = Vec::new();
    for module in &names {
        for c in problems_for(&report, module) {
            if c.status != Status::Fail && !said.contains(&c.id.as_str()) {
                said.push(&c.id);
                emit(Kind::Info, format!("warning: {}", with_fix(c)));
            }
        }
    }
    match blocking(&report, &names) {
        Some(msg) => bail!(msg),
        None => Ok(()),
    }
}

/// a check's detail with its fix after it.
fn with_fix(check: &Check) -> String {
    match &check.fix {
        Some(fix) => format!("{} ({fix})", check.detail),
        None => check.detail.clone(),
    }
}

/// after a bootstrap: say what this server can and cannot do, without stopping anything, since
/// bootstrap does not install docker.
pub(crate) fn note(session: &Session, ask_password: AskPassword) {
    match doctor::run_with(session, ask_password) {
        Ok(report) => {
            emit(
                Kind::Info,
                format!("this server is a {}", report.environment.describe()),
            );
            for check in problems_for(&report, "docker") {
                emit(Kind::Info, format!("note: {}", with_fix(check)));
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

    const UNPRIVILEGED_LXC: &str = "hostname=lanlord\nkernel=6.8.12-4-pve\ninit=systemd\ndistro=debian\ndistro_version=13\nvirt_container=lxc\nvirt_vm=none\nuid_map=0 100000 65536\nnesting=yes\nkeyctl=yes\ntun=ok\nfuse=no\nnft=ok\nlink_bridge=ok\nlink_vxlan=ok\nlink_gretap=ok\nlink_wireguard=ok\ndocker_installed=yes\ndocker_running=yes\ndocker_version=29.1.2\ndocker_storage=overlayfs\n";

    fn with(replace: &[(&str, &str)]) -> Report {
        let mut lines: Vec<String> = UNPRIVILEGED_LXC.lines().map(str::to_string).collect();
        for (key, value) in replace {
            let prefix = format!("{key}=");
            lines.retain(|l| !l.starts_with(&prefix));
            lines.push(format!("{key}={value}"));
        }
        Report::parse(&lines.join("\n"))
    }

    fn ids(problems: &[&Check]) -> Vec<String> {
        problems.iter().map(|c| c.id.clone()).collect()
    }

    #[test]
    fn an_unprivileged_container_with_both_features_is_fine() {
        let report = with(&[]);
        for module in ["docker", "nt4-domain", "web-proxy", "dns"] {
            assert!(problems_for(&report, module).is_empty(), "{module}");
        }
        assert_eq!(blocking(&report, DOCKER_MODULES), None);
    }

    #[test]
    fn a_missing_nesting_feature_stops_docker_and_what_runs_in_it() {
        let report = with(&[("nesting", "no")]);
        for module in DOCKER_MODULES {
            assert_eq!(
                ids(&problems_for(&report, module)),
                ["lxc-nesting"],
                "{module}"
            );
        }
        let msg = blocking(&report, &["dns"]).unwrap();
        assert!(
            msg.starts_with("this server cannot run docker yet:"),
            "{msg}"
        );
        assert!(msg.contains("nesting feature"), "{msg}");
        assert!(
            msg.contains("pct set <CTID> --features nesting=1,keyctl=1"),
            "{msg}"
        );
        // asked about several modules, each problem is said once
        let both = blocking(&report, &["docker", "dns", "web-proxy"]).unwrap();
        assert_eq!(both.matches("nesting feature").count(), 1, "{both}");
        // a module that does not use docker is not held up
        assert!(problems_for(&report, "gateway").is_empty());
        assert_eq!(blocking(&report, &["gateway"]), None);
    }

    #[test]
    fn a_missing_keyctl_feature_stops_an_unprivileged_container_only() {
        let report = with(&[("keyctl", "no")]);
        assert_eq!(ids(&problems_for(&report, "docker")), ["lxc-keyctl"]);
        assert!(blocking(&report, &["docker"]).unwrap().contains("keyctl"));
        // a privileged container needs no keyctl
        let privileged = with(&[("keyctl", "no"), ("uid_map", "0 0 4294967295")]);
        assert!(problems_for(&privileged, "docker").is_empty());
        assert_eq!(blocking(&privileged, &["docker"]), None);
        // a container whose privilege can't be told is treated as needing it
        let unsure = with(&[("keyctl", "no"), ("uid_map", "")]);
        assert_eq!(ids(&problems_for(&unsure, "docker")), ["lxc-keyctl"]);
    }

    #[test]
    fn what_cannot_be_told_only_warns() {
        let report = with(&[("nesting", "unknown"), ("keyctl", "unknown")]);
        let problems = problems_for(&report, "docker");
        assert_eq!(ids(&problems), ["lxc-nesting", "lxc-keyctl"]);
        assert!(problems.iter().all(|c| c.status == Status::Unknown));
        assert_eq!(blocking(&report, DOCKER_MODULES), None);
        // so does a probe with nothing in it
        let nothing = Report::parse("");
        assert_eq!(blocking(&nothing, DOCKER_MODULES), None);
        // lxc-libvirt is not an lxc container with the proxmox features
        let libvirt = with(&[("virt_container", "lxc-libvirt"), ("nesting", "no")]);
        assert!(problems_for(&libvirt, "docker").is_empty());
        assert_eq!(blocking(&libvirt, &["docker"]), None);
    }

    #[test]
    fn virtual_machines_and_bare_metal_have_no_container_problems() {
        for env in [
            "virt_container=none\nvirt_vm=kvm\nnesting=no\nkeyctl=no\n",
            "virt_container=none\nvirt_vm=none\nnesting=no\nkeyctl=no\n",
        ] {
            let report = Report::parse(env);
            assert!(problems_for(&report, "docker").is_empty(), "{env}");
        }
    }

    #[test]
    fn running_inside_another_container_manager_warns() {
        for manager in ["docker", "podman"] {
            let report = with(&[("virt_container", manager)]);
            let problems = problems_for(&report, "docker");
            assert_eq!(ids(&problems), ["nested-container"], "{manager}");
            assert_eq!(problems[0].status, Status::Warn);
            assert_eq!(blocking(&report, &["docker"]), None);
        }
    }

    #[test]
    fn a_missing_tun_device_only_warns_tunnel_modules() {
        let report = with(&[("tun", "missing")]);
        for module in ["vde", "wireguard", "link"] {
            let problems = problems_for(&report, module);
            assert_eq!(ids(&problems), ["tun"], "{module}");
            assert_eq!(problems[0].fix.as_deref(), Some(PROXMOX_TUN_FIX));
            assert_eq!(blocking(&report, &[module]), None);
        }
        assert!(problems_for(&report, "docker").is_empty());
    }

    #[test]
    fn a_plain_apply_checks_only_the_modules_the_settings_configure() {
        let settings = b"dns:\n  zone: example\nweb-proxy:\n  sites: []\nclock:\n  ntp: true\n";
        assert_eq!(modules_to_check(&[], Some(settings)), ["web-proxy", "dns"]);
        // modules named on the command line are checked as named, settings or not
        assert_eq!(modules_to_check(&["docker"], Some(settings)), ["docker"]);
        assert_eq!(modules_to_check(&["gateway"], None), ["gateway"]);
        // no settings, or none for docker, means nothing for docker to hold up
        assert!(modules_to_check(&[], None).is_empty());
        assert!(modules_to_check(&[], Some(b"clock:\n  ntp: true\n")).is_empty());
        assert!(modules_to_check(&[], Some(b"")).is_empty());
        assert!(modules_to_check(&[], Some(b"- not\n- a mapping\n")).is_empty());
        assert!(modules_to_check(&[], Some(b"\t: bad yaml [")).is_empty());
        // a key that only contains a module's name is not that module
        assert!(modules_to_check(&[], Some(b"docker-extra:\n  x: 1\n")).is_empty());
    }

    #[test]
    fn the_docker_playbook_runs_the_same_probe_and_gives_the_same_fix() {
        let playbooks =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ansible/playbooks");
        let playbook = std::fs::read_to_string(playbooks.join("docker.yml")).unwrap();
        // one probe: the playbook runs the script that lanlord embeds
        assert!(playbook.contains("doctor-probe.sh"));
        let parsed: serde_yaml_ng::Value = serde_yaml_ng::from_str(&playbook).unwrap();
        assert!(parsed.as_sequence().is_some_and(|plays| !plays.is_empty()));
        assert_eq!(
            std::fs::read_to_string(playbooks.join("doctor-probe.sh")).unwrap(),
            doctor::PROBE
        );
        // and gives the proxmox fix word for word
        assert!(playbook.contains("pct set <CTID> --features nesting=1,keyctl=1"));
        assert!(playbook.contains("pct reboot <CTID>"));
        assert!(PROXMOX_FEATURES_FIX.contains("nesting=1,keyctl=1"));
        // the playbook only stops on a definite no, as lanlord does
        assert!(playbook.contains("nesting=no"));
        assert!(playbook.contains("keyctl=no"));
        assert!(playbook.contains("virt_container=lxc"));
        assert!(playbook.contains("uid_map=0 0 4294967295"));
    }

    #[cfg(unix)]
    mod session {
        use super::*;

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

        const DNS: &[u8] = b"dns:\n  zone: example\n";

        #[test]
        fn the_gate_blocks_only_on_a_definite_no() {
            let blocked = server(
                "virt_container=lxc\nvirt_vm=none\nuid_map=0 100000 65536\nnesting=no\nkeyctl=yes\n",
            );
            let err = gate(&blocked, "tester", &["docker"], None, &mut no_password).unwrap_err();
            assert!(err.to_string().contains("nesting"), "{err}");
            // everything the settings configure is checked when no module is named
            assert!(gate(&blocked, "tester", &[], Some(DNS), &mut no_password).is_err());
            // but with no settings, or none for docker, nothing is held up
            assert!(gate(&blocked, "tester", &[], None, &mut no_password).is_ok());
            assert!(gate(
                &blocked,
                "tester",
                &[],
                Some(b"clock:\n  ntp: true\n"),
                &mut no_password
            )
            .is_ok());
            // a module that does not use docker is not checked
            assert!(gate(&blocked, "tester", &["gateway"], None, &mut no_password).is_ok());
            let unsure = server(
                "virt_container=lxc\nvirt_vm=none\nuid_map=0 100000 65536\nnesting=unknown\nkeyctl=unknown\n",
            );
            assert!(gate(&unsure, "tester", &["docker"], None, &mut no_password).is_ok());
            let fine = server(UNPRIVILEGED_LXC);
            assert!(gate(&fine, "tester", &["dns"], None, &mut no_password).is_ok());
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
            assert!(doctor::probe_as(&broken, "tester", &mut no_password).is_err());
            assert!(gate(&broken, "tester", &["docker"], None, &mut no_password).is_ok());
            note(&broken, &mut no_password);
        }

        #[test]
        fn a_note_after_bootstrap_does_not_stop_anything() {
            let blocked = server(
                "virt_container=lxc\nvirt_vm=none\nuid_map=0 100000 65536\nnesting=no\nkeyctl=no\n",
            );
            note(&blocked, &mut no_password);
            note(&server(UNPRIVILEGED_LXC), &mut no_password);
        }
    }
}
