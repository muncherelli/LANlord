//! the ops about accounts and permissions: a person's own account, password changes across
//! linked accounts, and the role assignments.

use anyhow::{bail, Result};

use lanlord_model::identity::{AccountInfo, PasswordReport};
use lanlord_model::ops::Reply;
use lanlord_model::secret::Secret;

use crate::access::{self, Access, Caller, Profile};
use crate::bootstrap::AskPassword;
use crate::ssh::Session;
use crate::{admins, config, identity};

/// the longest profile field.
const MAX_FIELD: usize = 128;

/// the gateway account the session signs in as, for the root commands.
fn admin_of(session: &Session) -> Result<String> {
    config::admin(session)
}

fn domain_installed(session: &Session, ask: AskPassword) -> Result<bool> {
    let (settings, _) = access::load(session, ask)?;
    Ok(settings.section("nt4-domain").is_some())
}

pub(crate) fn account_info(session: &Session, caller: &Caller, ask: AskPassword) -> Result<Reply> {
    let access = access::assignments(session, &mut *ask)?;
    let installed = domain_installed(session, &mut *ask)?;
    let admin = admin_of(session)?;
    let links = identity::links_for(session, &admin, installed, ask);
    Ok(Reply::Account(AccountInfo {
        user: caller.user.clone(),
        profile: access
            .profiles
            .get(&caller.user)
            .cloned()
            .unwrap_or_default(),
        roles: access.roles_of(&caller.user, caller.admin),
        grants: access.grants(&caller.user, caller.admin),
        links: identity::infos(&links),
    }))
}

/// set `new` for `user` on every linked account.
fn propagate(
    session: &Session,
    user: &str,
    new: &Secret,
    old: Option<&Secret>,
    ask: AskPassword,
) -> Result<PasswordReport> {
    identity::check_policy(old, new)?;
    let admin = admin_of(session)?;
    let installed = domain_installed(session, &mut *ask)?;
    let links = identity::links_for(session, &admin, installed, ask);
    Ok(identity::change_all(&links, user, new))
}

pub(crate) fn change_password(
    session: &Session,
    caller: &Caller,
    old: &Secret,
    new: &Secret,
    ask: AskPassword,
) -> Result<Reply> {
    identity::check_policy(Some(old), new)?;
    let admin = admin_of(session)?;
    if !identity::verify_password(session, &admin, &caller.user, old, &mut *ask)? {
        bail!("the old password is not correct");
    }
    let report = propagate(session, &caller.user, new, Some(old), ask)?;
    Ok(Reply::Password(report))
}

pub(crate) fn reset_password(
    session: &Session,
    user: &str,
    new: &Secret,
    ask: AskPassword,
) -> Result<Reply> {
    Ok(Reply::Password(propagate(session, user, new, None, ask)?))
}

fn check_field(name: &str, value: &str) -> Result<()> {
    if value.chars().count() > MAX_FIELD || value.chars().any(char::is_control) {
        bail!("{name} is too long or has control characters");
    }
    Ok(())
}

fn save(
    session: &Session,
    host: &str,
    mut settings: lanlord_model::settings::Settings,
    access: &Access,
    ask: AskPassword,
) -> Result<()> {
    access.store(&mut settings);
    config::push_plain_with(session, host, settings.text().as_bytes(), ask)
}

pub(crate) fn update_profile(
    session: &Session,
    host: &str,
    caller: &Caller,
    profile: &Profile,
    ask: AskPassword,
) -> Result<Reply> {
    check_field("the full name", &profile.full_name)?;
    check_field("the description", &profile.description)?;
    let (settings, access) = access::load(session, &mut *ask)?;
    let mut access = access.unwrap_or_default();
    access.profiles.insert(caller.user.clone(), profile.clone());
    save(session, host, settings, &access, ask)?;
    Ok(Reply::Done)
}

pub(crate) fn load_access(session: &Session, ask: AskPassword) -> Result<Reply> {
    let (_, access) = access::load(session, &mut *ask)?;
    let access = match access {
        Some(access) => access,
        // not migrated yet: the admins become Administrators, everyone else stays a User
        None => Access::migrate(&admins::list_with(session, ask)?),
    };
    Ok(Reply::Access(access))
}

pub(crate) fn save_access(
    session: &Session,
    host: &str,
    access: &Access,
    ask: AskPassword,
) -> Result<Reply> {
    if let Some(problem) = access.problem() {
        bail!("{problem}");
    }
    let (settings, _) = access::load(session, &mut *ask)?;
    save(session, host, settings, access, ask)?;
    Ok(Reply::Done)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::testgw::{no_password, Gateway};
    use lanlord_model::access::Role;
    use lanlord_model::identity::LinkState;

    fn dana() -> Caller {
        Caller {
            user: "dana".into(),
            admin: false,
        }
    }

    /// a server that says whether the helper accepts the old password and reports the old hash.
    fn passwd_server(helper_says: &str, log: &std::path::Path) -> Session {
        Session::fake(&format!(
            r#"for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sudo -n true") exit 0 ;;
  "sh -c "*) exit 0 ;;
  *)
input=$(cat)
case "$input" in
  *lanlord-auth-helper*) echo {helper_says} ;;
  *chpasswd*) echo 'OLD:hash'; echo set >> '{log}' ;;
