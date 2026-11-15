//! the nt4-domain module's logins, kept in the `users` list of your settings.
//! names, admin flags and removals live in the plain settings; passwords are
//! secrets and live in the encrypted ones. changing a login pulls both, edits
//! them here, pushes them back and applies the module.

use anyhow::{anyhow, bail, Result};
use serde_yaml_ng::{Mapping, Value};

use crate::apply;
use crate::bootstrap::AskPassword;
use crate::settings::{self, Document};
use crate::ssh::Session;

const MODULE: &str = "nt4-domain";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub name: String,
    /// a member of Domain Admins.
    pub admin: bool,
    /// may write to the software share without being a domain admin.
    pub software_admin: bool,
}

impl User {
    /// the roles this user has, for listings.
    pub fn roles(&self) -> Vec<&'static str> {
        let mut roles = Vec::new();
        if self.admin {
            roles.push("domain admin");
        }
        if self.software_admin {
            roles.push("software admin");
        }
        roles
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// create the user, or set the password of an existing one.
    SetPassword {
        name: String,
        password: String,
    },
    /// make an existing user a domain admin, or stop being one.
    SetAdmin {
        name: String,
        admin: bool,
    },
    /// let a user write to the software share without being a domain admin, or stop.
    SetSoftwareAdmin {
        name: String,
        software_admin: bool,
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
        .filter_map(|u| {
            Some(User {
                name: name_of(u)?.into(),
                admin: u.get("admin").and_then(Value::as_bool) == Some(true),
                software_admin: u.get("software_admin").and_then(Value::as_bool) == Some(true),
            })
        })
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
        Change::SetAdmin { name, .. }
        | Change::SetSoftwareAdmin { name, .. }
        | Change::Remove { name } => name,
    };
    if !valid_name(name) {
        bail!("user names are lowercase letters, digits, _ . - and start with a letter: {name:?}");
    }
    let mut root = parse(settings)?;
    let list = users(module(&mut root)?)?;
    let existing = list.iter_mut().find(|u| name_of(u) == Some(name));
    match (change, existing) {
        (Change::SetPassword { password, .. }, Some(entry)) => {
            // keep everything else about the user, such as the admin flag
            let map = entry
                .as_mapping_mut()
                .ok_or_else(|| anyhow!("{name} is not a mapping"))?;
            map.insert("password".into(), password.as_str().into());
            map.remove("state");
        }
        (Change::SetPassword { password, .. }, None) => {
            let mut entry = Mapping::new();
            entry.insert("name".into(), name.as_str().into());
            entry.insert("password".into(), password.as_str().into());
            list.push(Value::Mapping(entry));
        }
        (Change::SetAdmin { admin, .. }, Some(entry)) => {
            let map = entry
                .as_mapping_mut()
                .ok_or_else(|| anyhow!("{name} is not a mapping"))?;
            if *admin {
                map.insert("admin".into(), true.into());
            } else {
                map.remove("admin");
            }
        }
        (Change::SetSoftwareAdmin { software_admin, .. }, Some(entry)) => {
            let map = entry
                .as_mapping_mut()
                .ok_or_else(|| anyhow!("{name} is not a mapping"))?;
            if *software_admin {
                map.insert("software_admin".into(), true.into());
            } else {
                map.remove("software_admin");
            }
        }
        (Change::SetAdmin { .. } | Change::SetSoftwareAdmin { .. }, None) => {
            bail!("{name} is not in your settings; add the user first")
        }
        (Change::Remove { .. }, existing) => {
            let mut entry = Mapping::new();
            entry.insert("name".into(), name.as_str().into());
            entry.insert("state".into(), "absent".into());
            match existing {
                Some(slot) => *slot = Value::Mapping(entry),
                None => list.push(Value::Mapping(entry)),
            }
        }
    }
    Ok(serde_yaml_ng::to_string(&Value::Mapping(root))?.into_bytes())
}

/// where a user's password lives among the secrets.
pub fn secret_path(name: &str) -> String {
    format!("{MODULE}.users[{name}].password")
}

/// the result of `edit_split`.
#[derive(Debug, PartialEq, Eq)]
pub struct Edited {
    /// the new plain settings: no passwords in them.
    pub plain: String,
    /// secrets to set, as (path, password).
    pub secrets: Vec<(String, String)>,
    /// paths of secrets to forget, for users that were removed.
    pub removed: Vec<String>,
}

