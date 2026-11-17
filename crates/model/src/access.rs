//! who may see and change what: areas, levels, roles and the role assignments kept in the
//! `access` section of the gateway's plain settings.
//!
//! every [`Op`](crate::ops::Op) declares what it needs with `Op::requires`, and the same table
//! drives the server (core's op dispatch) and the front ends (which windows and menu items to
//! show, which controls to make read-only). the front end is never the only check.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_yaml_ng::Value;

use crate::settings::Settings;

/// the settings section the assignments live in.
pub const SECTION: &str = "access";

/// a part of the gateway that access is granted for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Area {
    /// the server's status, facts and health checks.
    Status,
    /// networks, dhcp leases (and the macs in them) and interface status.
    Network,
    /// dns names and records.
    Dns,
    Firewall,
    Clock,
    /// components, settings and the containers they run.
    Services,
    /// the nt4 domain: its logins, groups and machines.
    Domain,
    Shares,
    Logs,
    /// processes (ending one is manage).
    Tasks,
    /// install, updates and purge.
    Updates,
    /// running commands on the gateway.
    Commands,
    /// lanlord's own users, roles and permissions.
    Users,
}

impl Area {
    pub const ALL: [Area; 13] = [
        Area::Status,
        Area::Network,
        Area::Dns,
        Area::Firewall,
        Area::Clock,
        Area::Services,
        Area::Domain,
        Area::Shares,
        Area::Logs,
        Area::Tasks,
        Area::Updates,
        Area::Commands,
        Area::Users,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Status => "Server status",
            Self::Network => "Network and DHCP",
            Self::Dns => "DNS",
            Self::Firewall => "Firewall",
            Self::Clock => "Date and time",
            Self::Services => "Services and settings",
            Self::Domain => "NT4 domain",
            Self::Shares => "Shares and logon scripts",
            Self::Logs => "Logs",
            Self::Tasks => "Task manager",
            Self::Updates => "Install and updates",
            Self::Commands => "Remote commands",
            Self::Users => "Users and permissions",
        }
    }
}

/// how much of an area someone gets; later is more.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// not listed, and its data is never sent.
    #[default]
    Hidden,
    View,
    Manage,
}

impl Level {
    pub const ALL: [Level; 3] = [Level::Hidden, Level::View, Level::Manage];

    pub fn title(self) -> &'static str {
        match self {
            Self::Hidden => "Hidden",
            Self::View => "View",
            Self::Manage => "Manage",
        }
    }
}

/// what an op needs from whoever sends it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// at least this level of this area.
    Area(Area, Level),
    /// the caller's own account: always allowed, whatever the roles.
    SelfOnly,
}

impl Need {
    pub fn view(area: Area) -> Self {
        Self::Area(area, Level::View)
    }

    pub fn manage(area: Area) -> Self {
        Self::Area(area, Level::Manage)
    }
}

pub const ADMINISTRATORS: &str = "Administrators";
pub const OPERATORS: &str = "Operators";
pub const USERS: &str = "Users";

/// the role everyone without an assignment has.
pub const DEFAULT_ROLE: &str = USERS;

/// a named group of area levels. areas left out are [`Level::Hidden`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Role {
    pub name: String,
    #[serde(default)]
    pub grants: BTreeMap<Area, Level>,
}

impl Role {
    pub fn level(&self, area: Area) -> Level {
        self.grants.get(&area).copied().unwrap_or_default()
    }

    /// the three roles every gateway has; they cannot be edited or removed.
    pub fn builtins() -> Vec<Role> {
        use Area::*;
        use Level::*;
        let make = |name: &str, grants: &[(Area, Level)]| Role {
            name: name.into(),
            grants: grants.iter().copied().collect(),
        };
        vec![
            Role {
                name: ADMINISTRATORS.into(),
                grants: Area::ALL.iter().map(|a| (*a, Manage)).collect(),
            },
            make(
                OPERATORS,
                &[
                    (Status, View),
                    (Network, Manage),
                    (Dns, Manage),
                    (Firewall, View),
                    (Clock, View),
                    (Services, Manage),
                    (Domain, View),
                    (Shares, Manage),
                    (Logs, View),
                    (Tasks, View),
                ],
            ),
            make(USERS, &[(Status, View), (Clock, View)]),
        ]
    }

