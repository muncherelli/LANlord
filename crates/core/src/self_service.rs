//! the ops about accounts and permissions: a person's own account, password changes across
//! linked accounts, and the role assignments.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};

use lanlord_model::identity::{AccountInfo, PasswordReport};
use lanlord_model::ops::Reply;
use lanlord_model::secret::Secret;

use crate::access::{self, Access, Area, Caller, Grants, Profile};
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
    // without root on the server the stored assignments cannot be read: show the defaults
    let access = access::assignments(session, &mut *ask).unwrap_or_default();
    let installed = domain_installed(session, &mut *ask).unwrap_or(false);
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

/// how many wrong old passwords are free before the waiting starts, and the waits.
const FREE_GUESSES: u32 = 5;
const FIRST_WAIT: Duration = Duration::from_secs(60);
const LONGEST_WAIT: Duration = Duration::from_secs(15 * 60);

/// wrong old passwords per account: how many in a row, and when the last one was.
static GUESSES: Mutex<BTreeMap<String, (u32, Instant)>> = Mutex::new(BTreeMap::new());

/// how long `user` must still wait before the next guess, given `fails` in a row, the last at `last`.
fn wait_left(fails: u32, last: Instant, now: Instant) -> Option<Duration> {
    let over = fails.checked_sub(FREE_GUESSES)?;
    let wait = FIRST_WAIT
        .saturating_mul(2u32.saturating_pow(over))
        .min(LONGEST_WAIT);
    wait.checked_sub(now.saturating_duration_since(last))
        .filter(|left| !left.is_zero())
}

fn guesses() -> std::sync::MutexGuard<'static, BTreeMap<String, (u32, Instant)>> {
    GUESSES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn check_not_locked(user: &str, now: Instant) -> Result<()> {
    match guesses().get(user) {
        Some(&(fails, last)) => match wait_left(fails, last, now) {
            Some(left) => bail!(
                "too many wrong passwords; try again in {} seconds",
                left.as_secs() + 1
            ),
            None => Ok(()),
        },
        None => Ok(()),
    }
}

fn note_guess(user: &str, right: bool, now: Instant) {
    let mut book = guesses();
    if right {
        book.remove(user);
    } else {
        let entry = book.entry(user.to_string()).or_insert((0, now));
        *entry = (entry.0 + 1, now);
    }
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
    check_not_locked(&caller.user, Instant::now())?;
    let right = identity::verify_password(session, &admin, &caller.user, old, &mut *ask)
        .map_err(|e| e.context("the password cannot be checked without the lanlord service"))?;
    note_guess(&caller.user, right, Instant::now());
    if !right {
        bail!("the old password is not correct");
    }
    let report = propagate(session, &caller.user, new, Some(old), ask)?;
    Ok(Reply::Password(report))
}

/// accounts that are the machine's own and never have their password reset from here.
const SYSTEM_ACCOUNTS: [&str; 2] = ["root", "lanlord"];

/// the lowest uid of a person; below it are the system accounts.
const FIRST_PERSON: u32 = 1000;

fn refuse(why: impl Into<String>) -> anyhow::Error {
    access::Refused(why.into()).into()
}

/// the groups of `user`; no answer is a refusal, never an empty list.
fn groups_of(session: &Session, user: &str) -> Result<Vec<String>> {
    let out = session
        .capture(&["id".into(), "-nG".into(), user.into()])
        .map_err(|_| refuse(format!("the groups of {user} cannot be read")))?;
    Ok(out.split_whitespace().map(str::to_string).collect())
}