fn strip_password(plain: &[u8], name: &str) -> Result<Vec<u8>> {
    let mut root = parse(plain)?;
    for entry in users(module(&mut root)?)? {
        if name_of(entry) == Some(name) {
            if let Some(map) = entry.as_mapping_mut() {
                map.remove("password");
            }
        }
    }
    Ok(serde_yaml_ng::to_string(&Value::Mapping(root))?.into_bytes())
}

/// apply `changes` to the plain settings: what stays plain comes back as
/// text, the passwords come back as secrets to store.
pub fn edit_split(plain: &[u8], changes: &[Change]) -> Result<Edited> {
    let mut text = plain.to_vec();
    let mut secrets: Vec<(String, String)> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    for change in changes {
        text = edit(&text, change)?;
        match change {
            Change::SetPassword { name, password } => {
                text = strip_password(&text, name)?;
                let path = secret_path(name);
                removed.retain(|p| *p != path);
                secrets.retain(|(p, _)| *p != path);
                secrets.push((path, password.clone()));
            }
            Change::Remove { name } => {
                let path = secret_path(name);
                secrets.retain(|(p, _)| *p != path);
                removed.push(path);
            }
            Change::SetAdmin { .. } | Change::SetSoftwareAdmin { .. } => {}
        }
    }
    Ok(Edited {
        plain: String::from_utf8(text)?,
        secrets,
        removed,
    })
}

/// remembers the first password it is given, so one change asks once.
pub fn cached<'a>(ask: AskPassword<'a>) -> impl FnMut(&str) -> Result<String> + 'a {
    let mut saved: Option<String> = None;
    move |what| {
        if saved.is_none() {
            saved = Some(ask(what)?);
        }
        Ok(saved.clone().unwrap_or_default())
    }
}

fn document(
    session: &Session,
    host: &str,
    identity: &age::x25519::Identity,
    ask: AskPassword,
) -> Result<Document> {
    let document = settings::load_document_with(session, identity, ask)?;
    if document == Document::default() {
        bail!("no settings on {host}; run lanlord config push first");
    }
    Ok(document)
}

/// the users your settings define on `host`.
pub fn list_on(
    host: &str,
    identity: &age::x25519::Identity,
    ask_password: AskPassword,
) -> Result<Vec<User>> {
    let session = Session::new(host)?;
    list(
        document(&session, host, identity, ask_password)?
            .plain
            .as_bytes(),
    )
}

/// make the change in your settings on `host`, apply the module and return the users now defined.
pub fn change(
    host: &str,
    identity: &age::x25519::Identity,
    change: &Change,
    ask_password: AskPassword,
) -> Result<Vec<User>> {
    change_all(host, identity, std::slice::from_ref(change), ask_password)
}

/// make several changes in one go (one pull, one push, one apply).
pub fn change_all(
    host: &str,
    identity: &age::x25519::Identity,
    changes: &[Change],
    ask_password: AskPassword,
) -> Result<Vec<User>> {
    change_with(&Session::new(host)?, host, identity, changes, ask_password)
}