    pub fn is_builtin(name: &str) -> bool {
        [ADMINISTRATORS, OPERATORS, USERS].contains(&name)
    }
}

/// the profile fields a person may change about themselves.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub full_name: String,
    #[serde(default)]
    pub description: String,
}

/// the level of every area, for one caller.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grants(pub BTreeMap<Area, Level>);

impl Grants {
    pub fn all(level: Level) -> Self {
        Self(Area::ALL.iter().map(|a| (*a, level)).collect())
    }

    pub fn level(&self, area: Area) -> Level {
        self.0.get(&area).copied().unwrap_or_default()
    }

    pub fn allows(&self, need: Need) -> bool {
        match need {
            Need::SelfOnly => true,
            Need::Area(area, level) => level == Level::Hidden || self.level(area) >= level,
        }
    }

    /// the grants of a person who was not given any by the gateway: administrators get all,
    /// anyone else the Users role.
    pub fn fallback(admin: bool) -> Self {
        if admin {
            Self::all(Level::Manage)
        } else {
            Access::default().grants("", false)
        }
    }
}

/// the role assignments of a gateway: the custom roles and who has which.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Access {
    /// roles admins defined, besides the built-in ones.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<Role>,
    /// the role names each user has; a user not listed here has [`DEFAULT_ROLE`].
    #[serde(default)]
    pub members: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, Profile>,
}

impl Access {
    /// the assignments of a gateway that never had any: its current administrators become
    /// Administrators; everyone else stays Users without being listed.
    pub fn migrate(admins: &[String]) -> Self {
        Self {
            members: admins
                .iter()
                .map(|a| (a.clone(), vec![ADMINISTRATORS.to_string()]))
                .collect(),
            ..Self::default()
        }
    }

    /// every role by name: the built-in ones and the custom ones.
    pub fn all_roles(&self) -> Vec<Role> {
        let mut roles = Role::builtins();
        roles.extend(self.roles.iter().cloned());
        roles
    }

    /// the role names `user` has. members of the gateway's admins group always count as
    /// Administrators, so nobody can lock the last administrator out by editing roles.
    pub fn roles_of(&self, user: &str, admin: bool) -> Vec<String> {
        let mut names = match self.members.get(user) {
            Some(names) if !names.is_empty() => names.clone(),
            _ => vec![DEFAULT_ROLE.to_string()],
        };
        if admin && !names.iter().any(|n| n == ADMINISTRATORS) {
            names.push(ADMINISTRATORS.into());
        }
        names
    }

    /// the most any of the user's roles gives for each area.
    pub fn grants(&self, user: &str, admin: bool) -> Grants {
        let roles = self.all_roles();
        let mine = self.roles_of(user, admin);
        let mut grants = BTreeMap::new();
        for area in Area::ALL {
            let level = roles
                .iter()
                .filter(|r| mine.contains(&r.name))
                .map(|r| r.level(area))
                .max()
                .unwrap_or_default();
            grants.insert(area, level);
        }
        Grants(grants)
    }

    /// the assignments in `settings`; `None` when it has none yet (not migrated).
    pub fn from_settings(settings: &Settings) -> Option<Self> {
        let section = settings.section(SECTION)?;
        serde_yaml_ng::from_value(section.clone()).ok()
    }

    pub fn store(&self, settings: &mut Settings) {
        // a value of plain data always converts
        let value = serde_yaml_ng::to_value(self).unwrap_or(Value::Null);
        settings.set_section(SECTION, value);
    }

