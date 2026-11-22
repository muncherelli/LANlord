//! runs an [`Operation`] against a server with the same functions the cli and the windows use, so
//! every front end gets the same behaviour from one description of what it wants.

use anyhow::{anyhow, bail, Result};

pub use lanlord_model::operations::*;

use crate::access::{Caller, Grants, Need};
use crate::bootstrap::{self, AnsibleMode, AskPassword, Options};
use crate::purge::PurgeOptions;
use crate::server;
use crate::ssh::Session;
use crate::{
    access, authkeys, internet, lan, operate, save_guard, self_service, settings, stored, users,
};
use crate::{
    account, apply, clock, crypto, doctor, leases, logon, netstatus, network, nt4, nt4_exec, power,
    purge, rootssh, setup, sshauth,
};

/// the message when the gateway has no key to decrypt the secrets with.
pub const NO_KEY: &str = "the gateway has no settings key; operations that read secrets need one";

/// run `operation` on `host` as the account the session signs in as, asking for the sudo password
/// through `ask` when the server needs it. a client holds no key: an operation that touches secrets
/// runs where the key is, through the gateway's local socket when `host` is this machine
/// (the kernel says who is asking) and through `sudo lanlord operate` over ssh otherwise.
pub fn execute(host: &str, operation: &Operation, ask: AskPassword) -> Result<Reply> {
    if operate::is_this_server(host) {
        return operate::through_socket(std::path::Path::new(operate::LOCAL_SOCKET), operation);
    }
    let session = Session::new(host)?;
    if operation.needs_gateway_key() {
        return operate::through_sudo(&session, operation, ask);
    }
    let caller = Caller::of_session(&session)?;
    execute_in(&session, host, &caller, operation, ask)
}

/// run `operation` on `host` for `caller`: refused with [`Denied`] when the caller's roles do not
/// cover it, and the reply holds only what the caller may see. this is the one door every front
/// end uses; the web ui passes the account it signed in.
pub fn execute_as(
    host: &str,
    caller: &Caller,
    operation: &Operation,
    ask: AskPassword,
) -> Result<Reply> {
    execute_in(&Session::new(host)?, host, caller, operation, ask)
}

