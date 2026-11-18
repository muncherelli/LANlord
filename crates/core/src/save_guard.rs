//! what a save of the settings document may change. the document holds every area's settings,
//! so a save is checked section by section against what is stored: an area the caller manages
//! takes their edit, one they cannot see (and so never received intact) keeps what is stored,
//! and an edit to one they can see but not manage is refused.

use std::collections::BTreeSet;

use anyhow::Result;
use serde_yaml_ng::{Mapping, Value};

use crate::access::{Caller, Refused};
use lanlord_model::access::area_of_section;
use lanlord_model::access::{Access, Area, Grants, Level};
use lanlord_model::settings::{secret_section, Settings};

/// the settings to write for `sent`, given what is `stored`. administrators write what they
/// sent; nobody else changes an area they may not manage.
pub fn merge_save(
    stored: &Settings,
    sent: &str,
    grants: &Grants,
    caller: &Caller,
) -> Result<String> {
    let mut sent = Settings::parse(sent)?;
    if caller.admin {
        return Ok(sent.text());
    }
    let names: BTreeSet<String> = stored
        .sections()
        .into_iter()
        .chain(sent.sections())
        .collect();
    for name in names {
        if name == "dns" {
            settle_dns(stored, &mut sent, grants)?;
        } else {
            let (before, after) = (stored.section(&name), sent.section(&name));
            let kept = settle(&name, area_of_section(&name), before, after, grants)?;
            put(&mut sent, &name, kept);
        }
    }
    check_access(stored, &sent, grants)?;
    Ok(sent.text())
}

/// the secrets a save sets (by path, `section.key`) need Manage on the area of their section,
/// like the plain settings do.
pub fn check_updates(updates: &[(String, String)], grants: &Grants, caller: &Caller) -> Result<()> {
    if caller.admin {
        return Ok(());
    }
    for (path, _) in updates {
        let section = secret_section(path).map_err(|e| Refused(format!("{e}")))?;
        let area = area_of_section(&section);
        if grants.level(area) < Level::Manage {
            return Err(Refused(format!("setting {path} needs Manage on {}", area.title())).into());
        }
    }
    Ok(())
}

fn put(settings: &mut Settings, name: &str, value: Option<Value>) {
    match value {
        Some(value) => settings.set_section(name, value),
        None => settings.remove_section(name),
    }
}

/// what `name` becomes: the edit, the stored section, or a refusal.
fn settle(
    name: &str,
    area: Area,
    before: Option<&Value>,
    after: Option<&Value>,
    grants: &Grants,
) -> Result<Option<Value>> {
    if before == after {
        return Ok(after.cloned());
    }
    match grants.level(area) {
        Level::Manage => Ok(after.cloned()),
        Level::View => Err(Refused(format!(
            "you may look at {} but not change it ({name})",
            area.title()
        ))
        .into()),
        Level::Hidden => Ok(before.cloned()),
    }
}

/// dns holds two areas: its names and records, and the dhcp hosts with their macs (network).
fn settle_dns(stored: &Settings, sent: &mut Settings, grants: &Grants) -> Result<()> {
    let (old_dns, old_hosts) = split_hosts(stored.section("dns"));
    let (new_dns, new_hosts) = split_hosts(sent.section("dns"));
    let dns = settle("dns", Area::Dns, old_dns.as_ref(), new_dns.as_ref(), grants)?;
    let hosts = settle(
        "dns.dhcp.hosts",
        Area::Network,
        old_hosts.as_ref(),
        new_hosts.as_ref(),
        grants,
    )?;
    put(sent, "dns", join_hosts(dns, hosts));
    Ok(())
}

fn split_hosts(dns: Option<&Value>) -> (Option<Value>, Option<Value>) {
    let Some(Value::Mapping(map)) = dns else {
        return (dns.cloned(), None);
    };
    let mut map = map.clone();
    let hosts = match map.get_mut("dhcp") {
        Some(Value::Mapping(dhcp)) => dhcp.remove("hosts"),
        _ => None,
    };
    (Some(Value::Mapping(map)), hosts)
}

fn join_hosts(dns: Option<Value>, hosts: Option<Value>) -> Option<Value> {
    let (Some(Value::Mapping(mut map)), Some(hosts)) = (dns.clone(), hosts) else {
        return dns;
    };
    let mut dhcp = match map.remove("dhcp") {
        Some(Value::Mapping(dhcp)) => dhcp,
        _ => Mapping::new(),
    };
    dhcp.insert(Value::String("hosts".into()), hosts);
    map.insert(Value::String("dhcp".into()), Value::Mapping(dhcp));
    Some(Value::Mapping(map))
}

