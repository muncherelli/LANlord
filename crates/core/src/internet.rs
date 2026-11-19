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

    #[cfg(unix)]
    mod gateway {
        use super::*;
        use crate::testgw::{identity, no_password, Gateway};
        use lanlord_model::internet::Expiry;

        const LAB: &str = "docker: {}\nnetworks:\n  - name: lab\n    interface: eth2\n    address: 192.0.2.1/24\n";
        const MAC: &str = "02:00:00:00:00:0a";

        fn seeded(name: &str, extra: &str) -> (Gateway, age::x25519::Identity) {
            let (gw, id) = (Gateway::new(name), identity(name));
            gw.seed_sealed(&id, &format!("{LAB}{extra}"));
            (gw, id)
        }

        #[test]
        fn a_network_is_switched_on_and_off_and_stored() {
            let (gw, id) = seeded("inet-switch", "");
            let on = Change::Network {
                network: "lab".into(),
                on: true,
            };
            change(&gw.session, "gw", &id, &on, &mut no_password).unwrap();
            assert!(gw.plain().unwrap().contains("internet: true"));
            let off = Change::Network {
                network: "lab".into(),
                on: false,
            };
            change(&gw.session, "gw", &id, &off, &mut no_password).unwrap();
            assert!(!gw.plain().unwrap().contains("internet: true"));
        }

        #[test]
        fn a_computer_is_allowed_for_a_while_and_denied() {
            let (gw, id) = seeded("inet-allow", "");
            let allow = Change::Allow {
                network: "lab".into(),
                mac: MAC.into(),
                name: Some("dana-pc".into()),
                expiry: Expiry::At(now() + 3_600),
            };
            change(&gw.session, "gw", &id, &allow, &mut no_password).unwrap();
            let plain = gw.plain().unwrap();
            assert!(plain.contains(MAC) && plain.contains("until:"), "{plain}");
            let deny = Change::Deny {
                network: "lab".into(),
                mac: MAC.into(),
            };
            change(&gw.session, "gw", &id, &deny, &mut no_password).unwrap();
            assert!(!gw.plain().unwrap().contains(MAC));
        }

        #[test]
        fn a_change_that_cannot_be_made_changes_nothing() {
            let (gw, id) = seeded("inet-bad", "");
            let before = gw.plain();
            let unknown = Change::Network {
                network: "nowhere".into(),
                on: true,
            };
            assert!(change(&gw.session, "gw", &id, &unknown, &mut no_password).is_err());
            let past = Change::Allow {
                network: "lab".into(),
                mac: MAC.into(),
                name: None,
                expiry: Expiry::At(now() - 60),
            };
            assert!(change(&gw.session, "gw", &id, &past, &mut no_password).is_err());
            assert_eq!(gw.plain(), before);
        }

        #[test]
        fn a_server_without_settings_is_refused_plainly() {
            let (gw, id) = (Gateway::new("inet-none"), identity("inet-none"));
            let on = Change::Network {
                network: "lab".into(),
                on: true,
            };
            let err = change(&gw.session, "gw", &id, &on, &mut no_password).unwrap_err();
            assert!(err.to_string().contains("no settings"), "{err}");
            // the timer finds nothing to take out of settings that are not there yet
            assert_eq!(prune(&gw.session, &id, false, &mut no_password).unwrap(), 0);
        }

        #[test]
        fn prune_takes_out_the_grants_that_ended_and_leaves_the_rest() {
            let ended = format!(
                "    internet_allow:\n      - {{mac: \"{MAC}\", until: \"2001-01-01T00:00:00Z\"}}\n      - {{mac: \"02:00:00:00:00:0b\"}}\n"
            );
            let (gw, id) = seeded("inet-prune", &ended);
            assert_eq!(prune(&gw.session, &id, false, &mut no_password).unwrap(), 1);
            let plain = gw.plain().unwrap();
            assert!(
                !plain.contains(MAC) && plain.contains("02:00:00:00:00:0b"),
                "{plain}"
            );
            // nothing left to take out, and a refresh still applies
            assert_eq!(prune(&gw.session, &id, false, &mut no_password).unwrap(), 0);
            assert_eq!(prune(&gw.session, &id, true, &mut no_password).unwrap(), 0);
        }
    }
}