/// whether a password reset of `user` is allowed for `who`: never a system account, and only an
/// administrator resets someone who can do more than the caller can.
fn check_target(
    session: &Session,
    who: (&Caller, &Grants),
    user: &str,
    ask: AskPassword,
) -> Result<()> {
    if SYSTEM_ACCOUNTS.contains(&user) {
        return Err(refuse(format!("{user} is a system account")));
    }
    let uid = session
        .capture(&["id".into(), "-u".into(), user.into()])
        .ok()
        .and_then(|out| out.trim().parse::<u32>().ok())
        .ok_or_else(|| refuse(format!("there is no account {user}")))?;
    if uid < FIRST_PERSON {
        return Err(refuse(format!("{user} is a system account")));
    }
    let (caller, grants) = who;
    let is_admin = groups_of(session, user)?
        .iter()
        .any(|g| g == admins::ADMINS_GROUP);
    if caller.admin {
        return Ok(());
    }
    let theirs = access::assignments(session, ask)?.grants(user, is_admin);
    match Area::ALL
        .into_iter()
        .find(|a| theirs.level(*a) > grants.level(*a))
    {
        Some(area) => Err(refuse(format!(
            "{user} may do more in {} than you; only an administrator resets them",
            area.title()
        ))),
        None => Ok(()),
    }
}