/// the access section, if the save changes it, may not give the caller's friends more than the
/// caller has.
fn check_access(stored: &Settings, sent: &Settings, grants: &Grants) -> Result<()> {
    let old = Access::from_settings(stored)
        .map_err(anyhow::Error::msg)?
        .unwrap_or_default();
    let new = Access::from_settings(sent)
        .map_err(anyhow::Error::msg)?
        .unwrap_or_default();
    match old.escalation(&new, grants, false) {
        Some(why) => Err(Refused(why).into()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller() -> Caller {
        Caller {
            user: "olly".into(),
            admin: false,
        }
    }

    const STORED: &str = "dns:\n  zone: lab\n  dhcp:\n    hosts:\n      - name: pc01\n        mac: 00:00:5e:00:53:01\n\
        nt4-domain:\n  users:\n    - name: ann\n    - name: olly\n  machines: [ws1]\n\
        firewall:\n  allow: [ssh]\ndocker: {}\n";

    fn grants(levels: &[(Area, Level)]) -> Grants {
        let mut g = Grants::fallback(false);
        for (area, level) in levels {
            g.0.insert(*area, *level);
        }
        g
    }

    fn merged(sent: &str, g: &Grants) -> Result<String> {
        merge_save(&Settings::parse(STORED).unwrap(), sent, g, &caller())
    }

    #[test]
    fn an_administrator_writes_what_they_sent() {
        let admin = Caller {
            user: "ann".into(),
            admin: true,
        };
        let out = merge_save(
            &Settings::parse(STORED).unwrap(),
            "firewall:\n  allow: []\n",
            &Grants::all(Level::Manage),
            &admin,
        )
        .unwrap();
        assert!(!out.contains("ssh") && !out.contains("zone"));
    }

    #[test]
    fn an_edit_to_an_area_that_is_only_viewable_is_refused() {
        let g = grants(&[
            (Area::Firewall, Level::View),
            (Area::Services, Level::Manage),
        ]);
        let err = merged(&STORED.replace("allow: [ssh]", "allow: [all]"), &g).unwrap_err();
        assert!(err.downcast_ref::<Refused>().is_some(), "{err:#}");
        // the same text unchanged is fine
        assert!(merged(STORED, &g).is_ok());
    }

    #[test]
    fn a_secret_is_set_only_by_someone_who_manages_its_area() {
        let g = grants(&[(Area::Services, Level::Manage)]);
        let one = |path: &str| vec![(path.to_string(), "pw".to_string())];
        assert!(check_updates(&one("docker.token"), &g, &caller()).is_ok());
        let err = check_updates(&one("nt4-domain.admin_password"), &g, &caller()).unwrap_err();
        assert!(err.downcast_ref::<Refused>().is_some(), "{err:#}");
        assert!(check_updates(&one("nt4-domain.users.0.password"), &g, &caller()).is_err());
        // the writer reads these as the domain's secrets, so they are checked as such
        for sneaky in [
            ".nt4-domain.admin_password",
            "nt4-domain[x].password",
            "[x].password",
            "",
            "nt4-domain.name",
        ] {
            let err = check_updates(&one(sneaky), &g, &caller()).unwrap_err();
            assert!(err.downcast_ref::<Refused>().is_some(), "{sneaky:?}");
        }
        assert!(check_updates(&one("docker[a].token"), &g, &caller()).is_ok());
        let admin = Caller {
            user: "ann".into(),
            admin: true,
        };
        assert!(check_updates(&one("nt4-domain.admin_password"), &g, &admin).is_ok());
    }

    #[test]
    fn an_area_that_is_managed_takes_the_edit() {
        let g = grants(&[(Area::Firewall, Level::Manage)]);
        let out = merged(&STORED.replace("allow: [ssh]", "allow: [all]"), &g).unwrap();
        assert!(out.contains("all") && out.contains("pc01"));
    }

    #[test]
    fn sections_the_caller_never_saw_come_back_from_the_store() {
        // the editor was sent a redacted document: one login, no machines, no hosts
        let g = grants(&[(Area::Services, Level::Manage)]);
        let redacted = "dns:\n  zone: lab\nnt4-domain:\n  users:\n    - name: olly\n\
            firewall:\n  allow: [ssh]\ndocker: {}\n";
        let out = merged(redacted, &g).unwrap();
        assert!(
            out.contains("pc01") && out.contains("00:00:5e:00:53:01"),
            "{out}"
        );
        assert!(out.contains("ann") && out.contains("ws1"), "{out}");
    }

    #[test]
    fn dns_names_and_dhcp_hosts_are_separate_areas() {
        let g = grants(&[(Area::Dns, Level::Manage), (Area::Network, Level::Hidden)]);
        let sent = "dns:\n  zone: other\nnt4-domain:\n  users: []\nfirewall:\n  allow: [ssh]\ndocker: {}\n";
        let out = merged(sent, &g).unwrap();
        assert!(out.contains("zone: other") && out.contains("pc01"), "{out}");
    }

    #[test]
    fn nobody_but_an_administrator_saves_themselves_into_administrators() {
        let g = Grants::all(Level::Manage);
        let sent = format!("{STORED}access:\n  members:\n    olly: [Administrators]\n");
        let err = merged(&sent, &g).unwrap_err();
        assert!(err.downcast_ref::<Refused>().is_some(), "{err:#}");
    }

    #[test]
    fn a_save_that_leaves_the_assignments_alone_goes_through() {
        let g = grants(&[(Area::Users, Level::Manage)]);
        let sent = format!("{STORED}access:\n  members:\n    olly: [Users]\n");
        let stored = format!("{STORED}access:\n  members:\n    olly: [Users]\n");
        let out = merge_save(&Settings::parse(&stored).unwrap(), &sent, &g, &caller()).unwrap();
        assert!(out.contains("olly"));
    }
}
