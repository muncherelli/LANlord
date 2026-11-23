//! the nt4-domain settings a front end edits: the domain, its server name,
//! the machines to prepare accounts for and roaming profiles. logins and
//! secrets are handled elsewhere; keys this type does not know stay as they are.

use anyhow::{bail, Result};

use crate::apply;
use crate::bootstrap::{capture_as_root, AskPassword};
use crate::config;
use crate::log::{emit, Kind};
use crate::nt4_exec;
use crate::settings::{self, Settings};
use crate::ssh::Session;
use crate::users::cached;

pub use lanlord_model::nt4::*;

/// the machines your settings define on `host`.
pub fn machines_on(
    session: &Session,
    host: &str,
    identity: &age::x25519::Identity,
    ask_password: AskPassword,
) -> Result<Vec<String>> {
    let stored = settings::load_document_with(session, identity, ask_password)?;
    if stored == settings::Document::default() {
        bail!("no settings on {host}; run lanlord config push first");
    }
    Ok(Nt4Settings::from_settings(&Settings::parse(&stored.plain)?)?.machines)
}

/// set the account of machine `name` back to its first password, inside the container. the machine
/// must be taken out of the domain and joined again afterwards.
pub fn reset_machine(session: &Session, name: &str, ask_password: AskPassword) -> Result<()> {
    if !valid_machine(name) {
        bail!("{name} is not a valid machine name (up to 15 letters, digits or -)");
    }
    let admin = config::admin(session)?;
    // the container's accounts are lower case
    let name = name.to_ascii_lowercase();
    let container = nt4_exec::container_of(session, ask_password, &name)?;
    let script = format!(
        "docker exec {container} lanlord-user machine {name} reset\n",
        container = crate::ssh::shell_quote(&container)
    );
    let out = capture_as_root(session, &admin, script, ask_password)?;
    emit(Kind::Info, out.trim());
    Ok(())
}

/// change the machines in your settings on `host` and apply the module; a reset only touches the
/// account on the gateway.
pub fn change_machine(
    session: &Session,
    host: &str,
    identity: &age::x25519::Identity,
    change: &MachineChange,
    ask_password: AskPassword,
) -> Result<()> {
    if let MachineChange::Reset(name) = change {
        return reset_machine(session, name, ask_password);
    }
    change_section(session, host, identity, ask_password, |plain| {
        edit_machines(plain, change)
    })
}

/// turn the software share on or off in your settings on `host` and apply the module.
pub fn set_software(
    session: &Session,
    host: &str,
    identity: &age::x25519::Identity,
    on: bool,
    ask_password: AskPassword,
) -> Result<()> {
    change_section(session, host, identity, ask_password, |plain| {
        software_edit(plain, on)
    })
}

/// the plain settings with the software share on or off.
pub fn software_edit(plain: &str, on: bool) -> Result<String> {
    let mut settings = Settings::parse(plain)?;
    let mut nt4 = Nt4Settings::from_settings(&settings)?;
    nt4.software = on;
    nt4.apply_to(&mut settings);
    Ok(settings.text())
}

