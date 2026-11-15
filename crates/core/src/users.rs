//! the nt4-domain module's logins, kept in the `users` list of your settings.
//! changing one pulls your settings, edits them here, pushes them back and
//! applies the module.

use anyhow::{anyhow, bail, Result};
use serde_yaml_ng::{Mapping, Value};

use crate::bootstrap::AskPassword;
use crate::ssh::Session;
use crate::{apply, config};

const MODULE: &str = "nt4-domain";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// create the user, or set the password of an existing one.
    SetPassword {
        name: String,
        password: String,
    },
    Remove {
        name: String,
    },
}

/// names the container accepts: lowercase, starting with a letter.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_.-".contains(c))
}

fn parse(settings: &[u8]) -> Result<Mapping> {
    match serde_yaml_ng::from_slice::<Value>(settings)? {
        Value::Mapping(map) => Ok(map),
        Value::Null => Ok(Mapping::new()),
        _ => bail!("your settings are not a yaml mapping"),
    }
}

fn module(root: &mut Mapping) -> Result<&mut Mapping> {
    root.get_mut(MODULE)
        .and_then(Value::as_mapping_mut)
        .ok_or_else(|| anyhow!("your settings have no {MODULE} section; add one first"))
}

fn users(module: &mut Mapping) -> Result<&mut Vec<Value>> {
    module
        .entry(Value::from("users"))
        .or_insert_with(|| Value::Sequence(Vec::new()))
        .as_sequence_mut()
        .ok_or_else(|| anyhow!("{MODULE}.users must be a list"))
}

fn name_of(entry: &Value) -> Option<&str> {
    entry.get("name").and_then(Value::as_str)
}

fn is_absent(entry: &Value) -> bool {
    entry.get("state").and_then(Value::as_str) == Some("absent")
}

/// the users that should exist.
pub fn list(settings: &[u8]) -> Result<Vec<User>> {
    let mut root = parse(settings)?;
    let module = module(&mut root)?;
    Ok(users(module)?
        .iter()
        .filter(|u| !is_absent(u))
        .filter_map(name_of)
        .map(|name| User { name: name.into() })
        .collect())
}

/// apply `change` to the settings text and return the new text.
pub fn edit(settings: &[u8], change: &Change) -> Result<Vec<u8>> {
    let name = match change {
        Change::SetPassword { name, password } => {
            if password.is_empty() {
                bail!("the password is empty");
            }
            name
        }
        Change::Remove { name } => name,
    };
    if !valid_name(name) {
        bail!("user names are lowercase letters, digits, _ . - and start with a letter: {name:?}");
    }
    let mut root = parse(settings)?;
    let list = users(module(&mut root)?)?;
    let mut entry = Mapping::new();
    entry.insert("name".into(), name.as_str().into());
    match change {
        Change::SetPassword { password, .. } => {
            entry.insert("password".into(), password.as_str().into());
        }
        Change::Remove { .. } => {
            entry.insert("state".into(), "absent".into());
        }
    }
    match list.iter_mut().find(|u| name_of(u) == Some(name)) {
        Some(existing) => *existing = Value::Mapping(entry),
        None => list.push(Value::Mapping(entry)),
    }
    Ok(serde_yaml_ng::to_string(&Value::Mapping(root))?.into_bytes())
}

/// remembers the first password it is given, so one change asks once.
pub(crate) fn cached<'a>(ask: AskPassword<'a>) -> impl FnMut(&str) -> Result<String> + 'a {
    let mut saved: Option<String> = None;
    move |what| {
        if saved.is_none() {
            saved = Some(ask(what)?);
        }
        Ok(saved.clone().unwrap_or_default())
    }
}

fn pull(
    session: &Session,
    host: &str,
    identity: &age::x25519::Identity,
    ask: AskPassword,
) -> Result<Vec<u8>> {
    config::pull_with(session, identity, ask)?
        .ok_or_else(|| anyhow!("no settings on {host}; run lanlord config push first"))
}

/// the users your settings define on `host`.
pub fn list_on(
    host: &str,
    identity: &age::x25519::Identity,
    ask_password: AskPassword,
) -> Result<Vec<User>> {
    list(&pull(&Session::new(host)?, host, identity, ask_password)?)
}

