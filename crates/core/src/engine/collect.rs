//! what a module left on the machine, as one json document, so a run of the playbook and a run of
//! the engine can be compared (`tools/engine-parity.sh`). it holds no secret: a password is
//! reported only as whether LANlord's recorded fingerprint of it is the one in the settings.

use anyhow::Result;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::host::Host;
use super::modules::nt4;
use super::state::{hex, State};

const STATE: &str = "/opt/lanlord/nt4";

fn file(host: &dyn Host, path: &str) -> Result<Value> {
    Ok(match host.read(path)? {
        None => Value::Null,
        Some(bytes) => json!({
            "mode": format!("{:04o}", host.stat(path)?.map_or(0, |s| s.mode)),
            "sha256": hex(&Sha256::digest(&bytes)),
        }),
    })
}

/// what one server left on the machine.
fn server_state(host: &dyn Host, target: &nt4::Target, prints: &State) -> Result<Value> {
    let dir = &target.dir;
    let name = &target.container;
    let mut files = Map::new();
    for f in ["config/smb.conf", "lib/netlogon/lanlord.bat"] {
        files.insert(f.into(), file(host, &format!("{dir}/{f}"))?);
    }
    let mut dirs = Map::new();
    for d in ["", "/config", "/lib"] {
        dirs.insert(
            if d.is_empty() {
                "state".into()
            } else {
                d.to_string()
            },
            match host.stat(&format!("{dir}{d}"))? {
                Some(stat) => json!(format!("{:04o}", stat.mode)),
                None => Value::Null,
            },
        );
    }
    let inspect = host.run(
        &[
            "docker",
            "inspect",
            "--format",
            "{{.State.Running}} {{.Image}} {{index .Config.Labels \"lanlord.scripts\"}}",
            name,
        ],
        None,
    )?;
    let prev = host.run(
        &[
            "docker",
            "inspect",
            "--format",
            "{{.State.Running}}",
            &format!("{name}-prev"),
        ],
        None,
    )?;
    let listed = host.run(&["docker", "exec", name, "lanlord-user", "list"], None)?;
    let accounts: Vec<&str> = listed
        .stdout
        .lines()
        .filter_map(|line| line.split(':').next())
        .collect();
    let recorded: Map<String, Value> = target
        .passwords
        .iter()
        .map(|(key, secret)| (key.clone(), json!(prints.is_current(key, secret))))
        .collect();
    let rights = host.run(
        &[
            "docker",
            "exec",
            name,
            "net",
            "sam",
            "rights",
            "list",
            "SeMachineAccountPrivilege",
        ],
        None,
    )?;
    Ok(json!({
        "files": files,
        "dirs": dirs,
        "container": {
            "inspect": inspect.stdout.trim(),
            "previous_left_over": prev.success(),
        },
        "accounts": accounts,
        "password_recorded_current": recorded,
        "domain_admins_may_join": target.domain && rights.stdout.contains("Domain Admins"),
    }))
}

/// the nt4-domain module's state, per server of the list. `doc` is the merged settings and
/// `state_dir` the engine's, for the password comparison.
pub fn nt4_domain(host: &dyn Host, doc: &serde_yaml_ng::Value, state_dir: &str) -> Result<Value> {
    let mut servers = Map::new();
    if let Some(targets) = nt4::targets(host, doc)? {
        let prints = State::load(host, state_dir, false)?;
        for target in &targets {
            servers.insert(target.id.clone(), server_state(host, target, &prints)?);
        }
    }
    let mut shared = Map::new();
    for name in ["bin/entrypoint.sh", "bin/lanlord-user", "bin/lanlord-exec"] {
        shared.insert(name.into(), file(host, &format!("{STATE}/{name}"))?);
    }
    Ok(json!({
        "module": "nt4-domain",
        "shared": shared,
        "servers": servers,
    }))
}

/// the merged settings of this gateway, read as root.
#[cfg(unix)]
fn gateway_settings() -> Result<serde_yaml_ng::Value> {
    use crate::ssh::Session;
    crate::crypto::use_gateway_key(crate::crypto::GATEWAY_KEY.into())?;
    let identity = crate::crypto::load_default()?;
    let mut ask = |_: &str| -> Result<String> { anyhow::bail!("no sudo prompt: this is root") };
    let plain = crate::settings::merged_with(&Session::local(), identity.as_ref(), &mut ask)?;
    Ok(match plain {
        Some(bytes) => serde_yaml_ng::from_slice(&bytes)?,
        None => serde_yaml_ng::Value::Null,
    })
}

