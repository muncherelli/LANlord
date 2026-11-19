//! who is asking, whether they may, and what they get to see: the server side of the access
//! table (`Operation::requires`). every front end reaches it through the operation dispatch.

use anyhow::Result;
use serde_yaml_ng::Value;

use crate::bootstrap::AskPassword;
use crate::config;
use crate::ssh::Session;

pub use lanlord_model::access::*;
use lanlord_model::operations::{Operation, Reply};
use lanlord_model::settings::{Document, Settings};

/// who sends an operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    pub user: String,
    /// in the gateway's admins group, which always counts as Administrators.
    pub admin: bool,
}

impl Caller {
    /// the account the session signs in as: its name, and whether it is in `admins`. root counts
    /// as an administrator without the group (found by uid 0): bootstrap does not add it (root
    /// needs no membership or sudo rules), and a root login is accepted everywhere else.
    pub fn of_session(session: &Session) -> Result<Self> {
        let user: String = session.capture(&["id".into(), "-un".into()])?.trim().into();
        let groups = session
            .capture(&["id".into(), "-nG".into()])
            .unwrap_or_default();
        let uid = session
            .capture(&["id".into(), "-u".into()])
            .unwrap_or_default();
        let admin = uid.trim() == "0"
            || groups
                .split_whitespace()
                .any(|g| g == crate::admins::ADMINS_GROUP);
        Ok(Self { user, admin })
    }

    /// the named account as the machine `session` runs on knows it now: whether it is in
    /// `admins` is read again here, never taken from the caller. an unknown account or a name
    /// that is not a plain login name is an error.
    pub fn of_account(session: &Session, account: &str) -> Result<Self> {
        if !plain_account(account) {
            anyhow::bail!("not an account name");
        }
        let groups = session
            .capture(&["id".into(), "-nG".into(), "--".into(), account.into()])
            .map_err(|_| anyhow::anyhow!("unknown account"))?;
        let admin = groups
            .split_whitespace()
            .any(|g| g == crate::admins::ADMINS_GROUP);
        Ok(Self {
            user: account.to_string(),
            admin,
        })
    }
}

/// whether `name` is a plain login name: safe to pass as an argument to a system tool.
pub fn plain_account(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// the typed error for an operation the caller may not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Denied {
    pub area: Area,
    pub level: Level,
}

impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "permission denied: this needs {} on {}",
            self.level.title(),
            self.area.title()
        )
    }
}

impl std::error::Error for Denied {}

/// an operation the caller may run in general but not with what it asks for: the reason, in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "refused: {}", self.0)
    }
}

impl std::error::Error for Refused {}

/// the plain settings of the gateway, and the assignments in them; `None` for a gateway that
/// was never given any.
pub(crate) fn load(session: &Session, ask: AskPassword) -> Result<(Settings, Option<Access>)> {
    let settings = match config::pull_plain_with(session, ask)? {
        Some(bytes) => Settings::parse(&String::from_utf8(bytes)?)?,
        None => Settings::empty(),
    };
    let access = Access::from_settings(&settings).map_err(|e| anyhow::anyhow!(e))?;
    Ok((settings, access))
}

/// the assignments in force: stored, or everyone a User for a gateway without any.
pub(crate) fn assignments(session: &Session, ask: AskPassword) -> Result<Access> {
    Ok(load(session, ask)?.1.unwrap_or_default())
}

/// the caller's grants. administrators need no lookup.
pub(crate) fn grants_of(session: &Session, caller: &Caller, ask: AskPassword) -> Result<Grants> {
    if caller.admin {
        return Ok(Grants::all(Level::Manage));
    }
    Ok(assignments(session, ask)?.grants(&caller.user, false))
}

/// refuse an operation the grants do not cover.
pub fn authorize(grants: &Grants, operation: &Operation) -> Result<(), Denied> {
    let need = operation.requires();
    if !grants.allows(need) {
        return Err(denied(need));
    }
    // applying components changes what each of them configures
    if let Operation::Apply { modules } = operation {
        for area in areas_applied(modules) {
            if grants.level(area) < Level::Manage {
                return Err(Denied {
                    area,
                    level: Level::Manage,
                });
            }
        }
    }
    Ok(())
}

/// the areas whose settings an apply of `modules` puts on the server; all of them for none.
fn areas_applied(modules: &[String]) -> Vec<Area> {
    let names: Vec<&str> = if modules.is_empty() {
        lanlord_model::modules::MODULES
            .iter()
            .map(|m| m.id)
            .collect()
    } else {
        modules.iter().map(String::as_str).collect()
    };
    let mut areas: Vec<Area> = names.into_iter().map(area_of_section).collect();
    areas.sort();
    areas.dedup();
    areas
}

