//! the nt4-domain settings a front end edits: the domain, its server name,
//! the machines to prepare accounts for and roaming profiles. logins and
//! secrets are handled elsewhere; keys this type does not know stay as they are.

use anyhow::{bail, Result};
use serde_yaml_ng::{Mapping, Value};

use crate::apply;
use crate::bootstrap::AskPassword;
use crate::settings::{self, Settings, ValidationError};
use crate::ssh::Session;
use crate::users::cached;

const SECTION: &str = "nt4-domain";

#[derive(Clone, Debug, PartialEq, Eq)]
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
pub enum MachineChange {
    Add(String),
    Remove(String),
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
    }
    nt4.apply_to(&mut settings);
    Ok(settings.text())
}

/// the machines your settings define on `host`.
pub fn machines_on(
    session: &Session,
    host: &str,
    identity: &age::x25519::Identity,
    ask_password: AskPassword,
) -> Result<Vec<String>> {
    let stored = settings::load_document_with(session, identity, ask_password)?;
    if stored == settings::Document::default() {
        bail!("no settings on {host}; run lanlord config push first");
    }
    Ok(Nt4Settings::from_settings(&Settings::parse(&stored.plain)?)?.machines)
}

/// change the machines in your settings on `host` and apply the module.
pub fn change_machine(
    session: &Session,
    host: &str,
    identity: &age::x25519::Identity,
    change: &MachineChange,
    ask_password: AskPassword,
) -> Result<()> {
    let mut ask = cached(ask_password);
    let stored = settings::load_document_with(session, identity, &mut ask)?;
    if stored == settings::Document::default() {
        bail!("no settings on {host}; run lanlord config push first");
    }
    let edited = edit_machines(&stored.plain, change)?;
    settings::save_document_with(session, host, identity, &edited, &[], &mut ask)?;
    apply::apply_with(
        session,
        host,
        Some(identity),
        &["nt4-domain".to_string()],
        &mut ask,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "docker: {}\nnt4-domain:\n  mode: workgroup\n  name: lab\n  address: 172.16.120.1/22\n  machines: [ws1, ws2]\n  users:\n    - name: james\n";

    #[test]
    fn a_missing_section_gives_the_defaults() {
        let settings = Settings::parse("docker: {}\n").unwrap();
        assert_eq!(
            Nt4Settings::from_settings(&settings).unwrap(),
            Nt4Settings::default()
        );
        let null = Settings::parse("nt4-domain:\n").unwrap();
        assert_eq!(
            Nt4Settings::from_settings(&null).unwrap(),
            Nt4Settings::default()
        );
    }

    #[test]
    fn the_section_is_read_and_written_keeping_other_keys() {
        let mut settings = Settings::parse(TEXT).unwrap();
        let mut nt4 = Nt4Settings::from_settings(&settings).unwrap();
        assert_eq!(
            (nt4.mode.as_str(), nt4.name.as_str(), nt4.machines.len()),
            ("workgroup", "lab", 2)
        );
        nt4.netbios_name = "gw".into();
        nt4.roaming_profiles = true;
        nt4.software = true;
        nt4.machines.push("ws3".into());
        nt4.apply_to(&mut settings);
        let text = settings.text();
        assert!(text.contains("address: 172.16.120.1/22") && text.contains("james"));
        assert!(text.contains("ws3") && text.contains("netbios_name: gw"));
        let back = Nt4Settings::from_settings(&settings).unwrap();
        assert_eq!(back, nt4);
        // clearing the server name removes the key again
        nt4.netbios_name.clear();
        nt4.apply_to(&mut settings);
        assert!(!settings.text().contains("netbios_name"));
    }

    #[test]
    fn bad_sections_are_refused() {
        for text in ["nt4-domain: 3\n", "nt4-domain:\n  machines: x\n"] {
            let settings = Settings::parse(text).unwrap();
            assert!(Nt4Settings::from_settings(&settings).is_err(), "{text}");
        }
    }

    #[test]
    fn validation_follows_the_playbook() {
        let mut nt4 = Nt4Settings::default();
        assert!(nt4.validate(true).is_empty());
        let errors = nt4.validate(false);
        assert_eq!(errors[0].path, "nt4-domain.admin_password");
        nt4.mode = "workgroup".into();
        assert!(nt4.validate(false).is_empty(), "a workgroup needs none");
        nt4.mode = "odd".into();
        nt4.name = "toolongfordomainnames".into();
        nt4.netbios_name = "-x".into();
        nt4.machines = vec!["ok-1".into(), "1bad".into(), "has space".into(), "".into()];
        let paths: Vec<_> = nt4.validate(true).into_iter().map(|e| e.path).collect();
        assert_eq!(
            paths,
            [
                "nt4-domain.mode",
                "nt4-domain.name",
                "nt4-domain.netbios_name",
                "nt4-domain.machines[1]",
                "nt4-domain.machines[2]",
                "nt4-domain.machines[3]"
            ]
        );
    }

    #[test]
    fn machines_are_added_once_and_removed() {
        let added = edit_machines(TEXT, &MachineChange::Add("nt4test".into())).unwrap();
        let twice = edit_machines(&added, &MachineChange::Add("NT4TEST".into())).unwrap();
        let machines = |t: &str| {
            Nt4Settings::from_settings(&Settings::parse(t).unwrap())
                .unwrap()
                .machines
        };
        assert_eq!(machines(&twice), ["ws1", "ws2", "nt4test"]);
        let removed = edit_machines(&twice, &MachineChange::Remove("WS1".into())).unwrap();
        assert_eq!(machines(&removed), ["ws2", "nt4test"]);
        assert!(removed.contains("james"));
        assert!(edit_machines(TEXT, &MachineChange::Add("bad name".into())).is_err());
    }

    #[cfg(unix)]
    mod gateway {
        use super::*;
        use crate::testgw::{identity, no_password, Gateway};

        const STORED: &str =
            "docker: {}\nnt4-domain:\n  mode: domain\n  name: RETRONET\n  machines: [ws1]\n";

        #[test]
        fn machines_are_listed_changed_and_applied() {
            let (gw, id) = (Gateway::new("nt4-machines"), identity("nt4-machines"));
            gw.seed_sealed(&id, STORED);
            let listed = machines_on(&gw.session, "gw", &id, &mut no_password).unwrap();
            assert_eq!(listed, ["ws1"]);
            let add = MachineChange::Add("nt4test".into());
            change_machine(&gw.session, "gw", &id, &add, &mut no_password).unwrap();
            assert!(gw.plain().unwrap().contains("nt4test"));
        }

        #[test]
        fn machines_need_settings_on_the_gateway() {
            let (gw, id) = (Gateway::new("nt4-none"), identity("nt4-none"));
            assert!(machines_on(&gw.session, "gw", &id, &mut no_password).is_err());
            let add = MachineChange::Add("x".into());
            assert!(change_machine(&gw.session, "gw", &id, &add, &mut no_password).is_err());
        }
    }
}
