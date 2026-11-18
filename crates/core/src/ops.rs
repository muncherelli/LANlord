//! runs an [`Op`] against a server with the same functions the cli and the windows use, so
//! every front end gets the same behaviour from one description of what it wants.

use anyhow::{anyhow, bail, Result};

pub use lanlord_model::ops::*;

use crate::access::{Caller, Grants, Need};
use crate::bootstrap::{self, AnsibleMode, AskPassword, Options};
use crate::purge::PurgeOptions;
use crate::server;
use crate::ssh::Session;
use crate::{access, save_guard, self_service, settings, users};
use crate::{
    account, apply, clock, crypto, doctor, leases, logon, netstatus, network, nt4, power, purge,
};

/// the message when there is no key to decrypt the secrets with.
pub const NO_KEY: &str = "no key yet; run `lanlord keygen` in a terminal first";

/// run `op` on `host` as the account the session signs in as, asking for the sudo password
/// through `ask` when the server needs it.
pub fn execute(host: &str, op: &Op, ask: AskPassword) -> Result<Reply> {
    let caller = Caller::of_session(&Session::new(host)?)?;
    execute_as(host, &caller, op, ask)
}

/// run `op` on `host` for `caller`: refused with [`Denied`] when the caller's roles do not
/// cover it, and the reply holds only what the caller may see. this is the one door every front
/// end uses; the web ui passes the account it signed in.
pub fn execute_as(host: &str, caller: &Caller, op: &Op, ask: AskPassword) -> Result<Reply> {
    execute_in(&Session::new(host)?, host, caller, op, ask)
}

/// as [`execute_as`], through `session`: ssh for a client, [`Session::local`] on the gateway.
/// `host` only names the server in messages and settings.
pub fn execute_in(
    session: &Session,
    host: &str,
    caller: &Caller,
    op: &Op,
    ask: AskPassword,
) -> Result<Reply> {
    guarded(session, caller, op, ask, |grants, ask| match op {
        // these two reach the server their own way
        Op::Bootstrap => {
            if session.is_local() {
                bail!("bootstrap is run from a client, not on the gateway itself");
            }
            let options = Options {
                host: host.to_string(),
                ansible: AnsibleMode::detect(),
                passwordless_sudo: false,
            };
            bootstrap::bootstrap(&options, ask)?;
            Ok(Reply::Done)
        }
        Op::Run { command } => {
            let command = run_as(session, caller, command)?;
            Ok(Reply::ExitCode(session.stream(&command)?))
        }
        _ => {
            let who = Who { caller, grants };
            dispatch(session, host, &who, op, ask, crypto::load_default)
        }
    })
}

/// how long an operation may run: the longest ones (an apply, a purge) take minutes. the
/// operations service stops waiting then, and a command started in a transient unit is stopped.
pub const RUN_LIMIT: std::time::Duration = std::time::Duration::from_secs(16 * 60);

/// how a command is started as another account on the gateway.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Launch {
    /// a transient systemd unit: the command gets a clean environment of its own, and what it
    /// leaves running ends with the unit instead of staying next to the service's own tasks.
    Unit,
    /// `setpriv`, for a machine that is not run by systemd: a clean environment, but nothing
    /// ends the command's children.
    Priv,
}

impl Launch {
    fn here() -> Self {
        let on_path = |name: &str| {
            std::env::var_os("PATH").is_some_and(|path| {
                std::env::split_paths(&path).any(|dir| dir.join(name).is_file())
            })
        };
        if std::path::Path::new("/run/systemd/system").is_dir() && on_path("systemd-run") {
            Self::Unit
        } else {
            Self::Priv
        }
    }

