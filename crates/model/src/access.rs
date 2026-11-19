//! who may see and change what: areas, levels, roles and the role assignments kept in the
//! `access` section of the gateway's plain settings.
//!
//! every [`Operation`](crate::operations::Operation) declares what it needs with
//! `Operation::requires`, and the same table drives the server (core's operation dispatch) and the
//! front ends (which windows and menu items to show, which controls to make read-only). the front
//! end is never the only check.

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

/// the area a top-level section of the settings belongs to; what saving it needs.
pub fn area_of_section(key: &str) -> Area {
    match key {
        "dns" => Area::Dns,
        "firewall" => Area::Firewall,
        "nt4-domain" => Area::Domain,
        "network" | "networks" => Area::Network,
        "remote-commands" => Area::Commands,
        SECTION => Area::Users,
        _ => Area::Services,
    }
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

/// what an operation needs from whoever sends it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// at least this level of this area.
    Area(Area, Level),
    /// the caller's own account: always allowed, whatever the roles.
    SelfOnly,
    /// manage on every area: what only an administrator has.
    Admin,
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
            Need::Admin => Area::ALL.iter().all(|a| self.level(*a) >= Level::Manage),
            Need::Area(area, level) => self.level(area) >= level,
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

    /// the assignments in `settings`; `None` when it has none yet (not migrated), and an error
    /// when the section is there but cannot be read, so that nothing is saved over it.
    pub fn from_settings(settings: &Settings) -> Result<Option<Self>, String> {
        let Some(section) = settings.section(SECTION) else {
            return Ok(None);
        };
        serde_yaml_ng::from_value(section.clone())
            .map(Some)
            .map_err(|e| format!("the stored roles and assignments are damaged: {e}"))
    }

    /// what is wrong with `new` replacing these assignments when it is saved by someone with
    /// `grants`: only an administrator makes Administrators, and nobody gives a role more than
    /// they have themselves, whether by editing a role or by assigning one.
    pub fn escalation(&self, new: &Access, grants: &Grants, admin: bool) -> Option<String> {
        if admin {
            return None;
        }
        let before = self.all_roles();
        let beyond = |role: &Role| {
            Area::ALL
                .into_iter()
                .find(|a| role.level(*a) > grants.level(*a))
                .map(|a| {
                    format!(
                        "{} gives {} on {}, which you do not have yourself",
                        role.name,
                        role.level(a).title(),
                        a.title()
                    )
                })
        };
        if let Some(why) = new
            .roles
            .iter()
            .filter(|r| !before.contains(r))
            .find_map(beyond)
        {
            return Some(why);
        }
        let all = new.all_roles();
        for (user, names) in &new.members {
            let had = self.members.get(user);
            for name in names.iter().filter(|n| !had.is_some_and(|h| h.contains(n))) {
                if name == ADMINISTRATORS {
                    return Some("only an administrator can make someone an Administrator".into());
                }
                if let Some(why) = all.iter().find(|r| r.name == *name).and_then(beyond) {
                    return Some(why);
                }
            }
        }
        None
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
        assert_eq!(Access::from_settings(&legacy), Ok(None));
        let access = Access::migrate(&names(&["admin"]));
        assert_eq!(access.roles_of("admin", true), [ADMINISTRATORS]);
        assert_eq!(access.roles_of("dana", false), [USERS]);
        // stored and read back, nothing else in the settings changes
        let mut settings = legacy;
        access.store(&mut settings);
        let reread = Settings::parse(&settings.text()).unwrap();
        assert_eq!(Access::from_settings(&reread), Ok(Some(access)));
        assert!(settings.text().contains("zone: lab"));
        assert_eq!(
            Access::from_settings(&reread)
                .unwrap()
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

    #[test]
    fn admin_needs_manage_on_every_area() {
        assert!(Grants::all(Level::Manage).allows(Need::Admin));
        assert!(!Grants::all(Level::View).allows(Need::Admin));
        let mut nearly = Grants::all(Level::Manage);
        nearly.0.insert(Area::Updates, Level::View);
        assert!(!nearly.allows(Need::Admin));
        assert!(!Grants::default().allows(Need::Admin));
    }

    #[test]
    fn a_damaged_section_is_an_error_and_not_an_empty_set() {
        let bad = Settings::parse("access:\n  roles: 7\n").unwrap();
        assert!(Access::from_settings(&bad).unwrap_err().contains("damaged"));
    }

    #[test]
    fn sections_belong_to_the_area_that_changes_them() {
        assert_eq!(area_of_section("dns"), Area::Dns);
        assert_eq!(area_of_section("firewall"), Area::Firewall);
        assert_eq!(area_of_section("nt4-domain"), Area::Domain);
        assert_eq!(area_of_section("networks"), Area::Network);
        assert_eq!(area_of_section("access"), Area::Users);
        assert_eq!(area_of_section("docker"), Area::Services);
        assert_eq!(area_of_section("anything-new"), Area::Services);
    }

    fn with(user: &str, role: &str) -> Access {
        let mut a = Access::default();
        a.members.insert(user.into(), vec![role.into()]);
        a
    }

    #[test]
    fn only_an_administrator_makes_administrators_or_gives_more_than_they_have() {
        let users_manager = Grants::fallback(false);
        let old = Access::default();
        // an administrator may do anything
        let admins = with("dana", ADMINISTRATORS);
        assert_eq!(
            old.escalation(&admins, &Grants::fallback(false), true),
            None
        );
        // anyone else may not make an Administrator, even holding every right
        let everything = Grants::all(Level::Manage);
        assert!(old.escalation(&admins, &everything, false).is_some());
        // a role beyond the caller's own, defined or assigned
        let mut custom = with("dana", "Powerful");
        let mut role = Role {
            name: "Powerful".into(),
            grants: Default::default(),
        };
        role.grants.insert(Area::Updates, Level::Manage);
        custom.roles.push(role);
        assert!(old.escalation(&custom, &users_manager, false).is_some());
        assert_eq!(old.escalation(&custom, &everything, false), None);
        // assigning a role someone already had is not an escalation
        assert_eq!(custom.escalation(&custom, &users_manager, false), None);
        // an assignment within the caller's rights is fine
        assert_eq!(
            old.escalation(&with("dana", USERS), &users_manager, false),
            None
        );
    }
}
