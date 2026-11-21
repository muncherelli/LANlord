//! the nt4 settings a front end edits: the first server and its domain: the domain, the
//! server's name, the machines to prepare accounts for and roaming profiles. logins and
//! secrets are handled elsewhere; keys this type does not know stay as they are.

use anyhow::{bail, Result};
use serde_yaml_ng::{Mapping, Value};

use crate::format::Context;
use crate::nt4_servers::{self, DOMAINS, LEGACY as SECTION, SERVERS};
use crate::settings::{Settings, ValidationError};

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
pub fn netbios_ok(name: &str) -> bool {
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
    /// read the first nt4 server and its domain; no server gives the defaults. settings that
    /// still have the single `nt4-domain:` shape are read as if migrated.
    // pr2 extends this: the add and remove server ui picks which server to edit; today the
    // front ends only know one, so everything here is the first server and its domain.
    pub fn from_settings(settings: &Settings) -> Result<Self> {
        match settings.section(SECTION) {
            None | Some(Value::Null) | Some(Value::Mapping(_)) => {}
            Some(_) => bail!("{SECTION} must be a mapping"),
        }
        let mut settings = settings.clone();
        nt4_servers::migrate_legacy(&mut settings, &Context::default())?;
        let defaults = Self::default();
        let Some(Value::Sequence(servers)) = settings.section(SERVERS) else {
            return Ok(defaults);
        };
        let Some(Value::Mapping(server)) = servers.first() else {
            return Ok(defaults);
        };
        let workgroup = text(server, "role").as_deref() == Some("workgroup");
        let name = text(server, if workgroup { "workgroup" } else { "domain" });
        let domain = name.as_ref().and_then(|n| match settings.section(DOMAINS) {
            Some(Value::Mapping(domains)) => match domains.get(n.as_str()) {
                Some(Value::Mapping(d)) => Some((n.clone(), d.clone())),
                _ => None,
            },
            _ => None,
        });
        let machines = match domain.as_ref().and_then(|(_, d)| d.get("machines")) {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Sequence(items)) => items
                .iter()
                .filter_map(|m| match m {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                })
                .collect(),
            Some(_) => bail!(
                "{DOMAINS}.{}.machines must be a list",
                name.unwrap_or_default()
            ),
        };
        let mode = match text(server, "role").as_deref() {
            Some("pdc") | None => "domain".to_string(),
            Some(other) => other.to_string(),
        };
        Ok(Self {
            mode,
            name: name.unwrap_or(defaults.name),
            netbios_name: text(server, "netbios_name").unwrap_or_default(),
            roaming_profiles: domain
                .as_ref()
                .and_then(|(_, d)| d.get("roaming_profiles"))
                .and_then(Value::as_bool)
                == Some(true),
            software: server.get("software").and_then(Value::as_bool) == Some(true),
            machines,
        })
    }

    /// write this into the first nt4 server and its domain (made when there is none); the other
    /// servers, the logins and every other key stay as they are.
    pub fn apply_to(&self, settings: &mut Settings) {
        // an error here means a section that is not a mapping, which validation refuses
        let _ = nt4_servers::migrate_legacy(settings, &Context::default());
        if settings.section(SECTION).is_none() {
            settings.set_section(SECTION, Value::Mapping(Mapping::new()));
        }
        let mut servers = match settings.section(SERVERS) {
            Some(Value::Sequence(list)) => list.clone(),
            _ => Vec::new(),
        };
        if servers.is_empty() {
            let mut first = Mapping::new();
            first.insert("name".into(), nt4_servers::LEGACY.into());
            servers.push(Value::Mapping(first));
        }
        let Value::Mapping(server) = &mut servers[0] else {
            return;
        };
        let workgroup = self.mode == "workgroup";
        let old = ["domain", "workgroup"]
            .iter()
            .find_map(|k| text(server, k))
            .unwrap_or_default();
        let role = if self.mode == "domain" {
            "pdc"
        } else {
            self.mode.as_str()
        };
        server.insert("role".into(), role.into());
        let (keep, drop) = if workgroup {
            ("workgroup", "domain")
        } else {
            ("domain", "workgroup")
        };
        server.insert(keep.into(), self.name.as_str().into());
        server.remove(drop);
        if self.netbios_name.is_empty() {
            server.remove("netbios_name");
        } else {
            server.insert("netbios_name".into(), self.netbios_name.as_str().into());
        }
        server.insert("software".into(), self.software.into());
        server.entry("wins".into()).or_insert("serve".into());

        // the domain moves with a rename unless another server still names the old one
        let shared = servers.iter().skip(1).any(|s| {
            ["domain", "workgroup"]
                .iter()
                .any(|k| s.as_mapping().and_then(|m| text(m, k)).as_deref() == Some(old.as_str()))
        });
        let mut domains = match settings.section(DOMAINS) {
            Some(Value::Mapping(m)) => m.clone(),
            _ => Mapping::new(),
        };
        let mut domain = match domains.get(old.as_str()) {
            Some(Value::Mapping(d)) => d.clone(),
            _ => Mapping::new(),
        };
        if old != self.name && !shared {
            domains.remove(old.as_str());
        }
        if let Some(Value::Mapping(existing)) = domains.get(self.name.as_str()) {
            domain = existing.clone();
        }
        domain.insert("backend".into(), "nt4".into());
        domain.insert("roaming_profiles".into(), self.roaming_profiles.into());
        let machines = self.machines.iter().map(|m| m.as_str().into()).collect();
        domain.insert("machines".into(), Value::Sequence(machines));
        domains.insert(self.name.as_str().into(), Value::Mapping(domain));
        settings.set_section(DOMAINS, Value::Mapping(domains));
        settings.set_section(SERVERS, Value::Sequence(servers));
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

/// the machine names in `plain` settings after `change`; adding a name twice is a no-operation.
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

#[cfg(test)]
mod split_tests {
    use super::*;

    const TWO: &str = "domains:\n  RETRODOM: {machines: [ws1]}\nnt4-domain:\n  admin_password: example-secret\nnt4-servers:\n  - {name: nt4-domain, role: pdc, domain: RETRODOM, netbios_name: LANLORD, network: lab}\n  - {name: files, role: workgroup, workgroup: HOME, netbios_name: FILES, network: lab, wins: off}\n";

    #[test]
    fn a_front_end_reads_the_first_server_and_its_domain() {
        let settings = Settings::parse(TWO).unwrap();
        let nt4 = Nt4Settings::from_settings(&settings).unwrap();
        assert_eq!(nt4.mode, "domain");
        assert_eq!(nt4.name, "RETRODOM");
        assert_eq!(nt4.netbios_name, "LANLORD");
        assert_eq!(nt4.machines, vec!["ws1".to_string()]);
    }

    #[test]
    fn an_old_single_section_is_read_through_the_migration() {
        let settings =
            Settings::parse("nt4-domain:\n  mode: workgroup\n  name: HOME\n  netbios_name: BOX\n")
                .unwrap();
        let nt4 = Nt4Settings::from_settings(&settings).unwrap();
        assert_eq!(
            (nt4.mode.as_str(), nt4.name.as_str()),
            ("workgroup", "HOME")
        );
        assert_eq!(nt4.netbios_name, "BOX");
    }

    #[test]
    fn saving_edits_the_first_server_and_leaves_the_others() {
        let mut settings = Settings::parse(TWO).unwrap();
        let mut nt4 = Nt4Settings::from_settings(&settings).unwrap();
        nt4.netbios_name = "BIGBOX".into();
        nt4.roaming_profiles = true;
        nt4.apply_to(&mut settings);
        let servers = settings.section(SERVERS).unwrap();
        assert_eq!(servers[0]["netbios_name"], "BIGBOX");
        assert_eq!(servers[1]["name"], "files");
        assert_eq!(servers[1]["netbios_name"], "FILES");
        assert_eq!(servers.as_sequence().unwrap().len(), 2);
        assert_eq!(
            settings.section(DOMAINS).unwrap()["RETRODOM"]["roaming_profiles"],
            true
        );
        // the logins did not move
        assert_eq!(
            settings.section(SECTION).unwrap()["admin_password"],
            "example-secret"
        );
    }

    #[test]
    fn renaming_the_domain_moves_its_entry() {
        let mut settings = Settings::parse(TWO).unwrap();
        let mut nt4 = Nt4Settings::from_settings(&settings).unwrap();
        nt4.name = "NEWDOM".into();
        nt4.apply_to(&mut settings);
        let domains = settings.section(DOMAINS).unwrap();
        assert!(domains.get("RETRODOM").is_none());
        assert_eq!(domains["NEWDOM"]["machines"][0], "ws1");
        assert_eq!(settings.section(SERVERS).unwrap()[0]["domain"], "NEWDOM");
    }

    #[test]
    fn switching_the_first_server_to_a_workgroup_changes_its_role() {
        let mut settings = Settings::parse(TWO).unwrap();
        let mut nt4 = Nt4Settings::from_settings(&settings).unwrap();
        nt4.mode = "workgroup".into();
        nt4.name = "HOME".into();
        nt4.apply_to(&mut settings);
        let first = &settings.section(SERVERS).unwrap()[0];
        assert_eq!(first["role"], "workgroup");
        assert_eq!(first["workgroup"], "HOME");
    }
}