    /// `command` as the account with this uid and gid (and its other groups). a command that
    /// cannot start gives systemd's 203 on the unit path (127 under a shell).
    fn argv(self, uid: u32, gid: u32, command: &[String]) -> Vec<String> {
        let mut argv: Vec<String> = match self {
            Self::Unit => [
                "systemd-run".to_string(),
                format!("--uid={uid}"),
                format!("--gid={gid}"),
                "--pipe".into(),
                "--wait".into(),
                "--collect".into(),
                "--quiet".into(),
                format!("--property=RuntimeMaxSec={}", RUN_LIMIT.as_secs()),
                "--".into(),
            ]
            .into(),
            Self::Priv => [
                "setpriv".to_string(),
                format!("--reuid={uid}"),
                format!("--regid={gid}"),
                "--init-groups".into(),
                "--reset-env".into(),
                "--".into(),
            ]
            .into(),
        };
        argv.extend(command.iter().cloned());
        argv
    }
}

/// the uids of ordinary login accounts here: `UID_MIN` to `UID_MAX` of `login.defs`, or 1000 to
/// 60000. whatever that file says, never root, a system account (below 1000) or `nobody`.
fn login_uids() -> std::ops::RangeInclusive<u32> {
    login_uids_in(&std::fs::read_to_string("/etc/login.defs").unwrap_or_default())
}

fn login_uids_in(defs: &str) -> std::ops::RangeInclusive<u32> {
    let value = |key: &str, default: u32| {
        defs.lines()
            .find_map(|line| {
                let mut words = line.split_whitespace();
                (words.next()? == key).then(|| words.next()?.parse().ok())?
            })
            .unwrap_or(default)
    };
    value("UID_MIN", 1000).max(1000)..=value("UID_MAX", 60000).min(65533)
}

/// the uid and primary gid of the account on the machine `session` runs on.
fn ids_of(session: &Session, account: &str) -> Result<(u32, u32)> {
    let id = |flag: &str| -> Result<u32> {
        let out = session.capture(&["id".into(), flag.into(), "--".into(), account.into()])?;
        out.trim()
            .parse()
            .map_err(|_| anyhow!("could not read the account's ids"))
    };
    Ok((id("-u")?, id("-g")?))
}

/// the command a `Run` executes. over ssh it runs as the account that signed in; on the gateway
/// the service is root, so a local session runs it as the caller's own account, with that
/// account's groups and a clean environment, and never as root or a system account. only the
/// caller's name, checked to be a plain login name, and its numeric ids go into the command.
fn run_as(session: &Session, caller: &Caller, command: &[String]) -> Result<Vec<String>> {
    run_as_by(session, caller, command, Launch::here())
}

fn run_as_by(
    session: &Session,
    caller: &Caller,
    command: &[String],
    launch: Launch,
) -> Result<Vec<String>> {
    if !session.is_local() {
        return Ok(command.to_vec());
    }
    if command.is_empty() {
        bail!("nothing to run");
    }
    if !access::plain_account(&caller.user) {
        bail!("not an account name");
    }
    let (uid, gid) = ids_of(session, &caller.user)?;
    if !login_uids().contains(&uid) {
        return Err(access::Refused(
            "commands run only as an ordinary login account, not root or a system account".into(),
        )
        .into());
    }
    Ok(launch.argv(uid, gid, command))
}

/// who asks and what they may do.
struct Who<'a> {
    caller: &'a Caller,
    grants: &'a Grants,
}

/// check the caller's grants, run, and redact what comes back.
fn guarded(
    session: &Session,
    caller: &Caller,
    op: &Op,
    ask: AskPassword,
    run: impl FnOnce(&Grants, AskPassword) -> Result<Reply>,
) -> Result<Reply> {
    // a person's own account needs no lookup of rights, which needs root on the server
    let grants = match access::grants_of(session, caller, &mut *ask) {
        Ok(grants) => grants,
        Err(_) if op.requires() == Need::SelfOnly => Grants::fallback(caller.admin),
        Err(e) => return Err(e),
    };
    access::authorize(&grants, op)?;
    let reply = run(&grants, ask)?;
    Ok(access::redact(&grants, caller, reply))
}

/// where the key that decrypts the settings comes from; `None` when there is none yet.
type Keys = fn() -> Result<Option<crypto::Identity>>;

