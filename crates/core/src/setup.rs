//! first-run setup of a gateway that has only root: create the first administrator and, when
//! asked, set root's ssh access. it runs only while no administrator other than root exists, so a
//! race, or a second try after the first worked, is refused.

use std::sync::Mutex;

use anyhow::Result;
use lanlord_model::operations::Reply;
use lanlord_model::rootssh::RootSsh;

use crate::access::Refused;
use crate::account;
use crate::admins::{ADMINS_GID, ADMINS_GROUP};
use crate::bootstrap::AskPassword;
use crate::log::{emit, Kind};
use crate::rootssh;
use crate::ssh::Session;

/// one setup at a time, so two of them cannot each find no administrator and both create one.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// the sh script that lists the members of `admins` (by the group's member list and by primary
/// group), one name per line.
fn members_script() -> String {
    format!(
        "getent group {ADMINS_GROUP} | cut -d: -f4 | tr ',' '\\n'\n\
         getent passwd | awk -F: '$4 == {ADMINS_GID} {{ print $1 }}'\n"
    )
}

/// the members of the admins group other than root, from the lines `members_script` printed.
fn others(out: &str) -> Vec<String> {
    let mut names: Vec<String> = out
        .lines()
        .map(str::trim)
        .filter(|n| !n.is_empty() && *n != "root")
        .map(str::to_string)
        .collect();
    names.sort();
    names.dedup();
    names
}

/// the administrators other than root, read from the server now.
pub fn other_admins(session: &Session) -> Result<Vec<String>> {
    let out = session.capture(&["sh".into(), "-c".into(), members_script()])?;
    Ok(others(&out))
}

/// why "off" is refused in the setup when this server takes no ssh passwords.
const LOCKOUT: &str =
    "this server's sshd does not accept ssh passwords and the new account has no \
ssh key yet, so turning root's ssh access off would leave no way to sign in over ssh: choose \
keys only, or add a key for the new account first";

/// create the administrator `name` with `password`, then set root's ssh access to `root_ssh`.
/// refused when an administrator other than root exists. an ssh setting that does
/// not take leaves the new account in place and says so in the reply's note.
pub fn first_admin(
    session: &Session,
    host: &str,
    name: &str,
    password: &str,
    root_ssh: RootSsh,
    ask: AskPassword,
) -> Result<Reply> {
    let _one = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !other_admins(session)?.is_empty() {
        return Err(
            Refused("an administrator already exists: sign in as that account".into()).into(),
        );
    }
    // the new account has no ssh key and, when sshd takes no passwords, root's ssh would be the
    // last way in over ssh: turning it off would leave no one. refused before anything is made.
    if root_ssh == RootSsh::Off && rootssh::passwords_refused(session, &mut *ask)? {
        return Err(Refused(LOCKOUT.into()).into());
    }
    account::create_admin(session, host, name, password, None, false, &mut *ask)?;
    let note = match rootssh::apply(session, root_ssh, ask) {
        Ok(()) => None,
        Err(error) => {
            emit(
                Kind::Err,
                format!("root's ssh access was not set: {error:#}"),
            );
            Some(format!(
                "The administrator was created, but root's ssh access was not changed: {error:#}"
            ))
        }
    };
    Ok(Reply::SetupDone { note })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn root_alone_is_no_other_admin() {
        assert!(others("root\nroot\n").is_empty());
        assert!(others("\n").is_empty());
        assert_eq!(others("root\nolive\n\nolive\n"), ["olive"]);
    }

    #[test]
    fn the_script_asks_for_the_group_by_name_and_by_gid() {
        let script = members_script();
        assert!(script.contains("getent group admins"));
        assert!(script.contains("$4 == 9998"));
    }

    fn no_password(_: &str) -> Result<String> {
        anyhow::bail!("no password expected")
    }

    /// a server whose admins group lists `members`, answering id and the rest quietly.
    fn server(members: &str) -> Session {
        Session::fake(&format!(
            r#"for a; do last=$a; done
case "$last" in
  *"id -un"*) printf 'root\n0\n' ;;
  *getent*) printf '%s\n' '{members}' ;;
  *) cat > /dev/null ;;
esac"#
        ))
    }

    #[test]
    fn an_existing_administrator_refuses_setup_before_anything_is_created() {
        let session = server("olive");
        let err = first_admin(
            &session,
            "gw",
            "pat",
            "pw",
            RootSsh::KeysOnly,
            &mut no_password,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("already exists"), "{err}");
        assert!(err.downcast_ref::<Refused>().is_some());
    }

    #[test]
    fn the_setup_door_runs_that_one_operation_and_nothing_else() {
        use crate::operations::{execute_setup, Operation};
        let session = server("");
        for operation in [
            Operation::Leases,
            Operation::LoadDocument,
            Operation::SetRootSsh {
                access: RootSsh::Off,
            },
            Operation::CreateAdmin {
                name: "pat".into(),
                password: lanlord_model::secret::Secret::new("pw"),
                ssh_key: None,
                passwordless_sudo: false,
            },
        ] {
            let err = execute_setup(&session, "gw", &operation, &mut no_password).unwrap_err();
            assert!(
                err.downcast_ref::<Refused>().is_some(),
                "{operation:?}: {err}"
            );
        }
        // a bad name never reaches the server
        let bad = Operation::SetupFirstAdmin {
            name: "root".into(),
            password: lanlord_model::secret::Secret::new("pw"),
            root_ssh: RootSsh::KeysOnly,
        };
        let err = execute_setup(&session, "gw", &bad, &mut no_password).unwrap_err();
        assert!(format!("{err}").contains("name"), "{err}");
    }

    #[test]
    fn setup_creates_the_account_and_sets_the_ssh_access_only_when_alone() {
        use crate::operations::{execute_setup, Operation};
        let session = server("");
        let operation = Operation::SetupFirstAdmin {
            name: "pat".into(),
            password: lanlord_model::secret::Secret::new("pw"),
            root_ssh: RootSsh::KeysOnly,
        };
        let reply = execute_setup(&session, "gw", &operation, &mut no_password).unwrap();
        assert_eq!(reply, Reply::SetupDone { note: None });
    }

    #[test]
    fn off_is_refused_when_sshd_takes_no_passwords_and_the_account_has_no_key() {
        let session = Session::fake(
            r#"for a; do last=$a; done
case "$last" in
  *"id -un"*) printf 'root\n0\n' ;;
  *getent*) printf '\n' ;;
  *) if grep -q 'tolower($1)=="passwordauthentication"'; then echo no; fi ;;
esac"#,
        );
        let err =
            first_admin(&session, "gw", "pat", "pw", RootSsh::Off, &mut no_password).unwrap_err();
        assert!(err.downcast_ref::<Refused>().is_some(), "{err}");
        assert!(format!("{err}").contains("no ssh key"), "{err}");
        // keys only is allowed there (with a warning in the page)
        let reply = first_admin(
            &session,
            "gw",
            "pat",
            "pw",
            RootSsh::KeysOnly,
            &mut no_password,
        )
        .unwrap();
        assert!(matches!(reply, Reply::SetupDone { .. }));
    }
}
