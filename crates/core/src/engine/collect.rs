//! what a module left on the machine, as one json document, so a run of the playbook and a run of
//! the engine can be compared (`tools/engine-parity.sh`). it holds no secret: a password is
//! reported only as whether LANlord's recorded fingerprint of it is the one in the settings.

use anyhow::Result;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::host::Host;
use super::state::{hex, State};
use super::Secret;

const STATE: &str = "/opt/lanlord/nt4";
const NAME: &str = "lanlord-nt4";

fn file(host: &dyn Host, path: &str) -> Result<Value> {
    Ok(match host.read(path)? {
        None => Value::Null,
        Some(bytes) => json!({
            "mode": format!("{:04o}", host.stat(path)?.map_or(0, |s| s.mode)),
            "sha256": hex(&Sha256::digest(&bytes)),
        }),
    })
}

/// the nt4-domain module's state. `doc` is the merged settings and `state_dir` the engine's, for
/// the password comparison.
pub fn nt4_domain(host: &dyn Host, doc: &serde_yaml_ng::Value, state_dir: &str) -> Result<Value> {
    let mut files = Map::new();
    for name in [
        "config/smb.conf",
        "bin/entrypoint.sh",
        "bin/lanlord-user",
        "lib/netlogon/lanlord.bat",
    ] {
        files.insert(name.into(), file(host, &format!("{STATE}/{name}"))?);
    }
    let mut dirs = Map::new();
    for name in ["", "/config", "/bin", "/lib"] {
        let path = format!("{STATE}{name}");
        dirs.insert(
            if name.is_empty() {
                "state".into()
            } else {
                name.to_string()
            },
            match host.stat(&path)? {
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
            NAME,
        ],
        None,
    )?;
    let prev = host.run(
        &[
            "docker",
            "inspect",
            "--format",
            "{{.State.Running}}",
            &format!("{NAME}-prev"),
        ],
        None,
    )?;
    let listed = host.run(&["docker", "exec", NAME, "lanlord-user", "list"], None)?;
    let accounts: Vec<&str> = listed
        .stdout
        .lines()
        .filter_map(|line| line.split(':').next())
        .collect();
    let prints = State::load(host, state_dir, false)?;
    let section = doc.get("nt4-domain");
    let text = |v: Option<&serde_yaml_ng::Value>| match v {
        Some(serde_yaml_ng::Value::String(s)) => Some(s.clone()),
        Some(serde_yaml_ng::Value::Number(n)) => Some(n.to_string()),
        _ => None,
    };
    let mut recorded = Map::new();
    let mut check = |key: &str, password: Option<String>| {
        if let Some(password) = password {
            recorded.insert(
                key.to_string(),
                json!(prints.is_current(key, &Secret::new(password))),
            );
        }
    };
    check("admin", section.and_then(|s| text(s.get("admin_password"))));
    if let Some(users) = section
        .and_then(|s| s.get("users"))
        .and_then(|u| u.as_sequence())
    {
        for user in users {
            if let Some(name) = text(user.get("name")) {
                check(&format!("user:{name}"), text(user.get("password")));
            }
        }
    }
    let rights = host.run(
        &[
            "docker",
            "exec",
            NAME,
            "net",
            "sam",
            "rights",
            "list",
            "SeMachineAccountPrivilege",
        ],
        None,
    )?;
    Ok(json!({
        "module": "nt4-domain",
        "files": files,
        "dirs": dirs,
        "container": {
            "inspect": inspect.stdout.trim(),
            "previous_left_over": prev.success(),
        },
        "accounts": accounts,
        "password_recorded_current": recorded,
        "domain_admins_may_join": rights.stdout.contains("Domain Admins"),
    }))
}

/// the state of `module` on this gateway, for the parity run: root only, as it opens the settings.
#[cfg(unix)]
pub fn state_of(module: &str) -> Result<Value> {
    use crate::ssh::Session;
    crate::crypto::use_gateway_key(crate::crypto::GATEWAY_KEY.into())?;
    let identity = crate::crypto::load_default()?;
    let mut ask = |_: &str| -> Result<String> { anyhow::bail!("no sudo prompt: this is root") };
    let plain = crate::settings::merged_with(&Session::local(), identity.as_ref(), &mut ask)?;
    let doc: serde_yaml_ng::Value = match plain {
        Some(bytes) => serde_yaml_ng::from_slice(&bytes)?,
        None => serde_yaml_ng::Value::Null,
    };
    match module {
        "nt4-domain" => nt4_domain(&super::host::RealHost, &doc, super::STATE_DIR),
        other => anyhow::bail!("the engine has no state collector for {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::host::{FakeHost, Output};

    #[test]
    fn the_state_says_whether_a_password_is_the_recorded_one_and_never_holds_it() {
        let host = FakeHost::new();
        host.when(
            "docker exec lanlord-nt4 lanlord-user list",
            vec![Output::ok("root:0:Administrator\ndana:1000:\n")],
        );
        let mut prints = State::load(&host, "/state", true).unwrap();
        prints
            .remember(&host, "admin", &Secret::new("s3cretadmin"))
            .unwrap();
        prints
            .remember(&host, "user:dana", &Secret::new("older"))
            .unwrap();
        let doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            "nt4-domain:\n  admin_password: s3cretadmin\n  users:\n    - name: dana\n      password: other\n",
        )
        .unwrap();
        let state = nt4_domain(&host, &doc, "/state").unwrap();
        assert_eq!(state["password_recorded_current"]["admin"], true);
        assert_eq!(state["password_recorded_current"]["user:dana"], false);
        let shown = state.to_string();
        assert!(!shown.contains("other") && !shown.contains("s3cretadmin"));
    }

    #[test]
    fn missing_state_says_no_password_is_recorded() {
        let host = FakeHost::new();
        let doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            "nt4-domain:\n  admin_password: x\n  users:\n    - name: 7\n      password: 9\n",
        )
        .unwrap();
        let state = nt4_domain(&host, &doc, "/nowhere").unwrap();
        assert_eq!(state["password_recorded_current"]["admin"], false);
        assert_eq!(state["password_recorded_current"]["user:7"], false);
        assert_eq!(state["files"]["config/smb.conf"], Value::Null);
        assert_eq!(state["domain_admins_may_join"], false);
    }
}