/// the state of `module` on this gateway, for the parity run: root only, as it opens the settings.
#[cfg(unix)]
pub fn state_of(module: &str) -> Result<Value> {
    let doc = gateway_settings()?;
    match module {
        "nt4-domain" => nt4_domain(&super::host::RealHost, &doc, super::STATE_DIR),
        other => anyhow::bail!("the engine has no state collector for {other}"),
    }
}

/// record the settings' passwords of `module` as the ones that are set, without setting them: a
/// first native apply then leaves a password that was changed from Windows alone. root only.
/// returns what was recorded, by name.
#[cfg(unix)]
pub fn record_of(module: &str) -> Result<Vec<String>> {
    let doc = gateway_settings()?;
    super::modules::record(module, &super::host::RealHost, &doc, super::STATE_DIR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::host::{FakeHost, Output};
    use crate::engine::Secret;

    const LIST: &str = "nt4-servers:\n  - {name: nt4-domain, role: pdc, domain: R, netbios_name: A, address: 192.0.2.1/24}\n  - {name: lab, role: workgroup, workgroup: W, netbios_name: B, address: 198.51.100.1/24}\nnt4-domain:\n  admin_password: s3cretadmin\n  users:\n    - name: dana\n      password: other\n";

    fn doc(text: &str) -> serde_yaml_ng::Value {
        serde_yaml_ng::from_str(text).unwrap()
    }

    #[test]
    fn each_server_reports_its_own_state_and_no_password() {
        let host = FakeHost::new();
        host.when(
            "docker exec lanlord-nt4 lanlord-user list",
            vec![Output::ok("root:0:Administrator\ndana:1000:\n")],
        );
        host.when(
            "docker exec lanlord-nt4-lab lanlord-user list",
            vec![Output::ok("dana:1000:\n")],
        );
        host.when(
            "docker exec lanlord-nt4 net sam rights list",
            vec![Output::ok("R\\Domain Admins\n")],
        );
        host.put("/opt/lanlord/nt4/lab/config/smb.conf", b"x", 0o640);
        let mut prints = State::load(&host, "/state", true).unwrap();
        prints
            .remember(&host, "nt4-domain/admin", &Secret::new("s3cretadmin"))
            .unwrap();
        prints
            .remember(&host, "nt4-domain/user:dana", &Secret::new("older"))
            .unwrap();
        let state = nt4_domain(&host, &doc(LIST), "/state").unwrap();
        let main = &state["servers"]["nt4-domain"];
        assert_eq!(main["password_recorded_current"]["nt4-domain/admin"], true);
        assert_eq!(
            main["password_recorded_current"]["nt4-domain/user:dana"],
            false
        );
        assert_eq!(main["domain_admins_may_join"], true);
        let lab = &state["servers"]["lab"];
        assert_eq!(lab["accounts"][0], "dana");
        assert_eq!(lab["domain_admins_may_join"], false);
        assert!(lab["password_recorded_current"].get("lab/admin").is_none());
        assert_eq!(lab["password_recorded_current"]["lab/user:dana"], false);
        assert!(lab["files"]["config/smb.conf"]["sha256"].is_string());
        assert_eq!(main["files"]["config/smb.conf"], Value::Null);
        let shown = state.to_string();
        assert!(!shown.contains("other") && !shown.contains("s3cretadmin"));
    }

    #[test]
    fn missing_state_and_no_list_say_nothing_is_there() {
        let host = FakeHost::new();
        let state = nt4_domain(&host, &doc(LIST), "/nowhere").unwrap();
        assert_eq!(
            state["servers"]["nt4-domain"]["password_recorded_current"]["nt4-domain/admin"],
            false
        );
        assert_eq!(
            state["servers"]["nt4-domain"]["domain_admins_may_join"],
            false
        );
        let none = nt4_domain(&host, &doc("dns: {}\n"), "/nowhere").unwrap();
        assert!(none["servers"].as_object().unwrap().is_empty());
    }
}