/// make the change in your settings on `host`, apply the module and return the users now defined.
pub fn change(
    host: &str,
    identity: &age::x25519::Identity,
    change: &Change,
    ask_password: AskPassword,
) -> Result<Vec<User>> {
    change_with(&Session::new(host)?, host, identity, change, ask_password)
}

fn change_with(
    session: &Session,
    host: &str,
    identity: &age::x25519::Identity,
    change: &Change,
    ask_password: AskPassword,
) -> Result<Vec<User>> {
    let mut ask = cached(ask_password);
    let updated = edit(&pull(session, host, identity, &mut ask)?, change)?;
    config::push_with(session, host, identity, &updated, &mut ask)?;
    apply::apply_with(
        session,
        host,
        Some(identity),
        &[MODULE.to_string()],
        &mut ask,
    )?;
    list(&updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTINGS: &str = "docker:\n  internal_address: 172.16.120.2/22\nnt4-domain:\n  mode: domain\n  name: RETRONET\n  users:\n    - name: james\n      password: one\n    - name: gone\n      state: absent\n";

    fn names(settings: &[u8]) -> Vec<String> {
        list(settings)
            .unwrap()
            .into_iter()
            .map(|u| u.name)
            .collect()
    }

    #[test]
    fn list_hides_users_marked_absent() {
        assert_eq!(names(SETTINGS.as_bytes()), ["james"]);
    }

    #[test]
    fn list_without_a_users_key_is_empty() {
        let settings = b"nt4-domain:\n  mode: workgroup\n  name: WG\n";
        assert!(names(settings).is_empty());
    }

    #[test]
    fn adding_a_user_keeps_everything_else() {
        let change = Change::SetPassword {
            name: "olive".into(),
            password: "pw".into(),
        };
        let out = edit(SETTINGS.as_bytes(), &change).unwrap();
        assert_eq!(names(&out), ["james", "olive"]);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("internal_address: 172.16.120.2/22"));
        assert!(text.contains("name: RETRONET"));
    }

    #[test]
    fn setting_a_password_replaces_the_old_one_and_revives_absent_users() {
        let out = edit(
            SETTINGS.as_bytes(),
            &Change::SetPassword {
                name: "james".into(),
                password: "two".into(),
            },
        )
        .unwrap();
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(text.contains("password: two") && !text.contains("password: one"));
        let out = edit(
            &out,
            &Change::SetPassword {
                name: "gone".into(),
                password: "back".into(),
            },
        )
        .unwrap();
        assert_eq!(names(&out), ["james", "gone"]);
    }

    #[test]
    fn removing_marks_the_user_absent_and_drops_the_password() {
        let out = edit(
            SETTINGS.as_bytes(),
            &Change::Remove {
                name: "james".into(),
            },
        )
        .unwrap();
        assert!(names(&out).is_empty());
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains("password: one"));
        assert_eq!(text.matches("state: absent").count(), 2);
    }

    #[test]
    fn removing_an_unlisted_user_still_records_the_removal() {
        let out = edit(
            SETTINGS.as_bytes(),
            &Change::Remove {
                name: "stranger".into(),
            },
        )
        .unwrap();
        assert!(String::from_utf8(out).unwrap().contains("stranger"));
    }

    #[test]
    fn bad_input_is_refused() {
        for name in ["", "Bob", "1abc", "a b", "x;y"] {
            let change = Change::Remove { name: name.into() };
            assert!(edit(SETTINGS.as_bytes(), &change).is_err(), "{name}");
        }
        let empty = Change::SetPassword {
            name: "bob".into(),
            password: String::new(),
        };
        assert!(edit(SETTINGS.as_bytes(), &empty).is_err());
    }

    #[test]
    fn settings_without_the_module_explain_what_to_add() {
        let err = list(b"docker:\n  a: 1\n").unwrap_err().to_string();
        assert!(err.contains("no nt4-domain section"), "{err}");
        assert!(list(b"- just\n- a list\n").is_err());
        assert!(list(b"").is_err(), "empty settings have no module either");
    }

    #[test]
    fn the_password_is_asked_for_once() {
        let mut calls = 0;
        let mut ask = |_: &str| {
            calls += 1;
            Ok("pw".to_string())
        };
        let mut once = cached(&mut ask);
        assert_eq!(once("a").unwrap(), "pw");
        assert_eq!(once("b").unwrap(), "pw");
        drop(once);
        assert_eq!(calls, 1);
    }

    #[cfg(unix)]
    mod gateway {
        use super::*;
        use crate::crypto;
        use base64::Engine;

        const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

        fn identity(name: &str) -> age::x25519::Identity {
            let path = std::env::temp_dir()
                .join(format!("lanlord-users-{}-{name}", std::process::id()))
                .join("identity");
            crypto::generate(&path).unwrap();
            let identity = crypto::load(&path).unwrap();
            let _ = std::fs::remove_dir_all(path.parent().unwrap());
            identity
        }

        /// a fake gateway that serves `served` as the stored settings and
        /// records the push and apply scripts it receives.
        fn gateway(served: &str, dir: &std::path::Path) -> Session {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("served.b64"), served).unwrap();
            let d = dir.display();
            Session::fake(&format!(
                r#"for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sudo -n true") exit 0 ;;
  *)
    input=$(cat)
    case "$input" in
      *"base64 < /opt/lanlord/config/settings.age"*) cat '{d}/served.b64' ;;
      *"tee /opt/lanlord/config/settings.age"*) printf '%s' "$input" > '{d}/push.rec' ;;
      *) printf '%s' "$input" > '{d}/apply.rec' ;;
    esac ;;