pub(crate) fn reset_password(
    session: &Session,
    who: (&Caller, &Grants),
    user: &str,
    new: &Secret,
    ask: AskPassword,
) -> Result<Reply> {
    if !crate::bootstrap::valid_user(user) {
        return Err(refuse(format!("{user:?} is not a user name")));
    }
    check_target(session, who, user, &mut *ask)?;
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
    who: (&Caller, &Grants),
    access: &Access,
    ask: AskPassword,
) -> Result<Reply> {
    if let Some(problem) = access.problem() {
        bail!("{problem}");
    }
    let (settings, stored) = access::load(session, &mut *ask)?;
    let (caller, grants) = who;
    if let Some(why) = stored
        .unwrap_or_default()
        .escalation(access, grants, caller.admin)
    {
        return Err(refuse(why));
    }
    save(session, host, settings, access, ask)?;
    Ok(Reply::Done)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::testgw::{no_password, Gateway};
    use lanlord_model::access::{Level, Role};
    use lanlord_model::identity::LinkState;

    fn root() -> Caller {
        Caller {
            user: "ann".into(),
            admin: true,
        }
    }

    fn operator() -> Caller {
        Caller {
            user: "olly".into(),
            admin: false,
        }
    }

    /// a reset by `who` with the grants their role gives: all for an administrator, a user's else.
    fn reset(session: &Session, who: &Caller, user: &str) -> Result<Reply> {
        let grants = Grants::fallback(who.admin);
        reset_password(
            session,
            (who, &grants),
            user,
            &secret("new"),
            &mut no_password,
        )
    }

    #[test]
    fn nobody_resets_someone_who_may_do_more_than_they_do() {
        let gw = Gateway::new("self-reset-more");
        std::fs::write(
            gw.dir.join("yml"),
            "access:\n  members:\n    dana: [Operators]\n    eve: [Users]\n",
        )
        .unwrap();
        // dana is an operator: more than a user may do
        assert!(is_refused(&reset(&gw.session, &operator(), "dana")));
        assert!(reset(&gw.session, &root(), "dana").is_ok());
        assert!(reset(&gw.session, &operator(), "eve").is_ok());
    }

    #[test]
    fn a_name_that_is_not_one_and_groups_that_cannot_be_read_are_refused() {
        let gw = Gateway::new("self-reset-closed");
        for who in [root(), operator()] {
            assert!(is_refused(&reset(&gw.session, &who, "a b;c")));
            assert!(is_refused(&reset(&gw.session, &who, "flaky")));
        }
    }

    fn is_refused<T>(result: &Result<T>) -> bool {
        result
            .as_ref()
            .err()
            .is_some_and(|e| e.downcast_ref::<access::Refused>().is_some())
    }

    #[test]
    fn nobody_resets_a_system_account_or_one_that_does_not_exist() {
        let gw = Gateway::new("self-reset-system");
        for name in ["root", "lanlord", "svc", "ghost"] {
            for who in [root(), operator()] {
                let result = reset(&gw.session, &who, name);
                assert!(is_refused(&result), "{name} by {}", who.user);
            }
        }
    }

    #[test]
    fn only_an_administrator_resets_an_administrator() {
        let gw = Gateway::new("self-reset-admin");
        let by_operator = reset(&gw.session, &operator(), "boss");
        assert!(is_refused(&by_operator));
        let by_admin = reset(&gw.session, &root(), "boss");
        assert!(by_admin.is_ok(), "{by_admin:?}");
        // an operator may reset an ordinary person
        let person = reset(&gw.session, &operator(), "dana");
        assert!(person.is_ok(), "{person:?}");
    }

    #[test]
    fn only_an_administrator_saves_administrators_or_roles_beyond_their_own() {
        let gw = Gateway::new("self-access-escalate");
        let mut access = Access::default();
        access.members.insert(
            "olly".into(),
            vec![lanlord_model::access::ADMINISTRATORS.into()],
        );
        let users_manager = Grants::fallback(false);
        let result = save_access(
            &gw.session,
            "gw",
            (&operator(), &users_manager),
            &access,
            &mut no_password,
        );
        assert!(is_refused(&result), "{result:?}");
        assert!(gw.plain().is_none(), "nothing was stored");
        let result = save_access(
            &gw.session,
            "gw",
            (&root(), &Grants::all(Level::Manage)),
            &access,
            &mut no_password,
        );
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn wrong_guesses_cost_waiting_that_grows_and_a_right_one_clears_it() {
        let then = Instant::now();
        let later = |s: u64| then + Duration::from_secs(s);
        assert_eq!(wait_left(FREE_GUESSES - 1, then, then), None);
        assert_eq!(wait_left(FREE_GUESSES, then, then), Some(FIRST_WAIT));
        assert_eq!(
            wait_left(FREE_GUESSES, then, later(10)),
            Some(Duration::from_secs(50))
        );
        assert_eq!(wait_left(FREE_GUESSES, then, later(61)), None);
        assert_eq!(
            wait_left(FREE_GUESSES + 1, then, then),
            Some(FIRST_WAIT * 2)
        );
        assert_eq!(wait_left(60, then, then), Some(LONGEST_WAIT));
        let now = Instant::now();
        for _ in 0..FREE_GUESSES {
            note_guess("lockout-test", false, now);
        }
        assert!(check_not_locked("lockout-test", now).is_err());
        assert!(check_not_locked("someone-else", now).is_ok());
        note_guess("lockout-test", true, now);
        assert!(check_not_locked("lockout-test", now).is_ok());
    }

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
  "id -u svc") echo 998 ;;
  "id -u ghost") exit 1 ;;
  "id -u flaky") echo 1005 ;;
  "id -nG flaky") exit 1 ;;
  "id -u "*) echo 1001 ;;
  "id -nG boss") echo "boss admins" ;;
  "id -nG "*) echo "users" ;;
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
        let reply = reset(&server, &root(), "dana").unwrap();
        assert_eq!(the_links(reply), (true, vec![LinkState::Changed]));
        // with the domain component installed there is a second link that does nothing yet
        std::fs::write(gw.dir.join("yml"), "nt4-domain:\n  realm: EXAMPLE\n").unwrap();
        let reply = reset(&gw.session, &root(), "dana").unwrap();
        let (ok, states) = the_links(reply);
        assert!(ok);
        assert_eq!(states.len(), 2);
        assert!(matches!(states[1], LinkState::Skipped(_)));
        let _ = std::fs::remove_file(&log);
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
            save_access(
                &gw.session,
                "gw",
                (&root(), &Grants::all(Level::Manage)),
                &access,
                &mut no_password
            )
            .unwrap(),
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
        assert!(save_access(
            &gw.session,
            "gw",
            (&root(), &Grants::all(Level::Manage)),
            &bad,
            &mut no_password
        )
        .is_err());
    }
}