esac ;;
esac"#,
            log = log.display()
        ))
    }

    fn secret(text: &str) -> Secret {
        Secret::new(text)
    }

    fn the_links(reply: Reply) -> (bool, Vec<LinkState>) {
        match reply {
            Reply::Password(report) => (
                report.ok,
                report.links.into_iter().map(|l| l.state).collect(),
            ),
            other => panic!("not a password report: {other:?}"),
        }
    }

    #[test]
    fn the_right_old_password_changes_the_linux_account() {
        let log = std::env::temp_dir().join(format!("lanlord-pw-ok-{}", std::process::id()));
        let _ = std::fs::remove_file(&log);
        let server = passwd_server("ok", &log);
        let reply = change_password(
            &server,
            &dana(),
            &secret("old"),
            &secret("new"),
            &mut no_password,
        )
        .unwrap();
        assert_eq!(the_links(reply), (true, vec![LinkState::Changed]));
        assert!(std::fs::read_to_string(&log).unwrap().contains("set"));
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn a_wrong_old_password_changes_nothing() {
        let log = std::env::temp_dir().join(format!("lanlord-pw-bad-{}", std::process::id()));
        let _ = std::fs::remove_file(&log);
        let server = passwd_server("no", &log);
        let err = change_password(
            &server,
            &dana(),
            &secret("old"),
            &secret("new"),
            &mut no_password,
        )
        .unwrap_err();
        assert!(err.to_string().contains("not correct"), "{err:#}");
        assert!(!log.exists());
    }

    #[test]
    fn a_bad_new_password_is_refused_before_anything_is_asked() {
        let server = passwd_server("ok", std::path::Path::new("/nonexistent/log"));
        for (old, new) in [("old", ""), ("same", "same"), ("old", "two\nlines")] {
            let result = change_password(
                &server,
                &dana(),
                &secret(old),
                &secret(new),
                &mut no_password,
            );
            assert!(result.is_err(), "{new:?}");
        }
    }

    #[test]
    fn an_administrator_resets_someone_else_and_the_domain_link_is_skipped_until_it_exists() {
        let gw = Gateway::new("self-reset");
        let log = std::env::temp_dir().join(format!("lanlord-pw-reset-{}", std::process::id()));
        let _ = std::fs::remove_file(&log);
        let server = passwd_server("ok", &log);
        let reply = reset_password(&server, "dana", &secret("new"), &mut no_password).unwrap();
        assert_eq!(the_links(reply), (true, vec![LinkState::Changed]));
        // with the domain component installed there is a second link that does nothing yet
        std::fs::write(gw.dir.join("yml"), "nt4-domain:\n  realm: EXAMPLE\n").unwrap();
        let reply = reset_password(&gw.session, "dana", &secret("new"), &mut no_password).unwrap();
        let (ok, states) = the_links(reply);
        assert!(ok);
        assert_eq!(states.len(), 2);
        assert!(matches!(states[1], LinkState::Skipped(_)));
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn a_user_name_that_is_not_one_fails_the_link_and_the_report() {
        let gw = Gateway::new("self-reset-bad");
        let reply = reset_password(&gw.session, "a b;c", &secret("new"), &mut no_password).unwrap();
        let (ok, states) = the_links(reply);
        assert!(!ok);
        assert!(matches!(states[0], LinkState::Failed(_)));
    }

    #[test]
    fn a_profile_is_stored_for_its_owner_and_shows_in_their_account() {
        let gw = Gateway::new("self-profile");
        let profile = Profile {
            full_name: "Dana Example".into(),
            description: "front desk".into(),
        };
        let done = update_profile(&gw.session, "gw", &dana(), &profile, &mut no_password).unwrap();
        assert_eq!(done, Reply::Done);
        let Reply::Account(info) = account_info(&gw.session, &dana(), &mut no_password).unwrap()
        else {
            panic!("not an account");
        };
        assert_eq!(info.user, "dana");
        assert_eq!(info.profile, profile);
        assert_eq!(info.roles, [lanlord_model::access::USERS]);
        assert_eq!(info.links.len(), 1);
        // nobody else's account got the profile
        let Reply::Account(other) = account_info(
            &gw.session,
            &Caller {
                user: "ann".into(),
                admin: false,
            },
            &mut no_password,
        )
        .unwrap() else {
            panic!("not an account");
        };
        assert_eq!(other.profile, Profile::default());
    }

    #[test]
    fn profile_fields_that_are_too_long_or_have_control_characters_are_refused() {
        let gw = Gateway::new("self-profile-bad");
        for profile in [
            Profile {
                full_name: "x".repeat(MAX_FIELD + 1),
                description: String::new(),
            },
            Profile {
                full_name: String::new(),
                description: "line\nbreak".into(),
            },
        ] {
            let result = update_profile(&gw.session, "gw", &dana(), &profile, &mut no_password);
            assert!(result.is_err());
        }
        assert!(gw.plain().is_none(), "nothing was stored");
    }

    #[test]
    fn the_assignments_round_trip_and_a_bad_set_is_refused() {
        let gw = Gateway::new("self-access");
        // never stored: the migration answers, and it is valid
        let Reply::Access(fresh) = load_access(&gw.session, &mut no_password).unwrap() else {
            panic!("not access");
        };
        assert_eq!(fresh.problem(), None);
        let mut access = fresh;
        access
            .members
            .insert("dana".into(), vec![lanlord_model::access::OPERATORS.into()]);
        assert_eq!(
            save_access(&gw.session, "gw", &access, &mut no_password).unwrap(),
            Reply::Done
        );
        let Reply::Access(back) = load_access(&gw.session, &mut no_password).unwrap() else {
            panic!("not access");
        };
        assert_eq!(back.members["dana"], [lanlord_model::access::OPERATORS]);
        let mut bad = back;
        bad.roles.push(Role {
            name: lanlord_model::access::USERS.into(),
            grants: Default::default(),
        });
        assert!(save_access(&gw.session, "gw", &bad, &mut no_password).is_err());
    }
}