/// run `op` through `session` for `caller`. `host` names the server in the settings and
/// messages.
#[cfg(all(test, unix))]
pub(crate) fn execute_with(
    session: &Session,
    host: &str,
    caller: &Caller,
    op: &Op,
    ask: AskPassword,
    keys: Keys,
) -> Result<Reply> {
    guarded(session, caller, op, ask, |grants, ask| {
        dispatch(session, host, &Who { caller, grants }, op, ask, keys)
    })
}

/// the settings to write for a save: only what the caller may change differs from what is stored.
fn plain_for_save(session: &Session, who: &Who, plain: &str, ask: AskPassword) -> Result<String> {
    if who.caller.admin {
        return Ok(plain.to_string());
    }
    let (stored, _) = access::load(session, ask)?;
    save_guard::merge_save(&stored, plain, who.grants, who.caller)
}

fn dispatch(
    session: &Session,
    host: &str,
    who: &Who,
    op: &Op,
    ask: AskPassword,
    keys: Keys,
) -> Result<Reply> {
    let key = || keys()?.ok_or_else(|| anyhow!(NO_KEY));
    Ok(match op {
        Op::Bootstrap | Op::Run { .. } => bail!("this operation is run with `execute`"),
        Op::Apply { modules } => {
            // no key yet just means no settings; a damaged key is an error
            apply::apply_with(session, host, keys()?.as_ref(), modules, ask)?;
            Reply::Done
        }
        Op::LoadDocument => Reply::Document(settings::load_document_with(session, &key()?, ask)?),
        Op::SaveDocument { plain, updates } => {
            let updates: Vec<(String, String)> = updates
                .iter()
                .map(|u| (u.path.clone(), u.value.expose().to_string()))
                .collect();
            let saved = key().and_then(|key| {
                save_guard::check_updates(&updates, who.grants, who.caller)?;
                let plain = plain_for_save(session, who, plain, &mut *ask)?;
                settings::save_document_with(session, host, &key, &plain, &updates, ask)
            });
            // the plain copies of the secrets go before anything else can fail
            drop(zeroize_pairs(updates));
            saved?;
            Reply::Done
        }
        Op::Probe => Reply::Internal(network::probe(session)?),
        Op::ChangeUsers { changes } => {
            let changes: Vec<users::Change> = changes.iter().map(Into::into).collect();
            let changed =
                key().and_then(|key| users::change_with(session, host, &key, &changes, ask));
            // the plain copies of the passwords go before anything else can fail
            zeroize_changes(changes);
            changed?;
            Reply::Done
        }
        Op::ResetMachine { name } => {
            nt4::reset_machine(session, name, ask)?;
            Reply::Done
        }
        Op::CreateAdmin {
            name,
            password,
            ssh_key,
            passwordless_sudo,
        } => {
            if !who.caller.admin {
                return Err(
                    access::Refused("only an administrator creates administrators".into()).into(),
                );
            }
            account::create_admin(
                session,
                host,
                name,
                password.expose(),
                ssh_key.as_deref(),
                *passwordless_sudo,
                ask,
            )?;
            Reply::Done
        }
        Op::Leases => Reply::Leases(leases::read_with(session, ask)?),
        Op::LogonScripts => Reply::Names(logon::list(session, ask)?),
        Op::ReadLogon { name } => Reply::Text(logon::read(session, name, ask)?),
        Op::ReadGeneratedLogon => Reply::Text(logon::read_generated(session, ask)?),
        Op::WriteLogon { name, text } => {
            logon::write(session, name, text, ask)?;
            Reply::Done
        }
        Op::Power { power } => Reply::Outcome(power::run_with(session, *power, ask)?),
        Op::ServerFacts => Reply::Facts(server::read(session)?),
        Op::NetworkStatus => Reply::Interfaces(netstatus::read(session)?),
        Op::PurgePreview { remove_admins } => Reply::Lines(purge::preview(
            session,
            PurgeOptions {
                remove_admins: *remove_admins,
            },
            ask,
        )?),
        Op::Purge { remove_admins } => Reply::Lines(purge::purge(
            session,
            host,
            PurgeOptions {
                remove_admins: *remove_admins,
            },
            ask,
        )?),
        Op::ClockState => Reply::Clock(clock::read(session)?),
        Op::ClockZones => Reply::Names(clock::zones(session)?),
        Op::ClockZoneNow { zone } => Reply::ZoneNow(clock::zone_now(session, zone)?),
        Op::ClockApply { change } => {
            // no key just means the change is not kept in the settings
            let identity = keys()?;
            let (lines, stored) =
                clock::apply_stored(session, host, identity.as_ref(), change, ask)?;
            Reply::ClockApplied { lines, stored }
        }
        Op::ClockUpdateNow => Reply::Lines(clock::update_now(session, ask)?),
        Op::Doctor => Reply::Doctor(doctor::run_with(session, ask)?),
        Op::AccountInfo => self_service::account_info(session, who.caller, ask)?,
        Op::ChangePassword { old, new } => {
            self_service::change_password(session, who.caller, old, new, ask)?
        }
        Op::UpdateProfile { profile } => {
            self_service::update_profile(session, host, who.caller, profile, ask)?
        }
        Op::ResetPassword { user, new } => {
            self_service::reset_password(session, (who.caller, who.grants), user, new, ask)?
        }
        Op::LoadAccess => self_service::load_access(session, ask)?,
        Op::SaveAccess { access } => {
            self_service::save_access(session, host, (who.caller, who.grants), access, ask)?
        }
        Op::Subscribe { .. } => {
            bail!("live streams are served by the web ui, not run as an operation")
        }
    })
}