fn denied(need: Need) -> Denied {
    match need {
        Need::Area(area, level) => Denied { area, level },
        // the grants always allow it
        Need::SelfOnly => Denied {
            area: Area::Status,
            level: Level::View,
        },
    }
}

/// what the caller may be told of `reply`.
pub fn redact(grants: &Grants, caller: &Caller, reply: Reply) -> Reply {
    match reply {
        Reply::Leases(_) if grants.level(Area::Network) < Level::View => Reply::Leases(vec![]),
        Reply::Document(doc) => Reply::Document(redact_document(grants, caller, doc)),
        Reply::Access(access) => Reply::Access(redact_access(grants, caller, access)),
        other => other,
    }
}

fn redact_access(grants: &Grants, caller: &Caller, mut access: Access) -> Access {
    if grants.level(Area::Users) < Level::View {
        access.members.retain(|user, _| *user == caller.user);
        access.profiles.retain(|user, _| *user == caller.user);
    }
    access
}

fn redact_document(grants: &Grants, caller: &Caller, mut doc: Document) -> Document {
    if grants.level(Area::Services) < Level::Manage {
        doc.secrets_set.clear();
    }
    let Ok(mut settings) = Settings::parse(&doc.plain) else {
        return doc;
    };
    // nothing is rewritten for a caller who may see it all, so comments survive
    if grants.level(Area::Users) >= Level::View
        && grants.level(Area::Domain) >= Level::View
        && grants.level(Area::Network) >= Level::View
    {
        return doc;
    }
    if grants.level(Area::Users) < Level::View {
        settings.remove_section(SECTION);
    }
    if grants.level(Area::Domain) < Level::View {
        only_self_in_users(&mut settings, &caller.user);
    }
    if grants.level(Area::Network) < Level::View {
        drop_leases(&mut settings);
    }
    doc.plain = settings.text();
    doc
}

fn section_map(settings: &Settings, key: &str) -> Option<serde_yaml_ng::Mapping> {
    match settings.section(key) {
        Some(Value::Mapping(map)) => Some(map.clone()),
        _ => None,
    }
}

/// callers without Domain View see only their own login, and no machine names.
fn only_self_in_users(settings: &mut Settings, user: &str) {
    let Some(mut map) = section_map(settings, "nt4-domain") else {
        return;
    };
    map.remove("machines");
    if let Some(Value::Sequence(users)) = map.get_mut("users") {
        users.retain(|u| u.get("name").and_then(Value::as_str) == Some(user));
    }
    settings.set_section("nt4-domain", Value::Mapping(map));
}

