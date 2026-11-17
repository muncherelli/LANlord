//! the nt4-domain settings a front end edits: the domain, its server name,
//! the machines to prepare accounts for and roaming profiles. logins and
//! secrets are handled elsewhere; keys this type does not know stay as they are.

use anyhow::{bail, Result};
use serde_yaml_ng::{Mapping, Value};

use crate::settings::{Settings, ValidationError};

const SECTION: &str = "nt4-domain";

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Nt4Settings {
    /// `domain` or `workgroup`.
    pub mode: String,
    pub name: String,
    /// the server's own name; empty means the machine's hostname.
    pub netbios_name: String,
    pub roaming_profiles: bool,
    /// a software share that maps to S: at logon.
    pub software: bool,
    pub machines: Vec<String>,
}

impl Default for Nt4Settings {
    fn default() -> Self {
        Self {
            mode: "domain".into(),
            name: "RETRONET".into(),
            netbios_name: String::new(),
            roaming_profiles: false,
            software: false,
            machines: Vec::new(),
        }
    }
}

fn text(map: &Mapping, key: &str) -> Option<String> {
    match map.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// `^[A-Z0-9][A-Z0-9_-]{0,14}$`, as the playbook checks the upper-cased name.
fn netbios_ok(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    let mut chars = name.chars();
    name.len() <= 15
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// `^[A-Za-z][A-Za-z0-9-]{0,14}$`
pub fn valid_machine(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 15
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-')
}

impl Nt4Settings {
    /// read the `nt4-domain` section; a missing one gives the defaults.
    pub fn from_settings(settings: &Settings) -> Result<Self> {
        let map = match settings.section(SECTION) {
            None | Some(Value::Null) => return Ok(Self::default()),
            Some(Value::Mapping(map)) => map,
            Some(_) => bail!("{SECTION} must be a mapping"),
        };
        let defaults = Self::default();
        let machines = match map.get("machines") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Sequence(items)) => items
                .iter()
                .filter_map(|m| match m {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                })
                .collect(),
            Some(_) => bail!("{SECTION}.machines must be a list"),
        };
        Ok(Self {
            mode: text(map, "mode").unwrap_or(defaults.mode),
            name: text(map, "name").unwrap_or(defaults.name),
            netbios_name: text(map, "netbios_name").unwrap_or_default(),
            roaming_profiles: map.get("roaming_profiles").and_then(Value::as_bool) == Some(true),
            software: map.get("software").and_then(Value::as_bool) == Some(true),
            machines,
        })
    }

    /// write this into the `nt4-domain` section of `settings`; logins and
    /// every other key stay as they are.
    pub fn apply_to(&self, settings: &mut Settings) {
        let mut map = match settings.section(SECTION) {
            Some(Value::Mapping(map)) => map.clone(),
            _ => Mapping::new(),
        };
        map.insert("mode".into(), self.mode.as_str().into());
        map.insert("name".into(), self.name.as_str().into());
        if self.netbios_name.is_empty() {
            map.remove("netbios_name");
        } else {
            map.insert("netbios_name".into(), self.netbios_name.as_str().into());
        }
        map.insert("roaming_profiles".into(), self.roaming_profiles.into());
        map.insert("software".into(), self.software.into());
        let machines = self.machines.iter().map(|m| m.as_str().into()).collect();
        map.insert("machines".into(), Value::Sequence(machines));
        settings.set_section(SECTION, Value::Mapping(map));
    }

    /// everything wrong with these settings; a domain also needs its admin
    /// password, which `admin_password_set` says is stored.
    pub fn validate(&self, admin_password_set: bool) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        let mut fail = |path: &str, message: &str| {
            errors.push(ValidationError {
                path: path.into(),
                message: message.into(),
            });
        };
        if !matches!(self.mode.as_str(), "domain" | "workgroup") {
            fail("nt4-domain.mode", "must be domain or workgroup");
        }
        const NAME: &str = "up to 15 letters, digits, - or _, starting with a letter or digit";
        if !netbios_ok(&self.name) {
            fail("nt4-domain.name", NAME);
        }
        if !self.netbios_name.is_empty() && !netbios_ok(&self.netbios_name) {
            fail("nt4-domain.netbios_name", NAME);
        }
        for (i, machine) in self.machines.iter().enumerate() {
            if !valid_machine(machine) {
                fail(
                    &format!("nt4-domain.machines[{i}]"),
                    "a letter, then up to 14 letters, digits or dashes",
                );
            }
        }
        if self.mode == "domain" && !admin_password_set {
            fail(
                "nt4-domain.admin_password",
                "a domain needs an administrator password",
            );
        }
        errors
    }
}

/// what to do with a machine account.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MachineChange {
    Add(String),
    Remove(String),
    /// set the account back to its first password so the machine can join again.
    Reset(String),
}

/// the machine names in `plain` settings after `change`; adding a name twice is a no-op.
pub fn edit_machines(plain: &str, change: &MachineChange) -> Result<String> {
    let mut settings = Settings::parse(plain)?;
    let mut nt4 = Nt4Settings::from_settings(&settings)?;
    match change {
        MachineChange::Add(name) => {
            if !valid_machine(name) {
                bail!("{name} is not a valid machine name (up to 15 letters, digits or -)");
            }
            if !nt4.machines.iter().any(|m| m.eq_ignore_ascii_case(name)) {
                nt4.machines.push(name.clone());
            }
        }
        MachineChange::Remove(name) => nt4.machines.retain(|m| !m.eq_ignore_ascii_case(name)),
        // the account lives in the container, not in the settings
        MachineChange::Reset(_) => {}
    }
    nt4.apply_to(&mut settings);
    Ok(settings.text())
}
