//! runs an [`Op`] against a server with the same functions the cli and the windows use, so
//! every front end gets the same behaviour from one description of what it wants.

use anyhow::{anyhow, bail, Result};

pub use lanlord_model::ops::*;

use crate::bootstrap::{self, AnsibleMode, AskPassword, Options};
use crate::exec::{self, Target};
use crate::purge::PurgeOptions;
use crate::server;
use crate::ssh::Session;
use crate::{
    account, apply, clock, crypto, doctor, leases, logon, netstatus, network, nt4, power, purge,
};
use crate::{settings, users};

/// the message when there is no key to decrypt the secrets with.
pub const NO_KEY: &str = "no key yet; run `lanlord keygen` in a terminal first";

/// run `op` on `host`, asking for the sudo password through `ask` when the server needs it.
pub fn execute(host: &str, op: &Op, ask: AskPassword) -> Result<Reply> {
    match op {
        // these two reach the server their own way
        Op::Bootstrap => {
            let options = Options {
                host: host.to_string(),
                ansible: AnsibleMode::detect(),
                passwordless_sudo: false,
            };
            bootstrap::bootstrap(&options, ask)?;
            Ok(Reply::Done)
        }
        Op::Run { command } => {
            let target = Target::Gateway { host: host.into() };
            Ok(Reply::ExitCode(exec::exec_detached(&target, command)?))
        }
        _ => execute_with(&Session::new(host)?, host, op, ask, crypto::load_default),
    }
}

/// where the key that decrypts the settings comes from; `None` when there is none yet.
type Keys = fn() -> Result<Option<crypto::Identity>>;

/// run `op` through `session`. `host` names the server in the settings and messages.
pub(crate) fn execute_with(
    session: &Session,
    host: &str,
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
                settings::save_document_with(session, host, &key, plain, &updates, ask)
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
        Op::Subscribe { .. } => bail!("live streams are not available yet"),
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
  "sudo -n true") exit 0 ;;
  *) cat > /dev/null; exit 0 ;;
esac"#,
        )
    }

    fn run(session: &Session, op: Op) -> Result<Reply> {
        execute_with(session, "gw", &op, &mut no_password, key)
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
            let err = execute_with(&server, "gw", &op, &mut no_password, no_key).unwrap_err();
            assert_eq!(err.to_string(), NO_KEY, "{op:?}");
        }
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
}
