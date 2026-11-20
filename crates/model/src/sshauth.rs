//! how people may sign in to the gateway over ssh: whether sshd takes passwords, kept as an sshd
//! drop-in next to the one for root, and the authorized keys of the administrator accounts that
//! make turning passwords off safe. lanlord leaves sshd alone until an administrator chooses.

use serde::{Deserialize, Serialize};

/// where lanlord's drop-in lives, in the folder sshd reads after the main file. it sorts after
/// the root one (`10-lanlord-root.conf`) and before the ones distributions ship, so sshd, which
/// keeps the first value it reads, uses it.
pub const DROPIN: &str = "/etc/ssh/sshd_config.d/11-lanlord-auth.conf";

/// the first line of the drop-in, so a person reading it knows where it came from.
pub const DROPIN_HEADER: &str = "# managed by lanlord: ssh password sign-in";

/// the line that marks a key lanlord added to an authorized keys file, kept just above it.
/// removing such a key removes its mark too; keys without it can be removed as well.
pub const KEY_MARK: &str = "# added by lanlord";

/// the contents of the drop-in. with passwords off the keyboard-interactive method, which pam may
/// answer with the password, is closed too.
pub fn dropin(passwords: bool) -> String {
    if passwords {
        format!("{DROPIN_HEADER}\nPasswordAuthentication yes\n")
    } else {
        format!("{DROPIN_HEADER}\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n")
    }
}

/// whether passwords are on, as a drop-in written by [`dropin`] says; `None` for anything else.
pub fn from_dropin(text: &str) -> Option<bool> {
    let mut found = None;
    for line in text.lines().map(str::trim) {
        let mut words = line.split_whitespace();
        if words
            .next()
            .is_some_and(|k| k.eq_ignore_ascii_case("PasswordAuthentication"))
        {
            if found.is_some() {
                return None;
            }
            found = match words.next()? {
                "yes" => Some(true),
                "no" => Some(false),
                _ => return None,
            };
        }
    }
    found
}

/// the type and the base64 field of a key line, ignoring options, comment and spacing: what makes
/// two lines the same key.
pub fn key_id(line: &str) -> Option<String> {
    let mut words = line.split_whitespace();
    let kind = words.by_ref().find(|w| {
        ["ssh-", "ecdsa-", "sk-ssh-", "sk-ecdsa-"]
            .iter()
            .any(|p| w.starts_with(p))
    })?;
    Some(format!("{kind} {}", words.next()?))
}

/// the authorized keys of one administrator account, one public key line each.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountKeys {
    pub user: String,
    pub keys: Vec<String>,
}

/// what the server says about ssh sign-in.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshAuthInfo {
    /// whether lanlord's drop-in sets passwords on or off; `None` while it is left unmanaged.
    pub chosen: Option<bool>,
    /// what sshd would do now, when it could be asked.
    pub passwords: Option<bool>,
    /// whether the gateway's sshd reads `/etc/ssh/sshd_config.d`; without it nothing can be set.
    pub dropins: bool,
    /// the administrator accounts and their keys.
    pub accounts: Vec<AccountKeys>,
    /// whether root may sign in over ssh at all.
    pub root_login: bool,
    /// how many keys root has.
    pub root_keys: usize,
    /// sshd reads keys from somewhere other than the default files, so the lists here say
    /// nothing about who can sign in with a key.
    #[serde(default)]
    pub keys_unknown: bool,
}

impl SshAuthInfo {
    /// the accounts that could still sign in over ssh now only with a password: every
    /// administrator without a key, and root when it may sign in and has none.
    pub fn without_key(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .accounts
            .iter()
            .filter(|a| self.keys_unknown || a.keys.is_empty())
            .map(|a| a.user.clone())
            .collect();
        if self.root_login && (self.keys_unknown || self.root_keys == 0) {
            names.push("root".into());
        }
        names
    }

    /// why passwords must not be turned off yet, when some account would be locked out.
    pub fn refusal(&self) -> Option<String> {
        let names = self.without_key();
        (!names.is_empty()).then(|| {
            let why = if self.keys_unknown {
                "sshd reads keys from files lanlord does not check"
            } else {
                "no ssh key"
            };
            format!(
                "turning ssh passwords off would lock out {} ({why}); add a key for each \
                 first, or confirm that you can still reach the server another way",
                names.join(", ")
            )
        })
    }

    /// whether passwords are off now (or chosen off, while sshd could not be asked).
    fn passwords_off(&self) -> bool {
        self.passwords.or(self.chosen) == Some(false)
    }

