//! the first administrator, at the end of an install: a server with no LANlord administrator
//! other than root gets one by adopting an account that can already become root with sudo, or by
//! creating a new account, or the choice is skipped and root's first-run setup in the web ui does
//! it. the work is done by the operations the rest of lanlord uses (`admins`, `account`); this
//! module only decides, checks and refuses.

use anyhow::{bail, Result};
use base64::Engine;
use zeroize::Zeroizing;

use crate::account::{self, LoginUser};
use crate::admins;
use crate::bootstrap::{capture_as_root, valid_user, AskPassword};
use crate::log::{emit, Kind};
use crate::ssh::Session;

/// what to do about the first administrator.
#[derive(Clone, PartialEq, Eq)]
pub enum Choice {
    /// leave it: root signs in to the web ui and sets it up there.
    Skip,
    /// make this existing account an administrator; it must be able to become root with sudo.
    Adopt(String),
    /// create this account as an administrator.
    Create {
        name: String,
        password: Zeroizing<String>,
        ssh_public_key: Option<String>,
    },
}

// written by hand so a password never reaches a log line or a failed assertion
impl std::fmt::Debug for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Choice::Skip => f.write_str("Skip"),
            Choice::Adopt(user) => f.debug_tuple("Adopt").field(user).finish(),
            Choice::Create {
                name,
                ssh_public_key,
                ..
            } => f
                .debug_struct("Create")
                .field("name", name)
                .field("password", &"<redacted>")
                .field("ssh_public_key", ssh_public_key)
                .finish(),
        }
    }
}

/// asks what to do, given who the install signed in as.
pub type Decide<'a> = &'a mut dyn FnMut(&LoginUser) -> Result<Choice>;

/// whether the server has an administrator other than root.
pub(crate) fn has_admin(session: &Session, ask: AskPassword) -> Result<bool> {
    Ok(admins::list_with(session, ask)?.iter().any(|m| m != "root"))
}

/// whether `user` can become root with sudo on the server, read from sudo's own rules for that
/// account. the name travels encoded, not inside the script's text.
pub(crate) fn can_become_root(session: &Session, user: &str, ask: AskPassword) -> Result<bool> {
    if !valid_user(user) {
        bail!("not a valid user name: {user:?}");
    }
    let name = base64::engine::general_purpose::STANDARD.encode(user);
    let script = format!(
        r#"u=$(printf %s '{name}' | base64 -d)
if ! id -u "$u" >/dev/null 2>&1; then echo missing; exit 0; fi
if sudo -l -U "$u" 2>/dev/null | grep -Eq '\(ALL( : ALL)?\) +(NOPASSWD: *)?ALL'; then echo yes; else echo no; fi
"#
    );
    let admin = account::login_user(session)?;
    match capture_as_root(session, &admin.name, script, ask)?.trim() {
        "yes" => Ok(true),
        "no" => Ok(false),
        "missing" => bail!("the user {user} does not exist on the server"),
        _ => bail!("the server did not say whether {user} can use sudo"),
    }
}

/// carry out `choice`; returns the administrator's name, or `None` when it was skipped. an
/// account that cannot become root is refused before anything is changed.
pub(crate) fn carry_out(
    session: &Session,
    host: &str,
    choice: &Choice,
    ask: AskPassword,
) -> Result<Option<String>> {
    match choice {
        Choice::Skip => Ok(None),
        Choice::Adopt(user) => {
            if user == "root" {
                bail!("root is not an account to adopt; skip, or create a new administrator");
            }
            if !can_become_root(session, user, &mut *ask)? {
                bail!(
                    "{user} cannot become root with sudo on this server, so it was not made an \
                     administrator; add it to the sudo group (or wheel) first, or create a new \
                     account instead"
                );
            }
            admins::change_with(session, &admins::Action::Add(user.clone()), ask)?;
            Ok(Some(user.clone()))
        }
        Choice::Create {
            name,
            password,
            ssh_public_key,
        } => {
            account::create_admin(
                session,
                host,
                name,
                password.as_str(),
                ssh_public_key.as_deref(),
                false,
                &mut *ask,
            )?;
            // the account is in admins, and bootstrap's sudoers rule lets that group in; check
            // that it holds here rather than leave an administrator who cannot use sudo
            if !can_become_root(session, name, ask)? {
                bail!(
                    "created {name}, but it cannot become root with sudo on this server; check \
                     /etc/sudoers.d/admins"
                );
            }
            Ok(Some(name.clone()))
        }
    }
}

/// the whole step: nothing when an administrator exists, otherwise ask `decide` and carry it
/// out.
pub(crate) fn run(
    session: &Session,
    host: &str,
    decide: Decide,
    ask: AskPassword,
) -> Result<Option<String>> {
    if has_admin(session, &mut *ask)? {
        return Ok(None);
    }
    let login = account::login_user(session)?;
    let choice = decide(&login)?;
    if choice == Choice::Skip {
        emit(Kind::Info, "no administrator was set up");
    }
    carry_out(session, host, &choice, ask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_never_shows_the_password() {
        let choice = Choice::Create {
            name: "olive".into(),
            password: Zeroizing::new("correct horse".into()),
            ssh_public_key: None,
        };
        let shown = format!("{choice:?}");
        assert!(!shown.contains("correct horse"), "{shown}");
        assert!(
            shown.contains("olive") && shown.contains("redacted"),
            "{shown}"
        );
    }
}