/// wipe the plain passwords in user changes once they have been handed on.
fn zeroize_changes(mut changes: Vec<users::Change>) {
    use zeroize::Zeroize;
    for change in &mut changes {
        if let users::Change::SetPassword { password, .. } = change {
            password.zeroize();
        }
    }
}

/// wipe the plain copies of secrets once they have been handed on.
fn zeroize_pairs(mut pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    use zeroize::Zeroize;
    for (_, value) in &mut pairs {
        value.zeroize();
    }
    pairs
}

#[cfg(all(test, unix))]
mod tests {
    use crate::access::Level;
    use std::sync::OnceLock;

    use super::*;
    use crate::clock::Change as ClockChange;
    use crate::power::{Outcome, Power};
    use crate::testgw::{identity, no_password, Gateway};

    /// one key for every test, so settings sealed by a test can be read by the executor.
    fn key() -> Result<Option<crypto::Identity>> {
        static KEY: OnceLock<crypto::Identity> = OnceLock::new();
        Ok(Some(KEY.get_or_init(|| identity("ops")).clone()))
    }

    fn no_key() -> Result<Option<crypto::Identity>> {
        Ok(None)
    }

    /// a server that knows who is signed in, lets sudo through and answers everything else with
    /// nothing.
    fn quiet_server() -> Session {
        Session::fake(
            r#"for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "id -nG") echo "tester admins" ;;
  "sudo -n true") exit 0 ;;
  *) cat > /dev/null; exit 0 ;;
esac"#,
        )
    }

    fn admin() -> Caller {
        Caller {
            user: "tester".into(),
            admin: true,
        }
    }

    fn run(session: &Session, op: Op) -> Result<Reply> {
        execute_with(session, "gw", &admin(), &op, &mut no_password, key)
    }

    #[test]
    fn each_op_answers_with_its_own_kind_of_reply() {
        let server = quiet_server();
        let cases = [
            (Op::Apply { modules: vec![] }, Reply::Done),
            (
                Op::SaveDocument {
                    plain: "dns:\n  zone: example\n".into(),
                    updates: vec![],
                },
                Reply::Done,
            ),
            (
                Op::ResetMachine {
                    name: "pc01".into(),
                },
                Reply::Done,
            ),
            (Op::Leases, Reply::Leases(vec![])),
            (Op::LogonScripts, Reply::Names(vec![])),
            (
                Op::ReadLogon {
                    name: "alice.bat".into(),
                },
                Reply::Text(None),
            ),
            (Op::ReadGeneratedLogon, Reply::Text(None)),
            (
                Op::WriteLogon {
                    name: "alice.bat".into(),
                    text: "echo hi\n".into(),
                },
                Reply::Done,
            ),
            (
                Op::Power {
                    power: Power::Reboot,
                },
                Reply::Outcome(Outcome::Done),
            ),
            (Op::ClockUpdateNow, Reply::Lines(vec![])),
        ];
        for (op, want) in cases {
            let got = run(&server, op.clone()).unwrap_or_else(|e| panic!("{op:?}: {e:#}"));
            assert_eq!(got, want, "{op:?}");
        }
        assert!(matches!(
            run(&server, Op::LoadDocument).unwrap(),
            Reply::Document(doc) if doc.plain.is_empty()
        ));
        let change = ClockChange {
            timezone: Some("UTC".into()),
            ..ClockChange::default()
        };
        assert!(matches!(
            run(&server, Op::ClockApply { change }).unwrap(),
            Reply::ClockApplied { .. }
        ));
    }

    #[test]
    fn what_the_server_cannot_answer_comes_back_as_an_error() {
        let server = quiet_server();
        let ops = [
            Op::Probe,
            Op::ServerFacts,
            Op::ClockState,
            Op::ClockZones,
            Op::ClockZoneNow { zone: "UTC".into() },
            Op::PurgePreview {
                remove_admins: false,
            },
            Op::Purge {
                remove_admins: true,
            },
            Op::CreateAdmin {
                name: "admin".into(),
                password: "pw".into(),
                ssh_key: None,
                passwordless_sudo: false,
            },
        ];
        for op in ops {
            assert!(run(&server, op.clone()).is_err(), "{op:?}");
        }
    }

    #[test]
    fn user_changes_reach_the_stored_settings() {
        let gw = Gateway::new("ops-users");
        let key = key().unwrap().unwrap();
        gw.seed_sealed(
            &key,
            "nt4-domain:\n  mode: domain\n  name: EXAMPLE\n  users:\n    - name: dana\n      password: one\n",
        );
        let op = Op::ChangeUsers {
            changes: vec![UserChange::SetPassword {
                name: "olive".into(),
                password: "two".into(),
            }],
        };
        assert_eq!(run(&gw.session, op).unwrap(), Reply::Done);
        assert!(gw.plain().unwrap().contains("olive"));
        assert!(gw.sealed(&key).unwrap().contains("two"));
    }

    #[test]
    fn secrets_are_saved_through_the_settings() {
        let gw = Gateway::new("ops-save");
        let key = key().unwrap().unwrap();
        gw.seed_sealed(&key, "nt4-domain:\n  mode: domain\n  name: EXAMPLE\n");
        let op = Op::SaveDocument {
            plain: "nt4-domain:\n  mode: domain\n  name: EXAMPLE\n".into(),
            updates: vec![SecretUpdate {
                path: "nt4-domain.admin_password".into(),
                value: "s3cret".into(),
            }],
        };
        assert_eq!(run(&gw.session, op).unwrap(), Reply::Done);
        assert!(gw.sealed(&key).unwrap().contains("s3cret"));
        assert!(!gw.plain().unwrap().contains("s3cret"));
    }

    #[test]
    fn settings_ops_need_a_key() {
        let server = quiet_server();
        for op in [
            Op::LoadDocument,
            Op::SaveDocument {
                plain: String::new(),
                updates: vec![],
            },
            Op::ChangeUsers { changes: vec![] },
        ] {
            let err =
                execute_with(&server, "gw", &admin(), &op, &mut no_password, no_key).unwrap_err();
            assert_eq!(err.to_string(), NO_KEY, "{op:?}");
        }
    }

    #[test]
    fn only_an_administrator_creates_administrators_and_applies_every_area() {
        let gw = Gateway::new("ops-create-admin");
        let everything = Grants::all(Level::Manage);
        let olly = Caller {
            user: "olly".into(),
            admin: false,
        };
        let create = Op::CreateAdmin {
            name: "eve".into(),
            password: "pw".into(),
            ssh_key: None,
            passwordless_sudo: false,
        };
        let result = dispatch(
            &gw.session,
            "gw",
            &Who {
                caller: &olly,
                grants: &everything,
            },
            &create,
            &mut no_password,
            key,
        );
        let err = result.unwrap_err();
        assert!(err.downcast_ref::<access::Refused>().is_some(), "{err:#}");
    }

    #[test]
    fn someone_who_is_not_an_administrator_is_a_user() {
        let gw = Gateway::new("ops-user");
        let dana = Caller {
            user: "dana".into(),
            admin: false,
        };
        let go = |op: Op| execute_with(&gw.session, "gw", &dana, &op, &mut no_password, key);
        let denied = go(Op::Leases).unwrap_err();
        assert!(
            denied.downcast_ref::<access::Denied>().is_some(),
            "{denied:#}"
        );
        assert!(go(Op::Power {
            power: Power::Reboot
        })
        .is_err());
        assert!(go(Op::Apply { modules: vec![] }).is_err());
        // status is open to everyone, so this one is not refused
        let facts = go(Op::ServerFacts);
        assert!(
            facts.is_ok()
                || facts
                    .unwrap_err()
                    .downcast_ref::<access::Denied>()
                    .is_none()
        );
    }

    #[test]
    fn a_save_by_someone_without_users_manage_cannot_change_the_assignments() {
        let gw = Gateway::new("ops-save-access");
        let key = key().unwrap().unwrap();
        gw.seed_sealed(&key, "dns: {}\n");
        let plain = "dns: {}\naccess:\n  members:\n    dana: [Administrators]\n";
        let operator_grants = Grants::fallback(false);
        let olly = Caller {
            user: "olly".into(),
            admin: false,
        };
        let who = Who {
            caller: &olly,
            grants: &operator_grants,
        };
        let kept = plain_for_save(&gw.session, &who, plain, &mut no_password);
        // the assignments are an area this caller cannot see, so what is stored stays
        let kept = kept.unwrap();
        assert!(!kept.contains("dana"), "{kept}");
    }

    fn is_denied(result: &Result<Reply>) -> bool {
        result
            .as_ref()
            .err()
            .is_some_and(|e| e.downcast_ref::<access::Denied>().is_some())
    }

    #[test]
    fn each_built_in_role_is_allowed_exactly_what_its_grants_say() {
        let gw = Gateway::new("ops-roles");
        std::fs::write(
            gw.dir.join("yml"),
            "access:\n  members:\n    olly: [Operators]\n    dana: [Users]\n",
        )
        .unwrap();
        let ops = [
            Op::Leases,
            Op::ServerFacts,
            Op::LoadAccess,
            Op::ResetMachine {
                name: "pc01".into(),
            },
            Op::Power {
                power: Power::Reboot,
            },
            Op::LogonScripts,
            Op::AccountInfo,
        ];
        let callers = [
            (admin(), true),
            (
                Caller {
                    user: "olly".into(),
                    admin: false,
                },
                false,
            ),
            (
                Caller {
                    user: "dana".into(),
                    admin: false,
                },
                false,
            ),
        ];
        let stored = access::Access::from_settings(
            &lanlord_model::settings::Settings::parse(
                &std::fs::read_to_string(gw.dir.join("yml")).unwrap(),
            )
            .unwrap(),
        )
        .unwrap()
        .unwrap();
        for (caller, is_admin) in callers {
            let grants = stored.grants(&caller.user, is_admin);
            for op in &ops {
                let got = execute_with(&gw.session, "gw", &caller, op, &mut no_password, key);
                let allowed = grants.allows(op.requires());
                assert_eq!(
                    is_denied(&got),
                    !allowed,
                    "{} on {op:?}: {got:?}",
                    caller.user
                );
            }
        }
    }

    #[test]
    fn a_user_reaches_their_own_account_and_nobody_elses_or_the_permissions() {
        let gw = Gateway::new("ops-self");
        let dana = Caller {
            user: "dana".into(),
            admin: false,
        };
        let go = |caller: &Caller, op: Op| {
            execute_with(&gw.session, "gw", caller, &op, &mut no_password, key)
        };
        let Ok(Reply::Account(info)) = go(&dana, Op::AccountInfo) else {
            panic!("a user may read their own account");
        };
        assert_eq!(info.user, "dana");
        let profile = access::Profile {
            full_name: "Dana Example".into(),
            description: String::new(),
        };
        assert_eq!(
            go(&dana, Op::UpdateProfile { profile }).unwrap(),
            Reply::Done
        );
        let reset = Op::ResetPassword {
            user: "ann".into(),
            new: lanlord_model::secret::Secret::new("pw"),
        };
        assert!(is_denied(&go(&dana, reset.clone())));
        assert!(is_denied(&go(&dana, Op::LoadAccess)));
        assert!(is_denied(&go(
            &dana,
            Op::SaveAccess {
                access: access::Access::default()
            }
        )));
        // an administrator may do all three
        assert!(matches!(go(&admin(), reset), Ok(Reply::Password(_))));
        assert!(matches!(go(&admin(), Op::LoadAccess), Ok(Reply::Access(_))));
        let mut open = access::Access::default();
        open.members
            .insert("dana".into(), vec![access::OPERATORS.into()]);
        assert_eq!(
            go(&admin(), Op::SaveAccess { access: open }).unwrap(),
            Reply::Done
        );
        // and the new role takes effect for dana's next request
        assert!(go(&dana, Op::Leases).is_ok());
    }

    #[test]
    fn bootstrap_and_run_go_through_execute_only() {
        let server = quiet_server();
        assert!(run(&server, Op::Bootstrap).is_err());
        assert!(run(&server, Op::Run { command: vec![] }).is_err());
    }

    #[test]
    fn a_bad_host_is_refused_before_anything_runs() {
        let err = execute("-oProxyCommand=x", &Op::Leases, &mut no_password).unwrap_err();
        assert!(err.to_string().contains("invalid host"), "{err:#}");
    }

    fn commander(user: &str) -> Caller {
        Caller {
            user: user.into(),
            admin: false,
        }
    }

    #[test]
    fn a_local_run_is_the_callers_own_account_never_root() {
        let command = vec!["id".to_string(), "-u".to_string()];
        // the exact command for each way of starting it
        assert_eq!(
            Launch::Unit.argv(1001, 1002, &command),
            [
                "systemd-run",
                "--uid=1001",
                "--gid=1002",
                "--pipe",
                "--wait",
                "--collect",
                "--quiet",
                "--property=RuntimeMaxSec=960",
                "--",
                "id",
                "-u"
            ]
        );
        assert_eq!(
            Launch::Priv.argv(1001, 1002, &command),
            [
                "setpriv",
                "--reuid=1001",
                "--regid=1002",
                "--init-groups",
                "--reset-env",
                "--",
                "id",
                "-u"
            ]
        );
        // a name that is not a plain login name is never looked up, let alone run
        let local = Session::local();
        for bad in ["", "-u", "a b", "x;y", "root\n"] {
            assert!(
                run_as(&local, &commander(bad), &command).is_err(),
                "{bad:?}"
            );
        }
        assert!(run_as(&local, &commander("dana"), &[]).is_err());
        // over ssh the command is the caller's as it was: the login is the account
        let remote = Session::new("gw").unwrap();
        assert_eq!(
            run_as(&remote, &commander("dana"), &command).unwrap(),
            command
        );
    }

    #[test]
    fn login_defs_never_lets_in_root_system_accounts_or_nobody() {
        assert_eq!(login_uids_in(""), 1000..=60000);
        assert_eq!(login_uids_in("UID_MIN 500\nUID_MAX 70000\n"), 1000..=65533);
        let wide = login_uids_in("UID_MIN\t0\nUID_MAX 65534\n");
        assert!(!wide.contains(&0) && !wide.contains(&999) && !wide.contains(&65534));
        assert!(wide.contains(&1000));
        assert!(login_uids_in("UID_MIN 2000").contains(&2000));
        assert!(!login_uids_in("UID_MIN 2000").contains(&1500));
    }

    #[test]
    fn root_and_system_accounts_are_refused_a_local_run() {
        let command = vec!["id".to_string()];
        let local = Session::local();
        for name in ["root", "daemon", "nobody"] {
            // (an account this machine lacks is refused too, for another reason)
            let err = run_as(&local, &commander(name), &command).unwrap_err();
            let known = local
                .capture(&["id".into(), "-u".into(), name.into()])
                .is_ok();
            if known {
                assert!(
                    err.downcast_ref::<access::Refused>().is_some(),
                    "{name}: {err:#}"
                );
            }
        }
    }

    /// run `command` through core's run path as `caller` on this machine, and say what it
    /// printed to the file named by its one argument.
    #[cfg(unix)]
    fn output_of_run(caller: &Caller, script: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let file = std::env::temp_dir().join(format!("lanlord-run-{}", std::process::id()));
        std::fs::write(&file, "").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o666)).unwrap();
        let argv = run_as(
            &Session::local(),
            caller,
            &[
                "sh".into(),
                "-c".into(),
                script.into(),
                "sh".into(),
                file.display().to_string(),
            ],
        )
        .unwrap();
        let code = Session::local().stream(&argv).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        let _ = std::fs::remove_file(&file);
        assert_eq!(code, 0, "{text}");
        text
    }

    /// runs only as root, which is how the service runs (`--ignored root_`; CI does it in the
    /// job "run as the caller (root)"). a throwaway account with an extra group runs a command.
    #[test]
    #[ignore = "needs root"]
    #[cfg(unix)]
    fn root_a_local_run_is_the_callers_uid_and_groups_in_a_clean_environment() {
        let local = Session::local();
        let pid = std::process::id();
        let (user, group) = (format!("lltest{pid}"), format!("llgrp{pid}"));
        let sh = |args: &[&str]| {
            let argv: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            local.capture(&argv).unwrap()
        };
        sh(&["groupadd", &group]);
        sh(&["useradd", "-m", "-s", "/bin/sh", "-G", &group, &user]);
        let uid = sh(&["id", "-u", &user]).trim().to_string();
        let groups = sh(&["id", "-G", &user]).trim().to_string();
        // something of the service's own that must not reach the command
        std::env::set_var("ANSIBLE_HOME", "/root/leak");
        std::env::set_var("LANLORD_LEAK", "1");
        let text = output_of_run(&commander(&user), r#"{ id -u; id -G; env; } > "$1""#);
        std::env::remove_var("ANSIBLE_HOME");
        std::env::remove_var("LANLORD_LEAK");
        sh(&["userdel", "-r", &user]);
        sh(&["groupdel", &group]);

        let mut lines = text.lines();
        assert_eq!(lines.next(), Some(uid.as_str()), "{text}");
        assert_ne!(uid, "0");
        let seen = lines.next().unwrap_or_default();
        assert_eq!(seen, groups, "{text}");
        assert!(!seen.split(' ').any(|g| g == "0"), "root's group: {text}");
        let env: Vec<&str> = lines.collect();
        for leaked in ["ANSIBLE_", "LANLORD_LEAK", "HOME=/root"] {
            assert!(
                !env.iter().any(|l| l.starts_with(leaked)),
                "{leaked}: {text}"
            );
        }
        assert!(env.contains(&format!("USER={user}").as_str()), "{text}");
    }
}