/// as [`execute_as`], through `session`: ssh for a client, [`Session::local`] on the gateway.
/// `host` only names the server in messages and settings.
pub fn execute_in(
    session: &Session,
    host: &str,
    caller: &Caller,
    operation: &Operation,
    ask: AskPassword,
) -> Result<Reply> {
    guarded(
        session,
        caller,
        operation,
        ask,
        |grants, ask| match operation {
            // these two reach the server their own way
            Operation::Bootstrap => {
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
            Operation::Run { command } => {
                let command = run_as(session, caller, command)?;
                Ok(Reply::ExitCode(session.stream(&command)?))
            }
            _ => {
                let who = Who { caller, grants };
                dispatch(session, host, &who, operation, ask, crypto::load_default)
            }
        },
    )
}

/// run the first-run setup for root's web session, through `session`: the one operation such a
/// session may run, and nothing else. nothing here reads the caller's roles: root has none, and
/// the web ui only forwards this for a session the login helper let in for setup. what keeps it
/// from running twice is the check inside [`setup::first_admin`].
pub fn execute_setup(
    session: &Session,
    host: &str,
    operation: &Operation,
    ask: AskPassword,
) -> Result<Reply> {
    let Operation::SetupFirstAdmin {
        name,
        password,
        root_ssh,
        ssh_key,
    } = operation
    else {
        return Err(access::Refused("only the first-run setup may run now".into()).into());
    };
    operation.validate()?;
    let key = ssh_key.as_deref();
    setup::first_admin(
        session,
        host,
        name,
        password.expose(),
        (*root_ssh, key),
        ask,
    )
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
pub fn login_uids() -> std::ops::RangeInclusive<u32> {
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
    operation: &Operation,
    ask: AskPassword,
    run: impl FnOnce(&Grants, AskPassword) -> Result<Reply>,
) -> Result<Reply> {
    // a person's own account needs no lookup of rights, which needs root on the server
    let grants = match access::grants_of(session, caller, &mut *ask) {
        Ok(grants) => grants,
        Err(_) if operation.requires() == Need::SelfOnly => Grants::fallback(caller.admin),
        Err(e) => return Err(e),
    };
    access::authorize(&grants, operation)?;
    // whatever the client checked, nothing runs on an argument that fails the shared rules
    operation.validate()?;
    if session.is_local() {
        known_zones(operation, ZONEINFO)?;
    }
    let reply = run(&grants, ask)?;
    Ok(access::redact(&grants, caller, reply))
}

/// where this machine keeps its time zone database.
const ZONEINFO: &str = "/usr/share/zoneinfo";

/// the time zones an operation names must be in the zone database of the machine it runs on. when
/// the machine has no database (a minimal container) the name rules alone apply.
fn known_zones(operation: &Operation, zoneinfo: &str) -> Result<()> {
    let root = std::path::Path::new(zoneinfo);
    if !root.is_dir() {
        return Ok(());
    }
    let named = match operation {
        Operation::ClockZoneNow { zone } => Some(("zone", zone.clone())),
        Operation::ClockApply { change } => change.timezone.clone().map(|zone| ("timezone", zone)),
        Operation::SaveDocument { plain, .. } => {
            serde_yaml_ng::from_str::<serde_yaml_ng::Value>(plain)
                .ok()
                .and_then(|settings| settings["gateway"]["timezone"].as_str().map(str::to_string))
                .map(|zone| ("gateway.timezone", zone))
        }
        _ => None,
    };
    match named {
        Some((field, zone)) if !root.join(&zone).is_file() => {
            bail!("{field}: {zone} is not a time zone this machine knows")
        }
        _ => Ok(()),
    }
}

/// where the key that decrypts the settings comes from; `None` when there is none yet.
type Keys = fn() -> Result<Option<crypto::Identity>>;

/// run `operation` through `session` for `caller`. `host` names the server in the settings and
/// messages.
#[cfg(all(test, unix))]
pub(crate) fn execute_with(
    session: &Session,
    host: &str,
    caller: &Caller,
    operation: &Operation,
    ask: AskPassword,
    keys: Keys,
) -> Result<Reply> {
    guarded(session, caller, operation, ask, |grants, ask| {
        dispatch(session, host, &Who { caller, grants }, operation, ask, keys)
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
    operation: &Operation,
    ask: AskPassword,
    keys: Keys,
) -> Result<Reply> {
    let key = || keys()?.ok_or_else(|| anyhow!(NO_KEY));
    Ok(match operation {
        Operation::Bootstrap | Operation::Run { .. } => {
            bail!("this operation is run with `execute`")
        }
        Operation::Apply { modules } => {
            // no key yet just means no settings; a damaged key is an error
            apply::apply_with(session, host, keys()?.as_ref(), modules, ask)?;
            Reply::Done
        }
        Operation::PlanApply { modules } => {
            Reply::Plan(apply::plan(session, keys()?.as_ref(), modules, ask)?)
        }
        Operation::LoadDocument => {
            Reply::Document(settings::load_document_with(session, &key()?, ask)?)
        }
        Operation::MoveSecrets { key: old_key } => {
            let moved = crypto::parse(old_key.expose())
                .and_then(|old| settings::move_secrets_with(session, host, &key()?, &old, ask));
            moved?;
            Reply::Done
        }
        Operation::RotateExecPassword => {
            nt4_exec::rotate(session, host, &key()?, ask)?;
            Reply::Done
        }
        Operation::MigrateStored { stamp } => {
            Reply::Lines(stored::migrate_with(session, host, &key()?, stamp, ask)?)
        }
        Operation::SaveDocument { plain, updates } => {
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
        Operation::Probe => Reply::Internal(network::probe(session)?),
        Operation::ChangeInternet { change } => {
            internet::change(session, host, &key()?, change, ask)?;
            Reply::Done
        }
        Operation::ChangeSshAccess { change } => {
            internet::change_ssh(session, host, &key()?, change, ask)?;
            Reply::Done
        }
        Operation::ChangeNetworks { edit } => {
            lan::change(session, host, &key()?, edit, ask)?;
            Reply::Done
        }
        Operation::ChangeUsers { changes } => {
            let changes: Vec<users::Change> = changes.iter().map(Into::into).collect();
            let changed =
                key().and_then(|key| users::change_with(session, host, &key, &changes, ask));
            // the plain copies of the passwords go before anything else can fail
            zeroize_changes(changes);
            changed?;
            Reply::Done
        }
        Operation::ResetMachine { name } => {
            nt4::reset_machine(session, name, ask)?;
            Reply::Done
        }
        Operation::CreateAdmin {
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
        Operation::SetupFirstAdmin {
            name,
            password,
            root_ssh,
            ssh_key,
        } => {
            // only root is ever in setup: an administrator has no use for it, and the check
            // that no other administrator exists is made inside
            if who.caller.user != "root" {
                return Err(access::Refused("first-run setup is for root only".into()).into());
            }
            let key = ssh_key.as_deref();
            setup::first_admin(
                session,
                host,
                name,
                password.expose(),
                (*root_ssh, key),
                ask,
            )?
        }
        Operation::RootSshState => Reply::RootSsh(rootssh::state(session)?),
        Operation::SetRootSsh { access } => {
            rootssh::apply(session, *access, ask)?;
            Reply::Done
        }
        Operation::SshAuthState => Reply::SshAuth(sshauth::state(session, ask)?),
        Operation::SetPasswordLogin {
            passwords,
            confirmed,
        } => {
            sshauth::set_passwords(session, *passwords, *confirmed, ask)?;
            Reply::Done
        }
        Operation::AddAdminKey { user, key } => {
            sshauth::add_key(session, user, key, ask)?;
            Reply::Done
        }
        Operation::RemoveAdminKey {
            user,
            key,
            confirmed,
        } => {
            sshauth::remove_key(session, user, key, *confirmed, ask)?;
            Reply::Done
        }
        Operation::Leases => Reply::Leases(leases::read_with(session, ask)?),
        Operation::LogonScripts => Reply::Names(logon::list(session, ask)?),
        Operation::ReadLogon { name } => Reply::Text(logon::read(session, name, ask)?),
        Operation::ReadGeneratedLogon => Reply::Text(logon::read_generated(session, ask)?),
        Operation::WriteLogon { name, text } => {
            logon::write(session, name, text, ask)?;
            Reply::Done
        }
        Operation::Power { power } => Reply::Outcome(power::run_with(session, *power, ask)?),
        Operation::ServerFacts => {
            // the session may be root's (the operations service runs as root, and sudo is
            // elevation, not a login), so the account shown is the caller's, never `id -un`
            let mut facts = server::read(session)?;
            facts.user = who.caller.user.clone();
            facts.is_root = who.caller.user == "root";
            Reply::Facts(Box::new(facts))
        }
        Operation::NetworkStatus => Reply::Interfaces(netstatus::read(session)?),
        Operation::PurgePreview { remove_admins } => Reply::Lines(purge::preview(
            session,
            &who.caller.user,
            PurgeOptions {
                remove_admins: *remove_admins,
            },
            ask,
        )?),
        Operation::Purge { remove_admins } => Reply::Lines(purge::purge(
            session,
            &who.caller.user,
            host,
            PurgeOptions {
                remove_admins: *remove_admins,
            },
            ask,
        )?),
        Operation::ClockState => Reply::Clock(clock::read(session)?),
        Operation::ClockZones => Reply::Names(clock::zones(session)?),
        Operation::ClockZoneNow { zone } => Reply::ZoneNow(clock::zone_now(session, zone)?),
        Operation::ClockApply { change } => {
            // no key just means the change is not kept in the settings
            let identity = keys()?;
            let (lines, stored) =
                clock::apply_stored(session, host, identity.as_ref(), change, ask)?;
            Reply::ClockApplied { lines, stored }
        }
        Operation::ClockUpdateNow => Reply::Lines(clock::update_now(session, ask)?),
        Operation::Doctor => Reply::Doctor(doctor::run_with(session, ask)?),
        Operation::AccountInfo => self_service::account_info(session, who.caller, ask)?,
        Operation::MyKeys => Reply::Lines(authkeys::list(session, &who.caller.user, ask)?),
        Operation::AddMyKey { key } => {
            authkeys::add(session, &who.caller.user, key, ask)?;
            Reply::Done
        }
        Operation::RemoveMyKey { key } => {
            authkeys::remove(session, &who.caller.user, key, ask)?;
            Reply::Done
        }
        Operation::ChangePassword { old, new } => {
            self_service::change_password(session, who.caller, old, new, ask)?
        }
        Operation::UpdateProfile { profile } => {
            self_service::update_profile(session, host, who.caller, profile, ask)?
        }
        Operation::ResetPassword { user, new } => {
            self_service::reset_password(session, (who.caller, who.grants), user, new, ask)?
        }
        Operation::LoadAccess => self_service::load_access(session, ask)?,
        Operation::SaveAccess { access } => {
            self_service::save_access(session, host, (who.caller, who.grants), access, ask)?
        }
        Operation::Subscribe { .. } => {
            bail!("live streams are served by the web ui, not run as an operation")
        }
        Operation::Machines => Reply::Machines(nt4_exec::machines(session, ask)?),
        Operation::MachineRun {
            machine,
            command,
            timeout_secs,
        } => machine_run(session, &key()?, ask, (machine, command, *timeout_secs))?,
        Operation::MachinePreset {
            machine,
            preset,
            args,
        } => {
            let command = nt4_exec::render(preset, args).map_err(|reason| anyhow!("{reason}"))?;
            let timeout = DEFAULT_TIMEOUT_SECS;
            machine_run(session, &key()?, ask, (machine, &command, timeout))?
        }
    })
}

/// run a command on a machine and answer with what it did.
fn machine_run(
    session: &Session,
    identity: &age::x25519::Identity,
    ask: AskPassword,
    job: nt4_exec::Job,
) -> Result<Reply> {
    let outcome = nt4_exec::run(session, identity, ask, job, &mut |_, _| {})?;
    Ok(Reply::MachineRun {
        stdout: outcome.stdout_text(),
        stderr: outcome.stderr_text(),
        exit_code: outcome.exit_code,
        timed_out: outcome.timed_out,
        truncated: outcome.truncated,
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

    #[test]
    fn a_time_zone_must_be_in_the_zone_database() {
        let root = std::env::temp_dir().join(format!("lanlord-zoneinfo-{}", std::process::id()));
        std::fs::create_dir_all(root.join("Europe")).unwrap();
        std::fs::write(root.join("Europe/Berlin"), b"").unwrap();
        let dir = root.to_str().unwrap();
        let zone = |zone: &str| Operation::ClockZoneNow { zone: zone.into() };
        assert!(known_zones(&zone("Europe/Berlin"), dir).is_ok());
        let error = known_zones(&zone("Europe/Nowhere"), dir).unwrap_err();
        assert!(error.to_string().starts_with("zone: "), "{error}");
        let save = |text: &str| Operation::SaveDocument {
            plain: text.into(),
            updates: vec![],
        };
        assert!(known_zones(&save("gateway:\n  timezone: Europe/Berlin\n"), dir).is_ok());
        assert!(known_zones(&save("gateway:\n  timezone: Mars/Base\n"), dir).is_err());
        assert!(known_zones(&save("dns: {}\n"), dir).is_ok());
        assert!(known_zones(&Operation::LoadDocument, dir).is_ok());
        let change = ClockChange {
            timezone: Some("Mars/Base".into()),
            ..ClockChange::default()
        };
        assert!(known_zones(&Operation::ClockApply { change }, dir).is_err());
        // no database on the machine: the name rules alone apply
        assert!(known_zones(&zone("Mars/Base"), "/nonexistent-zoneinfo").is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn guarded_refuses_an_invalid_operation_before_anything_runs() {
        let server = quiet_server();
        let grants_ran = std::cell::Cell::new(false);
        let go = |operation: Operation| {
            grants_ran.set(false);
            let result = guarded(&server, &admin(), &operation, &mut no_password, |_, _| {
                grants_ran.set(true);
                Ok(Reply::Done)
            });
            (result, grants_ran.get())
        };
        let save = |plain: &str| Operation::SaveDocument {
            plain: plain.into(),
            updates: vec![],
        };
        let bomb = format!("a: &a [x]\nb: *a\n{}", "c: *a\n".repeat(1000));
        let hostile = [
            (
                save("nt4-domain:\n  log_level: \"1\\n root preexec = id\"\n"),
                "log_level",
            ),
            (
                save("dns:\n  zone: \"{{ lookup('pipe', 'id') }}\"\n"),
                "dns.zone",
            ),
            (save(&bomb), "anchors"),
            (save("image: evil\n"), "image"),
        ];
        for (operation, word) in hostile {
            let (result, ran) = go(operation);
            let message = format!("{:#}", result.unwrap_err());
            assert!(message.contains(word), "{message}");
            assert!(!message.contains('\n'), "one line: {message:?}");
            assert!(!ran, "{word}: the operation must not run");
        }
        let (result, ran) = go(save("gateway:\n  timezone: UTC\n"));
        assert!(result.is_ok() && ran);
    }

    /// one key for every test, so settings sealed by a test can be read by the executor.
    fn key() -> Result<Option<crypto::Identity>> {
        static KEY: OnceLock<crypto::Identity> = OnceLock::new();
        Ok(Some(KEY.get_or_init(|| identity("operations")).clone()))
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

    fn run(session: &Session, operation: Operation) -> Result<Reply> {
        execute_with(session, "gw", &admin(), &operation, &mut no_password, key)
    }

    #[test]
    fn each_operation_answers_with_its_own_kind_of_reply() {
        let server = quiet_server();
        let cases = [
            (Operation::Apply { modules: vec![] }, Reply::Done),
            (
                Operation::SaveDocument {
                    plain: "dns:\n  zone: example\n".into(),
                    updates: vec![],
                },
                Reply::Done,
            ),
            (
                Operation::ResetMachine {
                    name: "pc01".into(),
                },
                Reply::Done,
            ),
            (Operation::Leases, Reply::Leases(vec![])),
            (Operation::LogonScripts, Reply::Names(vec![])),
            (
                Operation::ReadLogon {
                    name: "alice.bat".into(),
                },
                Reply::Text(None),
            ),
            (Operation::ReadGeneratedLogon, Reply::Text(None)),
            (
                Operation::WriteLogon {
                    name: "alice.bat".into(),
                    text: "echo hi\n".into(),
                },
                Reply::Done,
            ),
            (
                Operation::Power {
                    power: Power::Reboot,
                },
                Reply::Outcome(Outcome::Done),
            ),
            (Operation::ClockUpdateNow, Reply::Lines(vec![])),
        ];
        for (operation, want) in cases {
            let got =
                run(&server, operation.clone()).unwrap_or_else(|e| panic!("{operation:?}: {e:#}"));
            assert_eq!(got, want, "{operation:?}");
        }
        assert!(matches!(
            run(&server, Operation::LoadDocument).unwrap(),
            Reply::Document(doc) if doc.plain.is_empty()
        ));
        let change = ClockChange {
            timezone: Some("UTC".into()),
            ..ClockChange::default()
        };
        assert!(matches!(
            run(&server, Operation::ClockApply { change }).unwrap(),
            Reply::ClockApplied { .. }
        ));
    }

    #[test]
    fn what_the_server_cannot_answer_comes_back_as_an_error() {
        let server = quiet_server();
        let operations = [
            Operation::Probe,
            Operation::ServerFacts,
            Operation::ClockState,
            Operation::ClockZones,
            Operation::ClockZoneNow { zone: "UTC".into() },
            Operation::PurgePreview {
                remove_admins: false,
            },
            Operation::Purge {
                remove_admins: true,
            },
            Operation::CreateAdmin {
                name: "admin".into(),
                password: "pw".into(),
                ssh_key: None,
                passwordless_sudo: false,
            },
        ];
        for operation in operations {
            assert!(run(&server, operation.clone()).is_err(), "{operation:?}");
        }
    }

    #[test]
    fn user_changes_reach_the_stored_settings() {
        let gw = Gateway::new("operations-users");
        let key = key().unwrap().unwrap();
        gw.seed_sealed(
            &key,
            "nt4-domain:\n  mode: domain\n  name: EXAMPLE\n  users:\n    - name: dana\n      password: one\n",
        );
        let operation = Operation::ChangeUsers {
            changes: vec![UserChange::SetPassword {
                name: "olive".into(),
                password: "two".into(),
            }],
        };
        assert_eq!(run(&gw.session, operation).unwrap(), Reply::Done);
        assert!(gw.plain().unwrap().contains("olive"));
        assert!(gw.sealed(&key).unwrap().contains("two"));
    }

    #[test]
    fn an_internet_change_reaches_the_stored_settings() {
        let gw = Gateway::new("operations-internet");
        let key = key().unwrap().unwrap();
        gw.seed_sealed(
            &key,
            "docker: {}\nnetworks:\n  - name: lab\n    interface: eth2\n    address: 192.0.2.1/24\n",
        );
        let operation = Operation::ChangeInternet {
            change: lanlord_model::internet::Change::Network {
                network: "lab".into(),
                on: true,
            },
        };
        assert_eq!(run(&gw.session, operation).unwrap(), Reply::Done);
        assert!(gw.plain().unwrap().contains("internet: true"));
    }

    #[test]
    fn secrets_are_saved_through_the_settings() {
        let gw = Gateway::new("operations-save");
        let key = key().unwrap().unwrap();
        gw.seed_sealed(&key, "nt4-domain:\n  mode: domain\n  name: EXAMPLE\n");
        let operation = Operation::SaveDocument {
            plain: "nt4-domain:\n  mode: domain\n  name: EXAMPLE\n".into(),
            updates: vec![SecretUpdate {
                path: "nt4-domain.admin_password".into(),
                value: "s3cret".into(),
            }],
        };
        assert_eq!(run(&gw.session, operation).unwrap(), Reply::Done);
        assert!(gw.sealed(&key).unwrap().contains("s3cret"));
        assert!(!gw.plain().unwrap().contains("s3cret"));
    }

    const MARKER: &str = "zq-marker-7731";

    /// the text of a key file, as a client would send it.
    fn key_text(identity: &crypto::Identity) -> lanlord_model::secret::Secret {
        use age::secrecy::ExposeSecret;
        lanlord_model::secret::Secret::new(identity.to_string().expose_secret().to_string())
    }

    fn save_marked(gw: &Gateway) {
        let operation = Operation::SaveDocument {
            plain: "nt4-domain:\n  mode: domain\n  name: EXAMPLE\n".into(),
            updates: vec![SecretUpdate {
                path: "nt4-domain.admin_password".into(),
                value: MARKER.into(),
            }],
        };
        assert_eq!(run(&gw.session, operation).unwrap(), Reply::Done);
    }

    #[test]
    fn a_secret_is_set_write_only_and_every_read_path_redacts_it() {
        let gw = Gateway::new("operations-redact");
        let sealed_with = key().unwrap().unwrap();
        gw.seed_sealed(
            &sealed_with,
            "nt4-domain:\n  mode: domain\n  name: EXAMPLE\n",
        );
        save_marked(&gw);
        // on the gateway it is sealed, and nowhere in the clear
        assert!(gw.sealed(&sealed_with).unwrap().contains(MARKER));
        assert!(!gw.plain().unwrap().contains(MARKER));
        let ciphertext = std::fs::read(gw.dir.join("age")).unwrap();
        assert!(!String::from_utf8_lossy(&ciphertext).contains(MARKER));
        // what a client reads back says "set", never the value, as an administrator or not
        for caller in [
            admin(),
            Caller {
                user: "dana".into(),
                admin: false,
            },
        ] {
            let reads = [
                Operation::LoadDocument,
                Operation::LoadAccess,
                Operation::AccountInfo,
            ];
            for read in reads {
                let reply = execute_with(&gw.session, "gw", &caller, &read, &mut no_password, key);
                if let Ok(reply) = reply {
                    let wire = serde_json::to_string(&reply).unwrap();
                    assert!(!wire.contains(MARKER), "{read:?}: {wire}");
                    assert!(!format!("{reply:?}").contains(MARKER), "{read:?}");
                }
            }
        }
        let Reply::Document(document) = run(&gw.session, Operation::LoadDocument).unwrap() else {
            panic!("not a document")
        };
        assert!(document
            .secrets_set
            .contains(&"nt4-domain.admin_password".to_string()));
        assert!(!document.needs_move);
    }

    #[test]
    fn a_client_machine_holds_no_key_and_never_decrypts() {
        // nothing in this process was told where a gateway key is
        let Err(err) = crypto::load_default() else {
            panic!("a client found a key")
        };
        assert_eq!(err.to_string(), crypto::CLIENT_HAS_NO_KEY);
    }

    #[test]
    fn secrets_sealed_to_an_old_client_key_move_to_the_gateway_once() {
        let gw = Gateway::new("operations-move");
        let gateway_key = key().unwrap().unwrap();
        let old = identity("operations-move-old");
        gw.seed_sealed(&old, "nt4-domain:\n  admin_password: oldsecret\n");
        // the gateway cannot read them yet, and says so
        let Reply::Document(before) = run(&gw.session, Operation::LoadDocument).unwrap() else {
            panic!("not a document")
        };
        assert!(before.needs_move);
        // a save does not quietly overwrite what it cannot read
        assert!(run(
            &gw.session,
            Operation::SaveDocument {
                plain: String::new(),
                updates: vec![]
            }
        )
        .is_err());
        // a wrong key moves nothing
        let wrong = Operation::MoveSecrets {
            key: key_text(&identity("operations-move-wrong")),
        };
        assert!(run(&gw.session, wrong).is_err());
        assert!(gw.sealed(&old).is_some());
        // the right one seals them to the gateway's key
        let mv = Operation::MoveSecrets {
            key: key_text(&old),
        };
        assert_eq!(run(&gw.session, mv.clone()).unwrap(), Reply::Done);
        assert!(gw.sealed(&gateway_key).unwrap().contains("oldsecret"));
        let Reply::Document(after) = run(&gw.session, Operation::LoadDocument).unwrap() else {
            panic!("not a document")
        };
        assert!(!after.needs_move);
        // moving again changes nothing
        assert_eq!(run(&gw.session, mv).unwrap(), Reply::Done);
        assert!(gw.sealed(&gateway_key).unwrap().contains("oldsecret"));
    }

    #[test]
    fn settings_operations_need_a_key() {
        let server = quiet_server();
        for operation in [
            Operation::LoadDocument,
            Operation::SaveDocument {
                plain: String::new(),
                updates: vec![],
            },
            Operation::ChangeUsers { changes: vec![] },
        ] {
            let err = execute_with(
                &server,
                "gw",
                &admin(),
                &operation,
                &mut no_password,
                no_key,
            )
            .unwrap_err();
            assert_eq!(err.to_string(), NO_KEY, "{operation:?}");
        }
    }

    /// a gateway with the exec account's secret sealed to the test key, whose machine `ws1`
    /// answers `events` to any run.
    fn machine_gateway(record: &std::path::Path, events: &str) -> Session {
        use crate::nt4_exec::tests as fixtures;
        let identity = key().unwrap().unwrap();
        let sealed = fixtures::sealed(&identity, fixtures::PASSWORD);
        fixtures::gateway(
            record,
            &fixtures::Fake {
                sealed: &sealed,
                lease: "",
                first: events,
                second: "",
            },
        )
    }

    fn on_machines(session: &Session, operation: &Operation) -> Result<Reply> {
        let everything = Grants::all(Level::Manage);
        let admin = Caller {
            user: "admin".into(),
            admin: true,
        };
        let who = Who {
            caller: &admin,
            grants: &everything,
        };
        dispatch(session, "gw", &who, operation, &mut no_password, key)
    }

    #[test]
    fn the_machines_of_the_domain_are_listed() {
        let record = tempfile::NamedTempFile::new().unwrap();
        let session = machine_gateway(record.path(), "");
        assert_eq!(
            on_machines(&session, &Operation::Machines).unwrap(),
            Reply::Machines(vec!["ws1".into()])
        );
    }

    #[test]
    fn a_machine_run_and_a_preset_answer_with_decoded_output() {
        use crate::nt4_exec::tests::event;
        let events = format!(
            "{}\n{}\n{}",
            event(r#"{"stream":"out","data":"gQ=="}"#),
            event(r#"{"stream":"err","data":"ZQ=="}"#),
            event(r#"{"exit":3}"#)
        );
        let record = tempfile::NamedTempFile::new().unwrap();
        let session = machine_gateway(record.path(), &events);
        let expected = Reply::MachineRun {
            exit_code: Some(3),
            stdout: "\u{fc}".into(),
            stderr: "e".into(),
            timed_out: false,
            truncated: false,
        };
        let run = Operation::MachineRun {
            machine: "WS1".into(),
            command: "ver".into(),
            timeout_secs: 5,
        };
        assert_eq!(on_machines(&session, &run).unwrap(), expected);
        let preset = Operation::MachinePreset {
            machine: "ws1".into(),
            preset: "net-start".into(),
            args: vec![],
        };
        assert_eq!(on_machines(&session, &preset).unwrap(), expected);
        let sent = std::fs::read_to_string(record.path()).unwrap();
        assert!(sent.contains(r#""command":"net start""#), "{sent}");
        let unknown = Operation::MachinePreset {
            machine: "ws1".into(),
            preset: "nope".into(),
            args: vec![],
        };
        assert!(on_machines(&session, &unknown).is_err());
    }

    #[test]
    fn a_timed_out_machine_run_has_no_exit_code() {
        let events = crate::nt4_exec::tests::event(r#"{"timed_out":true}"#);
        let record = tempfile::NamedTempFile::new().unwrap();
        let session = machine_gateway(record.path(), &events);
        let run = Operation::MachineRun {
            machine: "ws1".into(),
            command: "ping -t x".into(),
            timeout_secs: 1,
        };
        let Reply::MachineRun {
            exit_code,
            timed_out,
            ..
        } = on_machines(&session, &run).unwrap()
        else {
            panic!("not a run");
        };
        assert!(timed_out && exit_code.is_none());
    }

    #[test]
    fn only_an_administrator_creates_administrators_and_applies_every_area() {
        let gw = Gateway::new("operations-create-admin");
        let everything = Grants::all(Level::Manage);
        let olly = Caller {
            user: "olly".into(),
            admin: false,
        };
        let create = Operation::CreateAdmin {
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
        let gw = Gateway::new("operations-user");
        let dana = Caller {
            user: "dana".into(),
            admin: false,
        };
        let go = |operation: Operation| {
            execute_with(&gw.session, "gw", &dana, &operation, &mut no_password, key)
        };
        let denied = go(Operation::Leases).unwrap_err();
        assert!(
            denied.downcast_ref::<access::Denied>().is_some(),
            "{denied:#}"
        );
        assert!(go(Operation::Power {
            power: Power::Reboot
        })
        .is_err());
        assert!(go(Operation::Apply { modules: vec![] }).is_err());
        // status is open to everyone, so this one is not refused
        let facts = go(Operation::ServerFacts);
        assert!(
            facts.is_ok()
                || facts
                    .unwrap_err()
                    .downcast_ref::<access::Denied>()
                    .is_none()
        );
    }

    #[test]
    fn server_facts_name_the_caller_not_the_account_the_session_runs_as() {
        // the gateway answers as root, like the operations service's own session
        let session = Session::fake(
            r#"cat > /dev/null; printf 'hostname=gw\nuser=root\nuid=0\ninstalled=1\n---link\n'"#,
        );
        let alice = Caller {
            user: "alice".into(),
            admin: true,
        };
        let Ok(Reply::Facts(facts)) = execute_with(
            &session,
            "gw",
            &alice,
            &Operation::ServerFacts,
            &mut no_password,
            key,
        ) else {
            panic!("no facts");
        };
        assert_eq!(facts.user, "alice");
        assert!(!facts.is_root);
        let root = Caller {
            user: "root".into(),
            admin: true,
        };
        let Ok(Reply::Facts(facts)) = execute_with(
            &session,
            "gw",
            &root,
            &Operation::ServerFacts,
            &mut no_password,
            key,
        ) else {
            panic!("no facts");
        };
        assert_eq!((facts.user.as_str(), facts.is_root), ("root", true));
    }

    #[test]
    fn a_save_by_someone_without_users_manage_cannot_change_the_assignments() {
        let gw = Gateway::new("operations-save-access");
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
        let gw = Gateway::new("operations-roles");
        std::fs::write(
            gw.dir.join("yml"),
            "access:\n  members:\n    olly: [Operators]\n    dana: [Users]\n",
        )
        .unwrap();
        let operations = [
            Operation::Leases,
            Operation::ServerFacts,
            Operation::LoadAccess,
            Operation::ResetMachine {
                name: "pc01".into(),
            },
            Operation::Power {
                power: Power::Reboot,
            },
            Operation::LogonScripts,
            Operation::AccountInfo,
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
            for operation in &operations {
                let got =
                    execute_with(&gw.session, "gw", &caller, operation, &mut no_password, key);
                let allowed = grants.allows(operation.requires());
                assert_eq!(
                    is_denied(&got),
                    !allowed,
                    "{} on {operation:?}: {got:?}",
                    caller.user
                );
            }
        }
    }

    #[test]
    fn a_user_reaches_their_own_account_and_nobody_elses_or_the_permissions() {
        let gw = Gateway::new("operations-self");
        let dana = Caller {
            user: "dana".into(),
            admin: false,
        };
        let go = |caller: &Caller, operation: Operation| {
            execute_with(&gw.session, "gw", caller, &operation, &mut no_password, key)
        };
        let Ok(Reply::Account(info)) = go(&dana, Operation::AccountInfo) else {
            panic!("a user may read their own account");
        };
        assert_eq!(info.user, "dana");
        let profile = access::Profile {
            full_name: "Dana Example".into(),
            description: String::new(),
        };
        assert_eq!(
            go(&dana, Operation::UpdateProfile { profile }).unwrap(),
            Reply::Done
        );
        let reset = Operation::ResetPassword {
            user: "ann".into(),
            new: lanlord_model::secret::Secret::new("pw"),
        };
        assert!(is_denied(&go(&dana, reset.clone())));
        assert!(is_denied(&go(&dana, Operation::LoadAccess)));
        let internet = Operation::ChangeInternet {
            change: lanlord_model::internet::Change::Network {
                network: "lab".into(),
                on: true,
            },
        };
        assert!(is_denied(&go(&dana, internet)));
        let ssh = Operation::ChangeSshAccess {
            change: lanlord_model::ssh_access::Change::Network {
                network: "lab".into(),
                on: true,
            },
        };
        assert!(is_denied(&go(&dana, ssh)));
        let networks = Operation::ChangeNetworks {
            edit: Box::new(lanlord_model::lan_edit::Edit::Remove("lab".into())),
        };
        assert!(is_denied(&go(&dana, networks)));
        assert!(is_denied(&go(
            &dana,
            Operation::SaveAccess {
                access: access::Access::default()
            }
        )));
        // an administrator may do all three
        assert!(matches!(go(&admin(), reset), Ok(Reply::Password(_))));
        assert!(matches!(
            go(&admin(), Operation::LoadAccess),
            Ok(Reply::Access(_))
        ));
        let mut open = access::Access::default();
        open.members
            .insert("dana".into(), vec![access::OPERATORS.into()]);
        assert_eq!(
            go(&admin(), Operation::SaveAccess { access: open }).unwrap(),
            Reply::Done
        );
        // and the new role takes effect for dana's next request
        assert!(go(&dana, Operation::Leases).is_ok());
    }

    #[test]
    fn bootstrap_and_run_go_through_execute_only() {
        let server = quiet_server();
        assert!(run(&server, Operation::Bootstrap).is_err());
        assert!(run(&server, Operation::Run { command: vec![] }).is_err());
    }

    #[test]
    fn a_bad_host_is_refused_before_anything_runs() {
        let err = execute("-oProxyCommand=x", &Operation::Leases, &mut no_password).unwrap_err();
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