esac"#
            ))
        }

        fn no_password(_: &str) -> Result<String> {
            bail!("no password expected")
        }

        fn script_payload(script: &str, marker: &str) -> Vec<u8> {
            let body: String = script
                .lines()
                .skip_while(|l| !l.contains(marker))
                .skip(1)
                .take_while(|l| *l != "LANLORD_PAYLOAD")
                .collect();
            B64.decode(body).unwrap()
        }

        #[test]
        fn a_change_pulls_edits_pushes_and_applies_the_module() {
            let id = identity("change");
            let sealed = crypto::encrypt(&id, SETTINGS.as_bytes()).unwrap();
            let dir = std::env::temp_dir().join(format!("lanlord-users-gw-{}", std::process::id()));
            let session = gateway(&B64.encode(sealed), &dir);
            let change = Change::SetPassword {
                name: "olive".into(),
                password: "pw".into(),
            };
            let now = change_with(&session, "gw", &id, &change, &mut no_password).unwrap();
            assert_eq!(
                now.iter().map(|u| u.name.as_str()).collect::<Vec<_>>(),
                ["james", "olive"]
            );

            let push = std::fs::read_to_string(dir.join("push.rec")).unwrap();
            let stored =
                crypto::decrypt(&id, &script_payload(&push, "<<'LANLORD_PAYLOAD'")).unwrap();
            assert_eq!(names(&stored), ["james", "olive"]);

            let apply = std::fs::read_to_string(dir.join("apply.rec")).unwrap();
            assert!(apply.contains("--tags nt4-domain"));
            assert!(apply.contains("-e @/run/lanlord/settings.yml"));
            let _ = std::fs::remove_dir_all(dir);
        }

        #[test]
        fn list_on_reads_the_gateways_settings() {
            let id = identity("list");
            let sealed = crypto::encrypt(&id, SETTINGS.as_bytes()).unwrap();
            let dir = std::env::temp_dir().join(format!("lanlord-users-ls-{}", std::process::id()));
            let session = gateway(&B64.encode(sealed), &dir);
            let settings = pull(&session, "gw", &id, &mut no_password).unwrap();
            assert_eq!(names(&settings), ["james"]);
            let _ = std::fs::remove_dir_all(dir);
        }

        #[test]
        fn changing_users_needs_settings_on_the_gateway() {
            let id = identity("none");
            let dir =
                std::env::temp_dir().join(format!("lanlord-users-none-{}", std::process::id()));
            let session = gateway("", &dir);
            let change = Change::Remove {
                name: "james".into(),
            };
            let err = change_with(&session, "gw", &id, &change, &mut no_password)
                .unwrap_err()
                .to_string();
            assert!(err.contains("no settings on gw"), "{err}");
            assert!(!dir.join("push.rec").exists());
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
