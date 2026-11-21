//! what a module left on the machine, as one json document, so a run of the playbook and a run of
//! the engine can be compared (`tools/engine-parity.sh`). it holds no secret: a password is
//! reported only as whether it matches the settings.

use anyhow::Result;
use md4::Md4;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::host::Host;
use super::state::hex;

const STATE: &str = "/opt/lanlord/nt4";
const NAME: &str = "lanlord-nt4";

/// the NT hash of a password, as `pdbedit -w` shows it.
fn nt_hash(password: &str) -> String {
    let utf16: Vec<u8> = password.encode_utf16().flat_map(u16::to_le_bytes).collect();
    hex(&Md4::digest(&utf16)).to_ascii_uppercase()
}

fn file(host: &dyn Host, path: &str) -> Result<Value> {
    Ok(match host.read(path)? {
        None => Value::Null,
        Some(bytes) => json!({
            "mode": format!("{:04o}", host.stat(path)?.map_or(0, |s| s.mode)),
            "sha256": hex(&Sha256::digest(&bytes)),
        }),
    })
}

/// the nt4-domain module's state. `doc` is the merged settings, for the password comparison.
pub fn nt4_domain(host: &dyn Host, doc: &serde_yaml_ng::Value) -> Result<Value> {
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
    // names and password hashes: `name:uid:LM:NT:[flags]:...`
    let listed = host.run(&["docker", "exec", NAME, "pdbedit", "-L", "-w"], None)?;
    let mut hashes = std::collections::BTreeMap::new();
    for line in listed.stdout.lines() {
        let mut fields = line.split(':');
        if let (Some(name), Some(_), Some(_), Some(nt)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        {
            hashes.insert(name.to_string(), nt.to_string());
        }
    }
    let section = doc.get("nt4-domain");
    let mut matches = Map::new();
    let mut check = |account: &str, password: Option<String>| {
        if let Some(password) = password {
            matches.insert(
                account.to_string(),
                json!(hashes
                    .get(account)
                    .is_some_and(|h| h.eq_ignore_ascii_case(&nt_hash(&password)))),
            );
        }
    };
    let text = |v: Option<&serde_yaml_ng::Value>| match v {
        Some(serde_yaml_ng::Value::String(s)) => Some(s.clone()),
        Some(serde_yaml_ng::Value::Number(n)) => Some(n.to_string()),
        _ => None,
    };
    check("root", section.and_then(|s| text(s.get("admin_password"))));
    if let Some(users) = section
        .and_then(|s| s.get("users"))
        .and_then(|u| u.as_sequence())
    {
        for user in users {
            if let Some(name) = text(user.get("name")) {
                check(&name, text(user.get("password")));
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
        "accounts": hashes.keys().collect::<Vec<_>>(),
        "password_matches_settings": matches,
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
        "nt4-domain" => nt4_domain(&super::host::RealHost, &doc),
        other => anyhow::bail!("the engine has no state collector for {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::host::{FakeHost, Output};

    #[test]
    fn the_nt_hash_is_the_md4_of_the_utf16_password() {
        // the well-known NT hash of the empty password and of "password"
        assert_eq!(nt_hash(""), "31D6CFE0D16AE931B73C59D7E0C089C0");
        assert_eq!(nt_hash("password"), "8846F7EAEE8FB117AD06BDD830B7586C");
    }

    #[test]
    fn the_state_says_whether_a_password_matches_and_never_holds_it() {
        let host = FakeHost::new();
        host.when(
            "docker exec lanlord-nt4 pdbedit -L -w",
            vec![Output::ok(
                "root:0:XXX:8846F7EAEE8FB117AD06BDD830B7586C:[U          ]:LCT-1:\ndana:1000:XXX:31D6CFE0D16AE931B73C59D7E0C089C0:[U          ]:LCT-1:\n",
            )],
        );
        let doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            "nt4-domain:\n  admin_password: password\n  users:\n    - name: dana\n      password: other\n",
        )
        .unwrap();
        let state = nt4_domain(&host, &doc).unwrap();
        assert_eq!(state["password_matches_settings"]["root"], true);
        assert_eq!(state["password_matches_settings"]["dana"], false);
        let shown = state.to_string();
        assert!(!shown.contains("other"));
        assert!(!shown.contains("8846F7EA"));
    }
}