    /// what is wrong with these assignments, if anything: a built-in name reused for a custom
    /// role, a role named twice, or a user with a role that does not exist.
    pub fn problem(&self) -> Option<String> {
        let mut seen = Vec::new();
        for role in &self.roles {
            let name = role.name.trim();
            if name.is_empty() {
                return Some("a role needs a name".into());
            }
            if Role::is_builtin(name) {
                return Some(format!("{name} is a built-in role"));
            }
            if seen.contains(&name) {
                return Some(format!("the role {name} is defined twice"));
            }
            seen.push(name);
        }
        let known: Vec<String> = self.all_roles().into_iter().map(|r| r.name).collect();
        for (user, names) in &self.members {
            if let Some(unknown) = names.iter().find(|n| !known.contains(n)) {
                return Some(format!("{user} has the unknown role {unknown}"));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_builtin_roles_have_the_shape_the_design_asks_for() {
        let access = Access::default();
        let admin = access.grants("root", true);
        assert!(Area::ALL.iter().all(|a| admin.level(*a) == Level::Manage));
        let mut access = Access::default();
        access.members.insert("olive".into(), names(&[OPERATORS]));
        let operator = access.grants("olive", false);
        assert_eq!(operator.level(Area::Services), Level::Manage);
        assert_eq!(operator.level(Area::Network), Level::Manage);
        assert_eq!(operator.level(Area::Shares), Level::Manage);
        for hidden in [Area::Users, Area::Updates, Area::Commands] {
            assert_eq!(operator.level(hidden), Level::Hidden, "{hidden:?}");
        }
        let user = access.grants("dana", false);
        assert_eq!(user.level(Area::Status), Level::View);
        assert_eq!(user.level(Area::Network), Level::Hidden);
        assert_eq!(user.level(Area::Users), Level::Hidden);
    }

    #[test]
    fn a_level_is_the_most_over_all_the_roles() {
        let mut access = Access::default();
        access.roles.push(Role {
            name: "Helpdesk".into(),
            grants: [(Area::Domain, Level::Manage)].into(),
        });
        access
            .members
            .insert("dana".into(), names(&[USERS, "Helpdesk"]));
        let grants = access.grants("dana", false);
        assert_eq!(grants.level(Area::Domain), Level::Manage);
        assert_eq!(grants.level(Area::Status), Level::View);
        assert_eq!(access.problem(), None);
    }

    #[test]
    fn the_admins_group_cannot_be_edited_out_of_administrators() {
        let mut access = Access::default();
        access.members.insert("root".into(), names(&[USERS]));
        assert_eq!(
            access.grants("root", true).level(Area::Users),
            Level::Manage
        );
        assert_eq!(
            access.grants("root", false).level(Area::Users),
            Level::Hidden
        );
    }

    #[test]
    fn a_gateway_without_assignments_migrates_admins_and_users() {
        let legacy = Settings::parse("dns:\n  zone: lab\n").unwrap();
        assert_eq!(Access::from_settings(&legacy), None);
        let access = Access::migrate(&names(&["admin"]));
        assert_eq!(access.roles_of("admin", true), [ADMINISTRATORS]);
        assert_eq!(access.roles_of("dana", false), [USERS]);
        // stored and read back, nothing else in the settings changes
        let mut settings = legacy;
        access.store(&mut settings);
        let reread = Settings::parse(&settings.text()).unwrap();
        assert_eq!(Access::from_settings(&reread), Some(access));
        assert!(settings.text().contains("zone: lab"));
        assert_eq!(
            Access::from_settings(&reread)
                .unwrap()
                .grants("dana", false),
            Grants::fallback(false)
        );
    }

    #[test]
    fn bad_assignments_are_named() {
        let mut access = Access::default();
        access.roles.push(Role {
            name: ADMINISTRATORS.into(),
            grants: BTreeMap::new(),
        });
        assert!(access.problem().unwrap().contains("built-in"));
        let mut access = Access::default();
        access.members.insert("dana".into(), names(&["Ghost"]));
        assert!(access.problem().unwrap().contains("Ghost"));
    }

    #[test]
    fn hidden_needs_nothing_and_self_is_always_allowed() {
        let none = Grants::default();
        assert!(none.allows(Need::SelfOnly));
        assert!(!none.allows(Need::view(Area::Status)));
        assert!(Grants::all(Level::View).allows(Need::view(Area::Dns)));
        assert!(!Grants::all(Level::View).allows(Need::manage(Area::Dns)));
    }
}
