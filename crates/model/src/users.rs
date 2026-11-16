//! the nt4-domain module's logins, kept in the `users` list of your settings.
//! names, admin flags and removals live in the plain settings; passwords are
//! secrets and live in the encrypted ones. changing a login pulls both, edits
//! them here, pushes them back and applies the module.

use anyhow::{anyhow, bail, Result};
use serde_yaml_ng::{Mapping, Value};

#[doc(hidden)]
pub const MODULE: &str = "nt4-domain";

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
