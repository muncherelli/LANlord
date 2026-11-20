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

/// the line that marks a key lanlord added to an authorized keys file, kept just above it. keys
/// without it are somebody else's and are never touched.
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
}

impl SshAuthInfo {
    /// the accounts that could still sign in over ssh now only with a password: every
    /// administrator without a key, and root when it may sign in and has none.
    pub fn without_key(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .accounts
            .iter()
            .filter(|a| a.keys.is_empty())
            .map(|a| a.user.clone())
            .collect();
        if self.root_login && self.root_keys == 0 {
            names.push("root".into());
        }
        names
    }

    /// why passwords must not be turned off yet, when some account would be locked out.
    pub fn refusal(&self) -> Option<String> {
        let names = self.without_key();
        (!names.is_empty()).then(|| {
            format!(
                "turning ssh passwords off would lock out {} (no ssh key); add a key for each \
                 first, or confirm that you can still reach the server another way",
                names.join(", ")
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