/// callers without Network View get no static leases (they carry macs).
fn drop_leases(settings: &mut Settings) {
    let Some(mut map) = section_map(settings, "dns") else {
        return;
    };
    if let Some(Value::Mapping(dhcp)) = map.get_mut("dhcp") {
        dhcp.remove("hosts");
    }
    settings.set_section("dns", Value::Mapping(map));
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_account_name_must_be_a_plain_login_name() {
        let session = crate::ssh::Session::local();
        for bad in ["", "-x", "a b", "a;b", "$(id)", &"a".repeat(33)] {
            assert!(super::Caller::of_account(&session, bad).is_err(), "{bad:?}");
        }
    }

    use super::*;
    use lanlord_model::leases::Lease;

    #[cfg(unix)]
    #[test]
    fn a_root_login_is_an_administrator_without_the_group() {
        let who = |user: &str, uid: u32, groups: &str| {
            Session::fake(&format!(
                "for a; do last=$a; done\ncase \"$last\" in\n  \"id -un\") echo {user} ;;\n  \"id -u\") echo {uid} ;;\n  \"id -nG\") echo {groups} ;;\nesac"
            ))
        };
        let root = Caller::of_session(&who("root", 0, "root")).unwrap();
        assert_eq!((root.user.as_str(), root.admin), ("root", true));
        let other = Caller::of_session(&who("olive", 1000, "olive")).unwrap();
        assert!(!other.admin);
        let admin = Caller::of_session(&who("olive", 1000, "olive admins")).unwrap();
        assert!(admin.admin);
        // the name alone does not make an administrator
        let named = Caller::of_session(&who("root", 1000, "root")).unwrap();
        assert!(!named.admin);
    }

    fn caller(user: &str) -> Caller {
        Caller {
            user: user.into(),
            admin: false,
        }
    }

    fn grants(role: &str) -> Grants {
        let mut access = Access::default();
        access.members.insert("u".into(), vec![role.into()]);
        access.grants("u", false)
    }

    fn operations() -> Vec<Operation> {
        vec![
            Operation::Leases,
            Operation::ServerFacts,
            Operation::ClockUpdateNow,
            Operation::Power {
                power: lanlord_model::power::Power::Reboot,
            },
            Operation::LoadAccess,
            Operation::AccountInfo,
            Operation::Apply { modules: vec![] },
        ]
    }

    fn allowed(grants: &Grants) -> Vec<bool> {
        operations()
            .iter()
            .map(|o| authorize(grants, o).is_ok())
            .collect()
    }

    #[test]
    fn an_apply_needs_manage_on_every_area_it_changes() {
        let operator = grants(OPERATORS);
        let apply = |m: &[&str]| Operation::Apply {
            modules: m.iter().map(|s| s.to_string()).collect(),
        };
        assert!(authorize(&operator, &apply(&["docker"])).is_ok());
        let denied = authorize(&operator, &apply(&["docker", "nt4-domain"])).unwrap_err();
        assert_eq!(denied.area, Area::Domain);
        assert!(authorize(&operator, &apply(&[])).is_err());
        assert!(authorize(&grants(ADMINISTRATORS), &apply(&[])).is_ok());
    }

    #[test]
    fn changing_the_networks_needs_manage_on_network() {
        let change = Operation::ChangeNetworks {
            edit: lanlord_model::lan_edit::Edit::Remove("lab".into()),
        };
        assert!(authorize(&grants(ADMINISTRATORS), &change).is_ok());
        assert!(authorize(&grants(OPERATORS), &change).is_ok());
        let denied = authorize(&grants(USERS), &change).unwrap_err();
        assert_eq!(denied.area, Area::Network);
        // looking at the networks is not enough to change them
        let mut viewer = Access::default();
        viewer.roles.push(Role {
            name: "Viewers".into(),
            grants: [(Area::Network, Level::View)].into_iter().collect(),
        });
        viewer.members.insert("u".into(), vec!["Viewers".into()]);
        assert!(authorize(&viewer.grants("u", false), &change).is_err());
        assert!(authorize(&viewer.grants("u", false), &Operation::Leases).is_ok());
    }

    #[test]
    fn administrators_may_do_everything() {
        assert_eq!(allowed(&grants(ADMINISTRATORS)), [true; 7]);
    }

    #[test]
    fn operators_run_the_day_to_day_but_not_power_or_permissions() {
        // leases, facts, clock update (view only: denied), power, access, self, and apply of
        // everything (an operator does not manage every area, so it is denied)
        let want = [true, true, false, false, false, true, false];
        assert_eq!(allowed(&grants(OPERATORS)), want);
    }

    #[test]
    fn users_get_status_and_their_own_account() {
        let want = [false, true, false, false, false, true, false];
        assert_eq!(allowed(&grants(USERS)), want);
        let denied = authorize(&grants(USERS), &Operation::Leases).unwrap_err();
        assert_eq!(denied.area, Area::Network);
        assert!(denied.to_string().contains("permission denied"));
    }

    #[test]
    fn leases_and_macs_need_network_view() {
        let lease = Lease {
            expires: 0,
            mac: "00:00:5e:00:53:01".into(),
            address: "192.0.2.20".into(),
            name: "pc01".into(),
        };
        let reply = Reply::Leases(vec![lease.clone()]);
        let hidden = redact(&grants(USERS), &caller("dana"), reply.clone());
        assert_eq!(hidden, Reply::Leases(vec![]));
        assert_eq!(
            redact(&grants(OPERATORS), &caller("dana"), reply.clone()),
            reply
        );
    }

    fn document() -> Document {
        Document {
            plain: "nt4-domain:\n  users:\n    - name: dana\n    - name: eve\n  machines: [pc01]\ndns:\n  dhcp:\n    range: [192.0.2.10, 192.0.2.20]\n    hosts:\n      - {name: pc01, mac: '00:00:5e:00:53:01', address: 192.0.2.11}\naccess:\n  members:\n    eve: [Operators]\n".into(),
            secrets_set: vec!["nt4-domain.admin_password".into()],
            legacy: false,
            needs_move: false,
        }
    }

    #[test]
    fn users_see_only_themselves_and_no_macs_or_secrets() {
        let Reply::Document(doc) =
            redact(&grants(USERS), &caller("dana"), Reply::Document(document()))
        else {
            panic!("a document")
        };
        assert!(doc.secrets_set.is_empty());
        for gone in ["eve", "00:00:5e", "machines", "access"] {
            assert!(!doc.plain.contains(gone), "{gone}: {}", doc.plain);
        }
        assert!(doc.plain.contains("dana"));
    }

    #[test]
    fn administrators_see_the_whole_document() {
        let Reply::Document(doc) = redact(
            &Grants::all(Level::Manage),
            &caller("root"),
            Reply::Document(document()),
        ) else {
            panic!("a document")
        };
        assert!(doc.plain.contains("eve") && doc.plain.contains("00:00:5e"));
        assert_eq!(doc.secrets_set.len(), 1);
    }
}