    /// why the key `id` (see [`key_id`]) of `user` must not be removed yet: passwords are off
    /// and it is the last key of the account, or the keys cannot be checked.
    pub fn removal_refusal(&self, user: &str, id: &str) -> Option<String> {
        if !self.passwords_off() {
            return None;
        }
        let keys = self.keys_of(user)?;
        let same = |k: &String| key_id(k).as_deref() == Some(id);
        let last = keys.iter().any(same) && keys.iter().all(same);
        (self.keys_unknown || last).then(|| {
            format!(
                "passwords are off and this is the last ssh key of {user}; without it {user} \
                 could no longer sign in over ssh. add another key first, or confirm that you \
                 can still reach the server another way"
            )
        })
    }

    /// the keys of the account `user`, if it is one of the administrators.
    pub fn keys_of(&self, user: &str) -> Option<&[String]> {
        self.accounts
            .iter()
            .find(|a| a.user == user)
            .map(|a| a.keys.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(accounts: &[(&str, usize)], root: (bool, usize)) -> SshAuthInfo {
        SshAuthInfo {
            accounts: accounts
                .iter()
                .map(|(user, n)| AccountKeys {
                    user: user.to_string(),
                    keys: (0..*n).map(|i| format!("ssh-ed25519 AAAA{i} k")).collect(),
                })
                .collect(),
            root_login: root.0,
            root_keys: root.1,
            ..SshAuthInfo::default()
        }
    }

    #[test]
    fn the_dropin_round_trips_and_other_text_is_not_ours() {
        assert_eq!(from_dropin(&dropin(true)), Some(true));
        assert_eq!(from_dropin(&dropin(false)), Some(false));
        assert!(dropin(false).contains("KbdInteractiveAuthentication no"));
        assert_eq!(from_dropin(""), None);
        assert_eq!(from_dropin("PasswordAuthentication maybe\n"), None);
        assert_eq!(
            from_dropin("PasswordAuthentication yes\nPasswordAuthentication no\n"),
            None
        );
        assert_eq!(from_dropin("passwordauthentication no\n"), Some(false));
    }

    #[test]
    fn passwords_may_go_off_only_when_every_administrator_has_a_key() {
        let all = info(&[("dana", 1), ("pat", 2)], (false, 0));
        assert!(all.refusal().is_none());
        let some = info(&[("dana", 1), ("pat", 0)], (false, 0));
        let why = some.refusal().unwrap();
        assert!(why.contains("pat") && !why.contains("dana"), "{why}");
        assert_eq!(some.keys_of("pat"), Some(&[][..]));
        assert_eq!(some.keys_of("nobody"), None);
    }

    #[test]
    fn the_last_key_stays_while_passwords_are_off() {
        let mut one = info(&[("dana", 1), ("pat", 2)], (false, 0));
        let id = key_id(&one.accounts[0].keys[0]).unwrap();
        // passwords on: nothing to protect
        one.passwords = Some(true);
        assert!(one.removal_refusal("dana", &id).is_none());
        one.passwords = Some(false);
        assert!(one.removal_refusal("dana", &id).unwrap().contains("dana"));
        // pat has two keys: one may go
        let pat = key_id(&one.accounts[1].keys[0]).unwrap();
        assert!(one.removal_refusal("pat", &pat).is_none());
        // keys sshd reads elsewhere cannot be counted
        one.keys_unknown = true;
        assert!(one.removal_refusal("pat", &pat).is_some());
        assert!(one.refusal().unwrap().contains("does not check"));
        assert_eq!(one.without_key(), ["dana", "pat"]);
    }

    #[test]
    fn a_key_id_ignores_options_and_the_comment() {
        let plain = "ssh-ed25519 AAAAC3 pat@laptop";
        let prefixed = r#"from="192.0.2.7",command="true" ssh-ed25519 AAAAC3 other"#;
        assert_eq!(key_id(plain), key_id(prefixed));
        assert_eq!(key_id(plain).as_deref(), Some("ssh-ed25519 AAAAC3"));
        assert_eq!(key_id("not a key"), None);
    }

    #[test]
    fn root_counts_only_while_it_may_sign_in() {
        assert_eq!(
            info(&[("dana", 1)], (true, 0)).without_key(),
            ["root".to_string()]
        );
        assert!(info(&[("dana", 1)], (true, 1)).refusal().is_none());
        assert!(info(&[("dana", 1)], (false, 0)).refusal().is_none());
        // no administrator at all has no one to lock out but root
        assert!(info(&[], (false, 0)).refusal().is_none());
    }
}