fn change_with(
    session: &Session,
    host: &str,
    identity: &age::x25519::Identity,
    changes: &[Change],
    ask_password: AskPassword,
) -> Result<Vec<User>> {
    let mut ask = cached(ask_password);
    let stored = document(session, host, identity, &mut ask)?;
    let edited = edit_split(stored.plain.as_bytes(), changes)?;
    settings::save_changes_with(
        session,
        host,
        identity,
        &edited.plain,
        &edited.secrets,
        &edited.removed,
        &mut ask,
    )?;
    apply::apply_with(
        session,
        host,
        Some(identity),
        &[MODULE.to_string()],
        &mut ask,
    )?;
    list(edited.plain.as_bytes())
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

    #[test]
    fn changing_a_password_keeps_the_admin_flag() {
        let settings =
            "nt4-domain:\n  users:\n    - name: james\n      password: one\n      admin: true\n";
        let out = edit(
            settings.as_bytes(),
            &Change::SetPassword {
                name: "james".into(),
                password: "two".into(),
            },
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("password: two") && text.contains("admin: true"),
            "{text}"
        );
    }

    #[test]
    fn admin_can_be_granted_and_revoked() {
        let granted = edit(
            SETTINGS.as_bytes(),
            &Change::SetAdmin {
                name: "james".into(),
                admin: true,
            },
        )
        .unwrap();
        assert!(String::from_utf8(granted.clone())
            .unwrap()
            .contains("admin: true"));
        let revoked = edit(
            &granted,
            &Change::SetAdmin {
                name: "james".into(),
                admin: false,
            },
        )
        .unwrap();
        assert!(!String::from_utf8(revoked).unwrap().contains("admin"));
    }

    #[test]
    fn software_admin_can_be_granted_and_revoked_and_shows_in_the_list() {
        let granted = edit(
            SETTINGS.as_bytes(),
            &Change::SetSoftwareAdmin {
                name: "james".into(),
                software_admin: true,
            },
        )
        .unwrap();
        assert!(String::from_utf8(granted.clone())
            .unwrap()
            .contains("software_admin: true"));
        let james = list(&granted).unwrap().remove(0);
        assert!(james.software_admin && !james.admin);
        assert_eq!(james.roles(), ["software admin"]);
        let revoked = edit(
            &granted,
            &Change::SetSoftwareAdmin {
                name: "james".into(),
                software_admin: false,
            },
        )
        .unwrap();
        assert!(!String::from_utf8(revoked.clone())
            .unwrap()
            .contains("software_admin"));
        assert!(list(&revoked).unwrap()[0].roles().is_empty());
        let both = User {
            name: "x".into(),
            admin: true,
            software_admin: true,
        };
        assert_eq!(both.roles(), ["domain admin", "software admin"]);
    }

    #[test]
    fn software_admin_needs_a_listed_user_and_stays_plain() {
        let err = edit(
            SETTINGS.as_bytes(),
            &Change::SetSoftwareAdmin {
                name: "nobody".into(),
                software_admin: true,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("not in your settings"), "{err}");
        let edited = edit_split(
            SETTINGS.as_bytes(),
            &[Change::SetSoftwareAdmin {
                name: "james".into(),
                software_admin: true,
            }],
        )
        .unwrap();
        assert!(edited.plain.contains("software_admin: true"));
        assert!(edited.secrets.is_empty() && edited.removed.is_empty());
    }

    #[test]
    fn only_listed_users_can_be_made_admin() {
        let err = edit(
            SETTINGS.as_bytes(),
            &Change::SetAdmin {
                name: "nobody".into(),
                admin: true,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("not in your settings"), "{err}");
    }

    #[test]
    fn a_user_entry_that_is_not_a_mapping_is_reported() {
        let settings = b"nt4-domain:\n  users:\n    - james\n";
        // a bare name is not a user entry, so it is never matched; a mapping without a name is skipped
        let out = edit(
            settings,
            &Change::SetPassword {
                name: "olive".into(),
                password: "pw".into(),
            },
        )
        .unwrap();
        assert_eq!(names(&out), ["olive"]);
        let scalar = b"nt4-domain:\n  users:\n    - name: james\n";
        let granted = edit(
            scalar,
            &Change::SetAdmin {
                name: "james".into(),
                admin: true,
            },
        )
        .unwrap();
        assert!(String::from_utf8(granted).unwrap().contains("admin: true"));
    }

    #[test]
    fn revoking_admin_from_a_plain_user_changes_nothing_else() {
        let out = edit(
            SETTINGS.as_bytes(),
            &Change::SetAdmin {
                name: "james".into(),
                admin: false,
            },
        )
        .unwrap();
        assert_eq!(names(&out), ["james"]);
        assert!(String::from_utf8(out).unwrap().contains("password: one"));
    }

    #[test]
    fn setting_a_password_for_an_absent_user_brings_them_back_with_their_admin_flag() {
        let settings =
            b"nt4-domain:\n  users:\n    - name: gone\n      state: absent\n      admin: true\n";
        let out = edit(
            settings,
            &Change::SetPassword {
                name: "gone".into(),
                password: "back".into(),
            },
        )
        .unwrap();
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(
            !text.contains("absent") && text.contains("admin: true"),
            "{text}"
        );
        assert_eq!(names(&out), ["gone"]);
    }

    #[test]
    fn edit_split_keeps_passwords_out_of_the_plain_text() {
        let edited = edit_split(
            SETTINGS.as_bytes(),
            &[
                Change::SetPassword {
                    name: "olive".into(),
                    password: "pw".into(),
                },
                Change::SetAdmin {
                    name: "olive".into(),
                    admin: true,
                },
                Change::Remove {
                    name: "james".into(),
                },
            ],
        )
        .unwrap();
        assert!(!edited.plain.contains("pw") && !edited.plain.contains("password"));
        assert!(edited.plain.contains("admin: true"));
        assert_eq!(
            edited.secrets,
            [(
                "nt4-domain.users[olive].password".to_string(),
                "pw".to_string()
            )]
        );
        assert_eq!(edited.removed, ["nt4-domain.users[james].password"]);
        let users = list(edited.plain.as_bytes()).unwrap();
        assert_eq!(
            users,
            [User {
                name: "olive".into(),
                admin: true,
                software_admin: false,
            }]
        );
    }

    #[test]
    fn the_last_change_to_a_user_wins() {
        let set = |name: &str| Change::SetPassword {
            name: name.into(),
            password: "new".into(),
        };
        let remove = |name: &str| Change::Remove { name: name.into() };
        let edited = edit_split(SETTINGS.as_bytes(), &[set("james"), remove("james")]).unwrap();
        assert!(edited.secrets.is_empty() && edited.removed.len() == 1);
        let edited = edit_split(SETTINGS.as_bytes(), &[remove("james"), set("james")]).unwrap();
        assert!(edited.removed.is_empty() && edited.secrets.len() == 1);
        assert!(edit_split(b"docker: {}\n", &[set("james")]).is_err());
    }

    #[cfg(unix)]
    mod gateway {
        use super::*;
        use crate::testgw::{identity, no_password, Gateway};

        const LEGACY: &str = "docker:\n  internal_address: 172.16.120.2/22\nnt4-domain:\n  mode: domain\n  name: RETRONET\n  users:\n    - name: james\n      password: one\n      admin: true\n";

        #[test]
        fn a_change_stores_names_plain_and_passwords_encrypted() {
            let (gw, id) = (Gateway::new("users-change"), identity("users-change"));
            gw.seed_sealed(&id, LEGACY);
            let change = Change::SetPassword {
                name: "olive".into(),
                password: "pw".into(),
            };
            let now = change_with(
                &gw.session,
                "gw",
                &id,
                std::slice::from_ref(&change),
                &mut no_password,
            )
            .unwrap();
            let names: Vec<_> = now.iter().map(|u| (u.name.as_str(), u.admin)).collect();
            assert_eq!(names, [("james", true), ("olive", false)]);

            let plain = gw.plain().unwrap();
            assert!(!plain.contains("one") && !plain.contains("pw"), "{plain}");
            assert!(plain.contains("olive") && plain.contains("admin: true"));
            let sealed = gw.sealed(&id).unwrap();
            assert!(sealed.contains("one") && sealed.contains("pw"), "{sealed}");
            assert!(!sealed.contains("RETRONET"), "{sealed}");
        }

        #[test]
        fn software_admin_is_stored_plain_and_applied() {
            let (gw, id) = (Gateway::new("users-software"), identity("users-software"));
            gw.seed_sealed(&id, LEGACY);
            let change = Change::SetSoftwareAdmin {
                name: "james".into(),
                software_admin: true,
            };
            let now = change_with(&gw.session, "gw", &id, &[change], &mut no_password).unwrap();
            assert!(now[0].software_admin);
            assert!(gw.plain().unwrap().contains("software_admin: true"));
            assert!(gw.sealed(&id).unwrap().contains("one"), "password kept");
        }

        #[test]
        fn removing_a_user_forgets_the_password_and_admin_changes_touch_only_plain() {
            let (gw, id) = (Gateway::new("users-remove"), identity("users-remove"));
            gw.seed_sealed(&id, LEGACY);
            let admin = Change::SetAdmin {
                name: "james".into(),
                admin: false,
            };
            change_with(&gw.session, "gw", &id, &[admin], &mut no_password).unwrap();
            assert!(gw.sealed(&id).unwrap().contains("one"), "kept");
            let remove = Change::Remove {
                name: "james".into(),
            };
            change_with(&gw.session, "gw", &id, &[remove], &mut no_password).unwrap();
            assert!(!gw.sealed(&id).unwrap().contains("one"));
            assert!(gw.plain().unwrap().contains("state: absent"));
        }

        #[test]
        fn the_users_come_from_the_plain_settings() {
            let (gw, id) = (Gateway::new("users-list"), identity("users-list"));
            gw.seed_sealed(&id, LEGACY);
            let doc = document(&gw.session, "gw", &id, &mut no_password).unwrap();
            assert_eq!(names(doc.plain.as_bytes()), ["james"]);
        }

        #[test]
        fn changing_users_needs_settings_on_the_gateway() {
            let (gw, id) = (Gateway::new("users-none"), identity("users-none"));
            let change = Change::Remove {
                name: "james".into(),
            };
            let err = change_with(
                &gw.session,
                "gw",
                &id,
                std::slice::from_ref(&change),
                &mut no_password,
            )
            .unwrap_err()
            .to_string();
            assert!(err.contains("no settings on gw"), "{err}");
            assert!(gw.plain().is_none() && gw.sealed(&id).is_none());
        }
    }
}
