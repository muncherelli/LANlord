//! internet access for the lanlord networks: change it, and take the grants that have ended out
//! of the settings. the settings and the rules they turn into are described in
//! `lanlord_model::internet`; every change is stored and then applied with the docker module
//! (the nft rules, nat and forwarding) and the dns module (the router option and dns forwarding).

use anyhow::{bail, Result};
use lanlord_model::internet::{edit_settings, prune_settings, Change};

use crate::apply;
use crate::bootstrap::AskPassword;
use crate::lan::MODULES;
use crate::settings;
use crate::ssh::Session;
use crate::users::cached;

/// seconds since 1970, by this machine's clock.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// make `change` in the server's settings and apply the docker and dns modules.
pub fn change(
    session: &Session,
    host: &str,
    identity: &age::x25519::Identity,
    change: &Change,
    ask_password: AskPassword,
) -> Result<()> {
    let mut ask = cached(ask_password);
    let stored = settings::load_document_with(session, identity, &mut ask)?;
    if stored == settings::Document::default() {
        bail!("no settings on {host}; run lanlord config push first");
    }
    let edited = edit_settings(&stored.plain, change, now())?;
    settings::save_document_with(session, host, identity, &edited, &[], &mut ask)?;
    let modules = MODULES.map(String::from);
    apply::apply_with(session, host, Some(identity), &modules, &mut ask)
}

/// take the grants that have ended out of the settings and, when any went, apply the docker
/// module so the rules match. `refresh` applies it anyway: after a boot, the rules were loaded
/// with the time left when they were written. runs on the gateway as root (the timer).
pub fn prune(
    session: &Session,
    identity: &age::x25519::Identity,
    refresh: bool,
    ask_password: AskPassword,
) -> Result<usize> {
    let mut ask = cached(ask_password);
    let stored = settings::load_document_with(session, identity, &mut ask)?;
    let (plain, gone) = prune_settings(&stored.plain, now())?;
    if gone > 0 {
        settings::save_document_with(session, "localhost", identity, &plain, &[], &mut ask)?;
    }
    if gone > 0 || refresh {
        let modules = MODULES.map(String::from);
        apply::apply_with(session, "localhost", Some(identity), &modules, &mut ask)?;
    }
    Ok(gone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_is_past_2024() {
        assert!(now() > 1_700_000_000);
    }
}