/// edit the plain settings with `edit`, store them and apply the nt4-domain module.
fn change_section(
    session: &Session,
    host: &str,
    identity: &age::x25519::Identity,
    ask_password: AskPassword,
    edit: impl FnOnce(&str) -> Result<String>,
) -> Result<()> {
    let mut ask = cached(ask_password);
    let stored = settings::load_document_with(session, identity, &mut ask)?;
    if stored == settings::Document::default() {
        bail!("no settings on {host}; run lanlord config push first");
    }
    let edited = edit(&stored.plain)?;
    settings::save_document_with(session, host, identity, &edited, &[], &mut ask)?;
    apply::apply_with(
        session,
        host,
        Some(identity),
        &["nt4-domain".to_string()],
        &mut ask,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "docker: {}\nnt4-domain:\n  mode: workgroup\n  name: lab\n  address: 172.16.120.1/22\n  machines: [ws1, ws2]\n  users:\n    - name: dana\n";

    #[test]
    fn a_missing_section_gives_the_defaults() {
        let settings = Settings::parse("docker: {}\n").unwrap();
        assert_eq!(
            Nt4Settings::from_settings(&settings).unwrap(),
            Nt4Settings::default()
        );
        let null = Settings::parse("nt4-domain:\n").unwrap();
        assert_eq!(
            Nt4Settings::from_settings(&null).unwrap(),
            Nt4Settings::default()
        );
    }

    #[test]
    fn the_section_is_read_and_written_keeping_other_keys() {
        let mut settings = Settings::parse(TEXT).unwrap();
        let mut nt4 = Nt4Settings::from_settings(&settings).unwrap();
        assert_eq!(
            (nt4.mode.as_str(), nt4.name.as_str(), nt4.machines.len()),
            ("workgroup", "lab", 2)
        );
        nt4.netbios_name = "gw".into();
        nt4.roaming_profiles = true;
        nt4.software = true;
        nt4.machines.push("ws3".into());
        nt4.apply_to(&mut settings);
        let text = settings.text();
        assert!(text.contains("address: 172.16.120.1/22") && text.contains("dana"));
        assert!(text.contains("ws3") && text.contains("netbios_name: gw"));
        let back = Nt4Settings::from_settings(&settings).unwrap();
        assert_eq!(back, nt4);
        // clearing the server name removes the key again
        nt4.netbios_name.clear();
        nt4.apply_to(&mut settings);
        assert!(!settings.text().contains("netbios_name"));
    }

    #[test]
    fn bad_sections_are_refused() {
        for text in ["nt4-domain: 3\n", "nt4-domain:\n  machines: x\n"] {
            let settings = Settings::parse(text).unwrap();
            assert!(Nt4Settings::from_settings(&settings).is_err(), "{text}");
        }
    }

    #[test]
    fn validation_follows_the_playbook() {
        let mut nt4 = Nt4Settings::default();
        assert!(nt4.validate(true).is_empty());
        let errors = nt4.validate(false);
        assert_eq!(errors[0].path, "nt4-domain.admin_password");
        nt4.mode = "workgroup".into();
        assert!(nt4.validate(false).is_empty(), "a workgroup needs none");
        nt4.mode = "odd".into();
        nt4.name = "toolongfordomainnames".into();
        nt4.netbios_name = "-x".into();
        nt4.machines = vec!["ok-1".into(), "1bad".into(), "has space".into(), "".into()];
        let paths: Vec<_> = nt4.validate(true).into_iter().map(|e| e.path).collect();
        assert_eq!(
            paths,
            [
                "nt4-domain.mode",
                "nt4-domain.name",
                "nt4-domain.netbios_name",
                "nt4-domain.machines[1]",
                "nt4-domain.machines[2]",
                "nt4-domain.machines[3]"
            ]
        );
    }

    #[test]
    fn machines_are_added_once_and_removed() {
        let added = edit_machines(TEXT, &MachineChange::Add("nt4pc".into())).unwrap();
        let twice = edit_machines(&added, &MachineChange::Add("NT4PC".into())).unwrap();
        let machines = |t: &str| {
            Nt4Settings::from_settings(&Settings::parse(t).unwrap())
                .unwrap()
                .machines
        };
        assert_eq!(machines(&twice), ["ws1", "ws2", "nt4pc"]);
        let removed = edit_machines(&twice, &MachineChange::Remove("WS1".into())).unwrap();
        assert_eq!(machines(&removed), ["ws2", "nt4pc"]);
        assert!(removed.contains("dana"));
        assert!(edit_machines(TEXT, &MachineChange::Add("bad name".into())).is_err());
        // a reset does not touch the settings
        let reset = edit_machines(TEXT, &MachineChange::Reset("ws1".into())).unwrap();
        assert_eq!(machines(&reset), machines(TEXT));
    }

    #[cfg(unix)]
    mod gateway {
        use super::*;
        use crate::testgw::{identity, no_password, Gateway};

        const STORED: &str =
            "docker: {}\nnt4-domain:\n  mode: domain\n  name: RETRONET\n  machines: [ws1]\n";

        #[test]
        fn machines_are_listed_changed_and_applied() {
            let (gw, id) = (Gateway::new("nt4-machines"), identity("nt4-machines"));
            gw.seed_sealed(&id, STORED);
            let listed = machines_on(&gw.session, "gw", &id, &mut no_password).unwrap();
            assert_eq!(listed, ["ws1"]);
            let add = MachineChange::Add("nt4pc".into());
            change_machine(&gw.session, "gw", &id, &add, &mut no_password).unwrap();
            assert!(gw.plain().unwrap().contains("nt4pc"));
            set_software(&gw.session, "gw", &id, true, &mut no_password).unwrap();
            assert!(gw.plain().unwrap().contains("software: true"));
        }

        #[test]
        fn a_reset_runs_in_the_container_and_leaves_the_settings_alone() {
            let (gw, id) = (Gateway::new("nt4-reset"), identity("nt4-reset"));
            gw.seed_sealed(&id, STORED);
            let before = gw.plain();
            let record = gw.dir.join("sent");
            let session = Session::fake_gateway(true, &record, 0);
            let reset = MachineChange::Reset("WS1".into());
            change_machine(&session, "gw", &id, &reset, &mut no_password).unwrap();
            let sent = std::fs::read_to_string(&record).unwrap();
            assert!(sent.contains("docker exec lanlord-nt4 lanlord-user machine ws1 reset"));
            assert_eq!(gw.plain(), before);
        }

        #[test]
        fn a_reset_refuses_bad_names_and_reports_failure() {
            let record =
                std::env::temp_dir().join(format!("lanlord-nt4-bad-{}", std::process::id()));
            let ok = Session::fake_gateway(true, &record, 0);
            assert!(reset_machine(&ok, "bad name; rm", &mut no_password).is_err());
            assert!(!record.exists(), "nothing is sent for a bad name");
            let failing = Session::fake_gateway(true, &record, 3);
            assert!(reset_machine(&failing, "ws1", &mut no_password).is_err());
            let _ = std::fs::remove_file(&record);
        }

        #[test]
        fn machines_need_settings_on_the_gateway() {
            let (gw, id) = (Gateway::new("nt4-none"), identity("nt4-none"));
            assert!(machines_on(&gw.session, "gw", &id, &mut no_password).is_err());
            let add = MachineChange::Add("x".into());
            assert!(change_machine(&gw.session, "gw", &id, &add, &mut no_password).is_err());
        }
    }
}
