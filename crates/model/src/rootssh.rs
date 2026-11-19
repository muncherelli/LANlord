//! who may sign in to the gateway as root over ssh: one admin-controlled setting with three
//! levels, kept as an sshd drop-in. lanlord leaves sshd alone until someone chooses a level.

use serde::{Deserialize, Serialize};

/// where lanlord's drop-in lives, in the folder sshd reads after the main file.
pub const DROPIN: &str = "/etc/ssh/sshd_config.d/10-lanlord-root.conf";

/// the first line of the drop-in, so a person reading it knows where it came from.
pub const DROPIN_HEADER: &str = "# managed by lanlord: root ssh access";

/// how far root may sign in over ssh.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootSsh {
    /// with a password or a key (`PermitRootLogin yes`).
    Allowed,
    /// with a key only (`PermitRootLogin prohibit-password`).
    KeysOnly,
    /// not at all (`PermitRootLogin no`).
    Off,
}

impl RootSsh {
    pub const ALL: [RootSsh; 3] = [RootSsh::Allowed, RootSsh::KeysOnly, RootSsh::Off];

    /// the value sshd's `PermitRootLogin` takes for this level.
    pub fn directive(self) -> &'static str {
        match self {
            RootSsh::Allowed => "yes",
            RootSsh::KeysOnly => "prohibit-password",
            RootSsh::Off => "no",
        }
    }

    /// the level a `PermitRootLogin` value means, when it is one of ours.
    pub fn from_directive(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.directive() == value)
    }

    pub fn label(self) -> &'static str {
        match self {
            RootSsh::Allowed => "Allowed (password or key)",
            RootSsh::KeysOnly => "Keys only (no password)",
            RootSsh::Off => "Off",
        }
    }

    /// the extra warning shown in the first-run setup; `has_key` says whether the new account was
    /// given an ssh key, which is what lets it sign in over ssh when root may not.
    pub fn setup_warning(self, has_key: bool) -> Option<&'static str> {
        match (self, has_key) {
            (RootSsh::Allowed, _) | (_, true) => None,
            (RootSsh::KeysOnly, false) => Some(
                "Warning: the new account has no ssh key, so if this server takes no ssh \
                 passwords the administrator cannot use ssh. Add a key for it (go back to \
                 step 1, or later in My Account) before you rely on ssh.",
            ),
            (RootSsh::Off, false) => Some(
                "Not possible when this server accepts no ssh passwords: the new account has no \
                 ssh key, so no one could sign in over ssh. The setup says so and goes back.",
            ),
        }
    }

    /// what choosing this level means in plain words, with the warning that matters.
    pub fn explain(self) -> &'static str {
        match self {
            RootSsh::Allowed => "Root may sign in over ssh with its password or a key.",
            RootSsh::KeysOnly => {
                "Root may sign in over ssh with a key only; its password is refused."
            }
            RootSsh::Off => {
                "Root may not sign in over ssh at all. That also stops LANlord clients that \
                 connect to this server as root."
            }
        }
    }

    /// the contents of the drop-in for this level.
    pub fn dropin(self) -> String {
        format!("{DROPIN_HEADER}\nPermitRootLogin {}\n", self.directive())
    }

    /// the level a drop-in written by [`RootSsh::dropin`] holds; `None` for anything else.
    pub fn from_dropin(text: &str) -> Option<Self> {
        let mut found = None;
        for line in text.lines().map(str::trim) {
            let mut words = line.split_whitespace();
            if words
                .next()
                .is_some_and(|k| k.eq_ignore_ascii_case("PermitRootLogin"))
            {
                let value = words.next()?;
                if words.next().is_some() || found.is_some() {
                    return None;
                }
                found = Self::from_directive(value);
                found?;
            }
        }
        found
    }
}

/// what the server says about root ssh access.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootSshInfo {
    /// the level lanlord set, `None` while the setting is left unmanaged.
    pub chosen: Option<RootSsh>,
    /// whether the gateway's sshd reads `/etc/ssh/sshd_config.d`; without it a level cannot be
    /// applied.
    pub dropins: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_level_maps_to_its_sshd_value() {
        let pairs = [
            (RootSsh::Allowed, "yes"),
            (RootSsh::KeysOnly, "prohibit-password"),
            (RootSsh::Off, "no"),
        ];
        for (level, value) in pairs {
            assert_eq!(level.directive(), value);
            assert_eq!(RootSsh::from_directive(value), Some(level));
        }
        assert_eq!(RootSsh::from_directive("without-password"), None);
    }

    #[test]
    fn the_drop_in_says_where_it_came_from_and_reads_back() {
        for level in RootSsh::ALL {
            let text = level.dropin();
            assert!(text.starts_with("# managed by lanlord"), "{text}");
            assert_eq!(
                text.lines().last(),
                Some(format!("PermitRootLogin {}", level.directive()).as_str())
            );
            assert_eq!(RootSsh::from_dropin(&text), Some(level));
        }
        assert_eq!(RootSsh::Off.dropin().matches("PermitRootLogin").count(), 1);
    }

    #[test]
    fn a_drop_in_that_is_not_ours_is_not_read() {
        assert_eq!(RootSsh::from_dropin(""), None);
        assert_eq!(RootSsh::from_dropin("PermitRootLogin maybe\n"), None);
        assert_eq!(
            RootSsh::from_dropin("PermitRootLogin yes\nPermitRootLogin no\n"),
            None
        );
        assert_eq!(RootSsh::from_dropin("PermitRootLogin yes extra\n"), None);
    }

    #[test]
    fn only_off_warns_about_clients() {
        assert!(RootSsh::Off.explain().contains("clients"));
        assert!(!RootSsh::KeysOnly.explain().contains("clients"));
    }
}
