//! runs an [`Op`] against a server with the same functions the cli and the windows use, so
//! every front end gets the same behaviour from one description of what it wants.

use anyhow::{anyhow, Result};

pub use lanlord_model::ops::*;

use crate::bootstrap::{self, AnsibleMode, AskPassword, Options};
use crate::exec::{self, Target};
use crate::purge::PurgeOptions;
use crate::server;
use crate::ssh::Session;
use crate::{account, apply, clock, crypto, leases, logon, netstatus, network, nt4, power, purge};
use crate::{settings, users};

/// the message when there is no key to decrypt the secrets with.
pub const NO_KEY: &str = "no key yet; run `lanlord keygen` in a terminal first";

fn key() -> Result<crypto::Identity> {
    crypto::load_default()?.ok_or_else(|| anyhow!(NO_KEY))
}

/// run `op` on `host`, asking for the sudo password through `ask` when the server needs it.
pub fn execute(host: &str, op: &Op, ask: AskPassword) -> Result<Reply> {
    let session = || Session::new(host);
    Ok(match op {
        Op::Bootstrap => {
            let options = Options {
                host: host.to_string(),
                ansible: AnsibleMode::detect(),
                passwordless_sudo: false,
            };
            bootstrap::bootstrap(&options, ask)?;
            Reply::Done
        }
        Op::Apply { modules } => {
            // no key yet just means no settings; a damaged key is an error
            let identity = crypto::load_default()?;
            apply::apply(host, identity.as_ref(), modules, ask)?;
            Reply::Done
        }
        Op::LoadDocument => Reply::Document(settings::load_document(host, &key()?, ask)?),
        Op::SaveDocument { plain, updates } => {
            let updates: Vec<(String, String)> = updates
                .iter()
                .map(|u| (u.path.clone(), u.value.expose().to_string()))
                .collect();
            let saved = settings::save_document(host, &key()?, plain, &updates, ask);
            // the plain copies of the secrets go before anything else can fail
            drop(zeroize_pairs(updates));
            saved?;
            Reply::Done
        }
        Op::Probe => Reply::Internal(network::probe(&session()?)?),
        Op::ChangeUsers { changes } => {
            let changes: Vec<users::Change> = changes.iter().map(Into::into).collect();
            users::change_all(host, &key()?, &changes, ask)?;
            Reply::Done
        }
        Op::ResetMachine { name } => {
            nt4::reset_machine(&session()?, name, ask)?;
            Reply::Done
        }
        Op::CreateAdmin {
            name,
            password,
            ssh_key,
            passwordless_sudo,
        } => {
            account::create_admin(
                &session()?,
                host,
                name,
                password.expose(),
                ssh_key.as_deref(),
                *passwordless_sudo,
                ask,
            )?;
            Reply::Done
        }
        Op::Leases => Reply::Leases(leases::read_on(host, ask)?),
        Op::LogonScripts => Reply::Names(logon::list(&session()?, ask)?),
        Op::ReadLogon { name } => Reply::Text(logon::read(&session()?, name, ask)?),
        Op::ReadGeneratedLogon => Reply::Text(logon::read_generated(&session()?, ask)?),
        Op::WriteLogon { name, text } => {
            logon::write(&session()?, name, text, ask)?;
            Reply::Done
        }
        Op::Run { command } => {
            let target = Target::Gateway { host: host.into() };
            Reply::ExitCode(exec::exec_detached(&target, command)?)
        }
        Op::Power { power } => Reply::Outcome(power::run(host, *power, ask)?),
        Op::ServerFacts => Reply::Facts(server::read(&session()?)?),
        Op::NetworkStatus => Reply::Interfaces(netstatus::read(&session()?)?),
        Op::PurgePreview { remove_admins } => Reply::Lines(purge::preview(
            &session()?,
            PurgeOptions {
                remove_admins: *remove_admins,
            },
            ask,
        )?),
        Op::Purge { remove_admins } => Reply::Lines(purge::purge(
            &session()?,
            host,
            PurgeOptions {
                remove_admins: *remove_admins,
            },
            ask,
        )?),
        Op::ClockState => Reply::Clock(clock::read(&session()?)?),
        Op::ClockZones => Reply::Names(clock::zones(&session()?)?),
        Op::ClockZoneNow { zone } => Reply::ZoneNow(clock::zone_now(&session()?, zone)?),
        Op::ClockApply { change } => {
            // no key just means the change is not kept in the settings
            let identity = crypto::load_default()?;
            let (lines, stored) =
                clock::apply_stored(&session()?, host, identity.as_ref(), change, ask)?;
            Reply::ClockApplied { lines, stored }
        }
        Op::ClockUpdateNow => Reply::Lines(clock::update_now(&session()?, ask)?),
    })
}

/// wipe the plain copies of secrets once they have been handed on.
fn zeroize_pairs(mut pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    use zeroize::Zeroize;
    for (_, value) in &mut pairs {
        value.zeroize();
    }
    pairs
}
