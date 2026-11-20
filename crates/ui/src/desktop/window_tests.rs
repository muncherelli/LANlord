//! headless tests for the management windows, with a fake gateway.

use std::net::Ipv4Addr;
use std::sync::Arc;

use eframe::egui;
use lanlord_model::leases::Lease;
use lanlord_model::network::Internal;

use super::harness::{
    desktop, enabled, frame_on, frames, frames_on, idle, press, until, with_host,
};
use super::{window_size, Desktop, Target, MODULES};
use crate::nt4_window::Tab;
use lanlord_app::gateway::Fake;

const NT4: &str =
    "nt4-domain:\n  mode: domain\n  name: RETRONET\n  users:\n    - name: dana\n      admin: true\n";
const DNS: &str = "dns:\n  zone: retronet\n  dhcp:\n    range: [172.16.120.100, 172.16.120.200]\n    lease: 7d\n    hosts:\n      - {name: a, mac: 'aa:bb:cc:dd:ee:ff', address: 172.16.120.10}\n";

fn module(id: &str) -> Target {
    Target::Module(MODULES.iter().position(|m| m.id == id).unwrap())
}

fn open(id: &str, stored: &str) -> (Desktop, egui::Context, Arc<Fake>) {
    let (mut desktop, ctx, fake) = desktop();
    *fake.stored.lock().unwrap() = stored.into();
    desktop.open(module(id));
    with_host(&mut desktop, &ctx);
    (desktop, ctx, fake)
}

fn settled(desktop: &mut Desktop, ctx: &egui::Context) {
    until(desktop, ctx, idle("me@gw"));
    // a status bar slides in, and the window follows it, over a few frames
    let mut still = 0;
    let mut last = desktop.measured.clone();
    for _ in 0..200 {
        frames(desktop, ctx, 1);
        still = if desktop.measured == last {
            still + 1
        } else {
            0
        };
        last = desktop.measured.clone();
        if still >= 4 {
            frames(desktop, ctx, 2);
            return;
        }
    }
    panic!("the windows never settled");
}

fn stored(fake: &Fake) -> String {
    fake.stored.lock().unwrap().clone()
}

fn secret_paths(fake: &Fake) -> Vec<String> {
    fake.secrets
        .lock()
        .unwrap()
        .iter()
        .map(|(p, _)| p.clone())
        .collect()
}

fn logins() -> (Desktop, egui::Context, Arc<Fake>) {
    let (mut desktop, ctx, fake) = desktop();
    *fake.stored.lock().unwrap() = NT4.into();
    desktop.open(module("nt4-domain"));
    desktop.nt4.tab = Tab::Logins;
    with_host(&mut desktop, &ctx);
    until(&mut desktop, &ctx, |d| d.nt4.users.belongs_to("me@gw"));
    settled(&mut desktop, &ctx);
    (desktop, ctx, fake)
}

/// fill the open form's fields.
fn fill(form: Option<&mut crate::form::Form>, values: &[&str]) {
    let form = form.expect("a form is open");
    for (i, value) in values.iter().enumerate() {
        form.set(i, value);
    }
}

#[test]
fn logins_are_listed_added_with_a_generated_password_and_kept_out_of_the_plain_text() {
    let (mut desktop, ctx, fake) = logins();
    assert_eq!(desktop.nt4.users.names(), ["dana"]);
    assert!(press(&mut desktop, &ctx, "Add login..."));
    fill(desktop.nt4.users.form_mut(), &["olive"]);
    assert!(press(&mut desktop, &ctx, "Generate"));
    let generated = desktop.nt4.users.form_mut().unwrap().raw(1).to_string();
    assert_eq!(generated.len(), 12);
    desktop.nt4.users.form_mut().unwrap().checks[0].1 = true;
    desktop.nt4.users.form_mut().unwrap().checks[1].1 = true;
    assert!(press(&mut desktop, &ctx, "OK"));
    settled(&mut desktop, &ctx);
    assert_eq!(desktop.nt4.users.names(), ["dana", "olive"]);
    let plain = stored(&fake);
    assert!(
        plain.contains("olive") && !plain.contains(&generated),
        "{plain}"
    );
    assert!(plain.contains("software_admin: true"), "{plain}");
    assert_eq!(
        fake.secrets.lock().unwrap().clone(),
        [("nt4-domain.users[olive].password".to_string(), generated)]
    );
    assert_eq!(
        fake.calls(),
        [
            "load me@gw",
            "users me@gw 3",
            "apply me@gw [nt4-domain]",
            "load me@gw"
        ]
    );
}

#[test]
fn a_bad_add_keeps_the_form_open_and_cancel_changes_nothing() {
    let (mut desktop, ctx, fake) = logins();
    assert!(press(&mut desktop, &ctx, "Add login..."));
    for (name, password) in [("Bad Name", "x"), ("dana", "x"), ("olive", "")] {
        fill(desktop.nt4.users.form_mut(), &[name, password]);
        assert!(press(&mut desktop, &ctx, "OK"));
        let form = desktop.nt4.users.form_mut().expect("still open");
        assert!(form.error.is_some(), "{name}");
    }
    assert!(press(&mut desktop, &ctx, "Cancel"));
    frames(&mut desktop, &ctx, 2);
    assert!(desktop.nt4.users.form_mut().is_none());
    assert_eq!(fake.calls(), ["load me@gw"]);
}

#[test]
fn passwords_admin_flags_and_deletion_need_a_selection_and_go_through_the_task_runner() {
    let (mut desktop, ctx, fake) = logins();
    assert_eq!(
        enabled(&ctx, "Password..."),
        Some(false),
        "nothing selected"
    );
    desktop.nt4.users.select("dana");
    frames(&mut desktop, &ctx, 2);

    assert!(press(&mut desktop, &ctx, "Password..."));
    fill(desktop.nt4.users.form_mut(), &["N3WPASS"]);
    assert!(press(&mut desktop, &ctx, "OK"));
    settled(&mut desktop, &ctx);
    assert_eq!(
        fake.secrets.lock().unwrap()[0],
        (
            "nt4-domain.users[dana].password".to_string(),
            "N3WPASS".to_string()
        )
    );

    assert_eq!(
        enabled(&ctx, "Make software admin"),
        Some(false),
        "domain admins write to the software share already"
    );
    assert!(press(&mut desktop, &ctx, "Remove domain admin"));
    settled(&mut desktop, &ctx);
    assert!(!stored(&fake).contains("admin: true"));
    assert!(enabled(&ctx, "Make domain admin").is_some());

    assert!(press(&mut desktop, &ctx, "Make software admin"));
    settled(&mut desktop, &ctx);
    assert!(stored(&fake).contains("software_admin: true"));
    assert!(press(&mut desktop, &ctx, "Remove software admin"));
    settled(&mut desktop, &ctx);
    assert!(!stored(&fake).contains("software_admin"));

    assert!(press(&mut desktop, &ctx, "Delete..."));
    assert!(
        fake.calls().iter().all(|c| !c.contains("Remove")),
        "asks first"
    );
    assert!(press(&mut desktop, &ctx, "OK"));
    settled(&mut desktop, &ctx);
    assert!(desktop.nt4.users.names().is_empty());
    assert!(stored(&fake).contains("state: absent"));
    assert!(
        secret_paths(&fake).is_empty(),
        "the password goes with the login"
    );
}

#[test]
fn logins_without_a_domain_section_explain_themselves_and_reload_works() {
    let (mut desktop, ctx, fake) = open("nt4-domain", "docker: {}\n");
    desktop.nt4.tab = Tab::Logins;
    // a transient "the gateway is busy" note comes first when another read holds the host
    until(&mut desktop, &ctx, |d| {
        d.nt4
            .users
            .note()
            .is_some_and(|n| n.0 && n.1.contains("no nt4-domain section"))
    });
    *fake.stored.lock().unwrap() = NT4.into();
    assert!(press(&mut desktop, &ctx, "Reload logins"));
    until(&mut desktop, &ctx, |d| d.nt4.users.belongs_to("me@gw"));
    assert_eq!(desktop.nt4.users.names(), ["dana"]);
}

fn dns() -> (Desktop, egui::Context, Arc<Fake>) {
    let (mut desktop, ctx, fake) = desktop();
    *fake.stored.lock().unwrap() = DNS.into();
    *fake.internal.lock().unwrap() = Some(Internal {
        interface: "eth1".into(),
        address: Ipv4Addr::new(172, 16, 120, 2),
        prefix: 22,
    });
    fake.leases.lock().unwrap().push(Lease {
        expires: 0,
        mac: "aa:bb:cc:dd:ee:00".into(),
        address: "172.16.120.101".into(),
        name: "win98".into(),
    });
    desktop.open(module("dns"));
    with_host(&mut desktop, &ctx);
    until(&mut desktop, &ctx, |d| d.dns.belongs_to("me@gw"));
    frames(&mut desktop, &ctx, 2);
    (desktop, ctx, fake)
}

#[test]
fn the_dns_window_loads_makes_a_lease_static_and_saves_and_applies() {
    let (mut desktop, ctx, fake) = dns();
    assert!(desktop.dns.problems().is_empty());
    desktop.dns.tab = 3;
    frames(&mut desktop, &ctx, 2);
    assert_eq!(enabled(&ctx, "Make static"), Some(false), "no row picked");
    desktop.dns.select_current(0);
    assert!(press(&mut desktop, &ctx, "Make static"));
    let form = desktop.dns.form_mut().unwrap();
    assert_eq!(
        (form.value(0), form.value(1)),
        ("win98", "aa:bb:cc:dd:ee:00")
    );
    form.set(2, "172.16.120.101");
    assert!(press(&mut desktop, &ctx, "OK"));
    assert_eq!(desktop.dns.draft.dhcp.as_ref().unwrap().hosts.len(), 2);
    assert!(desktop.dns.dirty());

    assert!(press(&mut desktop, &ctx, "Save DNS & apply"));
    settled(&mut desktop, &ctx);
    assert!(stored(&fake).contains("win98"));
    assert!(fake.calls().contains(&"apply me@gw [dns]".to_string()));
    assert!(!desktop.dns.dirty());
}

#[test]
fn saving_without_applying_only_stores_the_plain_settings() {
    let (mut desktop, ctx, fake) = dns();
    desktop.dns.draft.zone = "lab".into();
    assert!(press(&mut desktop, &ctx, "Save DNS"));
    settled(&mut desktop, &ctx);
    assert!(stored(&fake).contains("zone: lab"));
    assert!(fake.calls().iter().all(|c| !c.starts_with("apply")));
    desktop.dns.draft.zone = "again".into();
    assert!(press(&mut desktop, &ctx, "Reload DNS"));
    settled(&mut desktop, &ctx);
    assert_eq!(desktop.dns.draft.zone, "lab");
}

#[test]
fn problems_are_shown_and_block_saving() {
    let (mut desktop, ctx, fake) = dns();
    desktop.dns.draft.zone = "Not A Label".into();
    desktop
        .dns
        .draft
        .records
        .push(lanlord_model::settings::Record {
            name: "ok".into(),
            address: "10.0.0.1".into(),
        });
    frames(&mut desktop, &ctx, 2);
    let paths: Vec<_> = desktop.dns.problems().into_iter().map(|e| e.path).collect();
    assert_eq!(paths, ["dns.zone", "dns.records[0].address"]);
    assert_eq!(enabled(&ctx, "Save DNS"), Some(false));
    assert_eq!(enabled(&ctx, "Save DNS & apply"), Some(false));
    assert!(!press(&mut desktop, &ctx, "Save DNS"));
    assert_eq!(fake.calls().len(), 3, "only the load ran");
}

#[test]
fn leases_and_records_are_added_edited_and_removed() {
    let (mut desktop, ctx, _) = dns();
    desktop.dns.tab = 1;
    assert!(press(&mut desktop, &ctx, "Add lease..."));
    fill(
        desktop.dns.form_mut(),
        &["b", "AA:BB:CC:DD:EE:01", "172.16.120.11"],
    );
    assert!(press(&mut desktop, &ctx, "OK"));
    let hosts = || desktop_hosts(&desktop);
    assert_eq!(hosts(), ["a", "b"]);
    desktop.dns.select_lease(1);
    assert!(press(&mut desktop, &ctx, "Edit lease..."));
    fill(desktop.dns.form_mut(), &["c"]);
    assert!(press(&mut desktop, &ctx, "OK"));
    assert_eq!(desktop_hosts(&desktop), ["a", "c"]);
    desktop.dns.select_lease(0);
    assert!(press(&mut desktop, &ctx, "Remove lease"));
    assert_eq!(desktop_hosts(&desktop), ["c"]);

    desktop.dns.tab = 2;
    assert!(press(&mut desktop, &ctx, "Add record..."));
    assert!(press(&mut desktop, &ctx, "OK"));
    assert!(
        desktop.dns.form_mut().unwrap().error.is_some(),
        "blank is refused"
    );
    fill(desktop.dns.form_mut(), &["srv", "172.16.120.20"]);
    assert!(press(&mut desktop, &ctx, "OK"));
    assert_eq!(desktop.dns.draft.records.len(), 1);
    desktop.dns.select_record(0);
    assert!(press(&mut desktop, &ctx, "Edit record..."));
    fill(desktop.dns.form_mut(), &["srv2"]);
    assert!(press(&mut desktop, &ctx, "OK"));
    assert_eq!(desktop.dns.draft.records[0].name, "srv2");
    assert!(press(&mut desktop, &ctx, "Remove record"));
    assert!(desktop.dns.draft.records.is_empty());
}

fn desktop_hosts(desktop: &Desktop) -> Vec<String> {
    let dhcp = desktop.dns.draft.dhcp.as_ref().unwrap();
    dhcp.hosts.iter().map(|h| h.name.clone()).collect()
}

#[test]
fn dhcp_can_be_switched_off_and_a_failed_probe_is_reported() {
    let (mut desktop, ctx, fake) = desktop();
    *fake.stored.lock().unwrap() = "dns: {}\n".into();
    desktop.open(module("dns"));
    with_host(&mut desktop, &ctx);
    until(&mut desktop, &ctx, |d| d.dns.belongs_to("me@gw"));
    frames(&mut desktop, &ctx, 2);
    assert!(desktop.dns.draft.dhcp.is_none());
    desktop.dns.tab = 1;
    frames(&mut desktop, &ctx, 2);
    assert_eq!(enabled(&ctx, "Add lease..."), Some(false));
    assert_eq!(
        enabled(&ctx, "Save DNS"),
        Some(true),
        "unchecked addresses still save"
    );
    desktop.dns.draft.dhcp = Some(lanlord_model::settings::Dhcp::default());
    frames(&mut desktop, &ctx, 2);
    assert_eq!(enabled(&ctx, "Add lease..."), Some(true));
}

fn nt4() -> (Desktop, egui::Context, Arc<Fake>) {
    let (mut desktop, ctx, fake) = open("nt4-domain", NT4);
    until(&mut desktop, &ctx, |d| d.nt4.settings.belongs_to("me@gw"));
    frames(&mut desktop, &ctx, 2);
    (desktop, ctx, fake)
}

#[test]
fn the_nt4_settings_tab_needs_an_admin_password_for_a_domain_and_never_shows_it() {
    let (mut desktop, ctx, fake) = nt4();
    assert_eq!(desktop.nt4.settings.draft.name, "RETRONET");
    assert_eq!(
        enabled(&ctx, "Save settings"),
        Some(false),
        "no admin password yet"
    );
    assert!(press(&mut desktop, &ctx, "Set password..."));
    fill(desktop.nt4.settings.form_mut(), &["Adm1nPass"]);
    assert!(press(&mut desktop, &ctx, "OK"));
    assert!(desktop.nt4.settings.has_new_password());
    assert!(desktop.nt4.settings.dirty());

    assert!(press(&mut desktop, &ctx, "Add machine..."));
    fill(desktop.nt4.settings.form_mut(), &["1bad"]);
    assert!(press(&mut desktop, &ctx, "OK"));
    assert!(desktop.nt4.settings.form_mut().unwrap().error.is_some());
    fill(desktop.nt4.settings.form_mut(), &["ws1"]);
    assert!(press(&mut desktop, &ctx, "OK"));
    desktop.nt4.settings.draft.roaming_profiles = true;

    assert!(press(&mut desktop, &ctx, "Save & apply"));
    settled(&mut desktop, &ctx);
    let plain = stored(&fake);
    assert!(plain.contains("ws1") && plain.contains("roaming_profiles: true"));
    assert!(
        plain.contains("dana") && !plain.contains("Adm1nPass"),
        "{plain}"
    );
    assert_eq!(secret_paths(&fake), ["nt4-domain.admin_password"]);
    assert!(fake
        .calls()
        .contains(&"apply me@gw [nt4-domain]".to_string()));
    assert!(!desktop.nt4.settings.has_new_password());
    assert_eq!(desktop.nt4.settings.machines(), ["ws1"]);
}

#[test]
fn machines_are_removed_and_a_workgroup_needs_no_password() {
    let stored_text = "nt4-domain:\n  mode: workgroup\n  name: LAB\n  machines: [ws1, ws2]\n";
    let (mut desktop, ctx, fake) = open("nt4-domain", stored_text);
    until(&mut desktop, &ctx, |d| d.nt4.settings.belongs_to("me@gw"));
    settled(&mut desktop, &ctx);
    assert_eq!(enabled(&ctx, "Save settings"), Some(true));
    desktop.nt4.settings.draft.machines.push("ws3".into());
    assert!(press(&mut desktop, &ctx, "Save settings"));
    settled(&mut desktop, &ctx);
    assert!(stored(&fake).contains("ws3"));
    assert!(fake.calls().iter().all(|c| !c.starts_with("apply")));
    assert!(press(&mut desktop, &ctx, "Reload settings"));
    settled(&mut desktop, &ctx);
    assert_eq!(desktop.nt4.settings.draft.machines.len(), 3);
}

#[test]
fn a_machine_account_is_reset_only_after_the_confirmation() {
    let stored_text = "nt4-domain:\n  mode: domain\n  name: LAB\n  machines: [ws1, ws2]\n";
    let (mut desktop, ctx, fake) = open("nt4-domain", stored_text);
    until(&mut desktop, &ctx, |d| d.nt4.settings.belongs_to("me@gw"));
    settled(&mut desktop, &ctx);
    assert_eq!(
        enabled(&ctx, "Reset account..."),
        Some(false),
        "nothing selected"
    );
    desktop.nt4.settings.select(1);
    frames(&mut desktop, &ctx, 2);
    assert_eq!(enabled(&ctx, "Reset account..."), Some(true));
    assert!(press(&mut desktop, &ctx, "Reset account..."));
    frames(&mut desktop, &ctx, 2);
    assert!(fake.calls().iter().all(|c| !c.starts_with("reset")));
    assert!(press(&mut desktop, &ctx, "Cancel"));
    frames(&mut desktop, &ctx, 2);
    assert!(fake.calls().iter().all(|c| !c.starts_with("reset")));
    assert!(press(&mut desktop, &ctx, "Reset account..."));
    frames(&mut desktop, &ctx, 2);
    assert!(press(&mut desktop, &ctx, "OK"));
    settled(&mut desktop, &ctx);
    assert!(fake.calls().contains(&"reset me@gw ws2".to_string()));
}

#[test]
fn a_broken_nt4_section_is_reported() {
    let (mut desktop, ctx, _) = open("nt4-domain", "nt4-domain: 3\n");
    until(&mut desktop, &ctx, |d| {
        d.nt4.settings.note().is_some_and(|n| n.0)
    });
    frames(&mut desktop, &ctx, 2);
}

#[test]
fn remote_commands_run_through_sh_and_show_their_output() {
    let (mut desktop, ctx, fake) = open("remote-commands", "");
    frames(&mut desktop, &ctx, 2);
    assert_eq!(enabled(&ctx, "Run"), Some(false), "nothing typed");
    desktop.commands.line = "ls -l | wc".into();
    assert!(press(&mut desktop, &ctx, "Run"));
    until(&mut desktop, &ctx, |d| !d.commands.running());
    let text: Vec<String> = desktop
        .commands
        .output()
        .iter()
        .map(|l| l.text.clone())
        .collect();
    assert!(text.contains(&"$ ls -l | wc".to_string()), "{text:?}");
    assert!(text.contains(&"exit 0".to_string()), "{text:?}");
    assert!(text
        .iter()
        .any(|t| t.contains("output of sh -c ls -l | wc")));
    assert_eq!(fake.calls(), ["run me@gw sh -c ls -l | wc"]);
    assert_eq!(desktop.commands.history(), ["ls -l | wc"]);

    *fake.exit.lock().unwrap() = 3;
    desktop.commands.line = "false".into();
    assert!(press(&mut desktop, &ctx, "Run"));
    until(&mut desktop, &ctx, |d| !d.commands.running());
    assert!(desktop.commands.output().iter().any(|l| l.text == "exit 3"));
    assert_eq!(desktop.commands.history(), ["false", "ls -l | wc"]);

    *fake.fail.lock().unwrap() = Some("boom".into());
    desktop.commands.line = "x".into();
    assert!(press(&mut desktop, &ctx, "Run"));
    until(&mut desktop, &ctx, |d| !d.commands.running());
    assert!(desktop.commands.output().iter().any(|l| l.text == "boom"));
}

#[test]
fn remote_commands_list_the_nt4_machines_as_unavailable() {
    let (mut desktop, ctx, _) = open(
        "nt4-domain",
        "nt4-domain:\n  mode: workgroup\n  name: LAB\n  machines: [ws1]\n",
    );
    until(&mut desktop, &ctx, |d| d.nt4.settings.belongs_to("me@gw"));
    desktop.open(module("remote-commands"));
    frames(&mut desktop, &ctx, 3);
    assert_eq!(desktop.nt4.settings.machines(), ["ws1"]);
    desktop.commands.line = String::new();
    assert!(
        !press(&mut desktop, &ctx, "Run"),
        "an empty line does not run"
    );
}

#[test]
fn remote_commands_without_a_host_say_so() {
    let (mut desktop, ctx, fake) = desktop();
    desktop.open(module("remote-commands"));
    frames(&mut desktop, &ctx, 3);
    assert!(fake.calls().is_empty());
}

fn logon_tab() -> (Desktop, egui::Context, Arc<Fake>) {
    let (mut desktop, ctx, fake) = desktop();
    *fake.stored.lock().unwrap() = NT4.into();
    fake.logon
        .lock()
        .unwrap()
        .insert("logon".into(), "echo hi\n".into());
    *fake.generated.lock().unwrap() = Some("@echo off\n".into());
    desktop.open(module("nt4-domain"));
    desktop.nt4.tab = Tab::Logon;
    with_host(&mut desktop, &ctx);
    until(&mut desktop, &ctx, |d| d.nt4.logon.belongs_to("me@gw"));
    settled(&mut desktop, &ctx);
    (desktop, ctx, fake)
}

#[test]
fn the_logon_tab_opens_the_common_script_and_saves_edits() {
    let (mut desktop, ctx, fake) = logon_tab();
    assert_eq!(desktop.nt4.logon.selected(), "logon");
    assert_eq!(desktop.nt4.logon.draft_mut().as_str(), "echo hi\n");
    assert_eq!(enabled(&ctx, "Save script"), Some(false), "nothing changed");

    desktop.nt4.logon.draft_mut().push_str("echo two\n");
    frames(&mut desktop, &ctx, 2);
    assert_eq!(enabled(&ctx, "Save script"), Some(true));
    assert!(press(&mut desktop, &ctx, "Revert"));
    assert_eq!(desktop.nt4.logon.draft_mut().as_str(), "echo hi\n");

    desktop.nt4.logon.draft_mut().push_str("echo two\n");
    assert!(press(&mut desktop, &ctx, "Save script"));
    settled(&mut desktop, &ctx);
    assert_eq!(fake.logon.lock().unwrap()["logon"], "echo hi\necho two\n");
    assert!(fake
        .calls()
        .contains(&"logon-write me@gw logon".to_string()));
    assert_eq!(enabled(&ctx, "Save script"), Some(false), "saved");
}

#[test]
fn a_new_script_for_a_login_is_named_edited_and_saved() {
    let (mut desktop, ctx, fake) = logon_tab();
    assert!(press(&mut desktop, &ctx, "New script..."));
    fill(desktop.nt4.logon.form_mut(), &["lanlord"]);
    assert!(press(&mut desktop, &ctx, "OK"));
    assert!(
        desktop.nt4.logon.form_mut().unwrap().error.is_some(),
        "the generated script is reserved"
    );
    fill(desktop.nt4.logon.form_mut(), &["dana"]);
    assert!(press(&mut desktop, &ctx, "OK"));
    until(&mut desktop, &ctx, |d| d.nt4.logon.selected() == "dana");
    settled(&mut desktop, &ctx);
    assert_eq!(
        desktop.nt4.logon.entries_for_test(),
        ["logon", "dana"],
        "listed before it is saved"
    );
    assert_eq!(enabled(&ctx, "New script..."), Some(true));

    *desktop.nt4.logon.draft_mut() = "net use P: \\\\A\\b\n".into();
    frames(&mut desktop, &ctx, 2);
    assert_eq!(enabled(&ctx, "New script..."), Some(false), "unsaved edits");
    assert!(press(&mut desktop, &ctx, "Save script"));
    settled(&mut desktop, &ctx);
    assert_eq!(fake.logon.lock().unwrap()["dana"], "net use P: \\\\A\\b\n");
    assert_eq!(desktop.nt4.logon.entries_for_test(), ["logon", "dana"]);
}

#[test]
fn a_failing_gateway_is_reported_in_the_logon_tab() {
    let (mut desktop, ctx, fake) = logon_tab();
    *fake.fail.lock().unwrap() = Some("sudo said no".into());
    assert!(press(&mut desktop, &ctx, "Reload scripts"));
    settled(&mut desktop, &ctx);
    assert_eq!(
        desktop.nt4.logon.note(),
        Some(&(true, "sudo said no".to_string()))
    );
}

#[test]
fn the_console_opens_at_a_working_size_can_be_resized_and_keeps_the_size_the_user_gave_it() {
    use eframe::egui::{Event, PointerButton, Pos2, Vec2};
    use lanlord_app::log::{emit, Kind};
    let size = Vec2::new(1280.0, 720.0);
    let (mut desktop, ctx, _) = desktop();
    desktop.open(Target::Console);
    frames_on(&mut desktop, &ctx, size, 6);
    let rect = |d: &Desktop| {
        d.measured
            .iter()
            .find(|m| m.0 == Target::Console)
            .unwrap()
            .1
    };
    let opened = rect(&desktop);
    // an empty console does not collapse to its contents
    assert!(
        opened.height() >= 300.0 && opened.width() >= 600.0,
        "{opened:?}"
    );
    // drag the bottom right corner in, then out
    let drag = |desktop: &mut Desktop, ctx: &egui::Context, from: Pos2, to: Pos2| {
        let button = |pressed| Event::PointerButton {
            pos: to,
            button: PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frames_on(desktop, ctx, size, 1);
        frame_on(desktop, ctx, size, vec![Event::PointerMoved(from)]);
        frame_on(
            desktop,
            ctx,
            size,
            vec![Event::PointerButton {
                pos: from,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        for i in 1..=4 {
            let at = from + (to - from) * (i as f32 / 4.0);
            frame_on(desktop, ctx, size, vec![Event::PointerMoved(at)]);
        }
        frame_on(desktop, ctx, size, vec![button(false)]);
        frames_on(desktop, ctx, size, 3);
    };
    let corner = opened.right_bottom() - Vec2::splat(2.0);
    drag(&mut desktop, &ctx, corner, corner - Vec2::new(150.0, 120.0));
    let smaller = rect(&desktop);
    assert!(
        smaller.width() < opened.width() - 100.0 && smaller.height() < opened.height() - 80.0,
        "{opened:?} -> {smaller:?}"
    );
    let corner = smaller.right_bottom() - Vec2::splat(2.0);
    drag(&mut desktop, &ctx, corner, corner + Vec2::new(90.0, 70.0));
    let user = rect(&desktop);
    assert!(user.width() > smaller.width() + 60.0 && user.height() > smaller.height() + 40.0);
    // new output does not snap it back
    for n in 0..60 {
        emit(Kind::Out, format!("line {n}"));
    }
    frames_on(&mut desktop, &ctx, size, 8);
    let after = rect(&desktop);
    assert!(
        (after.size() - user.size()).abs().max_elem() < 1.5,
        "{user:?} -> {after:?}"
    );
}

/// drag the bottom right corner of `target`'s window by `by`, on a screen of `size`.
fn drag_corner(
    desktop: &mut Desktop,
    ctx: &egui::Context,
    size: egui::Vec2,
    target: Target,
    by: egui::Vec2,
) {
    use eframe::egui::{Event, PointerButton};
    let rect = |d: &Desktop| d.measured.iter().find(|m| m.0 == target).unwrap().1;
    let from = rect(desktop).right_bottom() - egui::Vec2::splat(2.0);
    let button = |at, pressed| Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame_on(desktop, ctx, size, vec![Event::PointerMoved(from)]);
    frame_on(desktop, ctx, size, vec![button(from, true)]);
    for i in 1..=4 {
        let at = from + by * (i as f32 / 4.0);
        frame_on(desktop, ctx, size, vec![Event::PointerMoved(at)]);
    }
    frame_on(desktop, ctx, size, vec![button(from + by, false)]);
    frames_on(desktop, ctx, size, 3);
}

#[test]
fn a_list_window_keeps_the_size_the_user_gave_it_and_never_goes_below_its_least() {
    use eframe::egui::Vec2;
    let size = Vec2::new(1280.0, 720.0);
    let (mut desktop, ctx, _) = desktop();
    with_host(&mut desktop, &ctx);
    desktop.open(Target::Networks);
    let rect = |d: &Desktop| {
        d.measured
            .iter()
            .find(|m| m.0 == Target::Networks)
            .map(|m| m.1)
            .unwrap_or(egui::Rect::ZERO)
    };
    // the contents arrive when they arrive, which depends on the machine: wait until the window
    // has kept its size for a while
    let (mut last, mut steady) = (egui::Rect::ZERO, 0);
    for _ in 0..400 {
        frames_on(&mut desktop, &ctx, size, 1);
        let now = rect(&desktop);
        steady = if now == last && now != egui::Rect::ZERO {
            steady + 1
        } else {
            0
        };
        last = now;
        if steady >= 10 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(steady >= 10, "never settled: {last:?}");
    let opened = rect(&desktop);
    // taller, then kept over many frames
    drag_corner(
        &mut desktop,
        &ctx,
        size,
        Target::Networks,
        Vec2::new(0.0, 120.0),
    );
    let taller = rect(&desktop);
    assert!(
        taller.height() > opened.height() + 60.0,
        "{opened:?} {taller:?}"
    );
    frames_on(&mut desktop, &ctx, size, 10);
    assert!(
        (rect(&desktop).height() - taller.height()).abs() < 1.5,
        "snapped back"
    );
    // and smaller again, down to what its parts that cannot shrink need
    drag_corner(
        &mut desktop,
        &ctx,
        size,
        Target::Networks,
        Vec2::new(-200.0, -400.0),
    );
    let smaller = rect(&desktop);
    assert!(
        smaller.height() < taller.height() - 80.0,
        "{taller:?} {smaller:?}"
    );
    // and down to the least it can be, no further
    drag_corner(
        &mut desktop,
        &ctx,
        size,
        Target::Networks,
        Vec2::new(-2000.0, -2000.0),
    );
    let least = crate::windows::least_size(&ctx, "Networks", crate::controls::FILL_MIN);
    let tiny = rect(&desktop);
    assert!(
        tiny.width() >= least.x - 1.0 && tiny.height() >= least.y - 1.0,
        "{tiny:?} {least:?}"
    );
    // the list shrinks to its least and the rest of the window stays
    assert!(
        tiny.height() < taller.height() - 80.0,
        "{tiny:?} {taller:?}"
    );
}

#[test]
fn a_form_window_has_a_fixed_size_and_ignores_a_drag_on_its_corner() {
    use eframe::egui::Vec2;
    let size = Vec2::new(1280.0, 720.0);
    let (mut desktop, ctx, _) = desktop();
    let form = module("web-proxy");
    desktop.open(form);
    frames_on(&mut desktop, &ctx, size, 6);
    let rect = |d: &Desktop| d.measured.iter().find(|m| m.0 == form).unwrap().1;
    let opened = rect(&desktop);
    drag_corner(&mut desktop, &ctx, size, form, Vec2::new(-100.0, -100.0));
    assert!((rect(&desktop).size() - opened.size()).abs().max_elem() < 1.5);
}

#[test]
fn the_console_opens_eighty_columns_by_twenty_four_rows_of_its_own_font() {
    let size = egui::Vec2::new(1280.0, 720.0);
    let (mut desktop, ctx, _) = desktop();
    desktop.open(Target::Console);
    frames_on(&mut desktop, &ctx, size, 6);
    let rect = desktop
        .measured
        .iter()
        .find(|m| m.0 == Target::Console)
        .unwrap()
        .1;
    let wanted = window_size(&ctx, Target::Console);
    assert!(
        (rect.size() - wanted).abs().max_elem() < 1.5,
        "{rect:?} {wanted:?}"
    );
    let font = egui::TextStyle::Monospace.resolve(&ctx.global_style());
    let cell = ctx.fonts_mut(|f| {
        f.layout_no_wrap("M".into(), font.clone(), egui::Color32::BLACK)
            .size()
            .x
    });
    assert!(
        wanted.x >= 80.0 * cell && wanted.x < 80.0 * cell + 60.0,
        "{wanted:?}"
    );
}

#[test]
fn the_welcome_topics_are_measured_once_for_a_width() {
    let size = egui::Vec2::new(1280.0, 720.0);
    let (mut desktop, ctx, _) = desktop();
    desktop.open(Target::Welcome);
    frames_on(&mut desktop, &ctx, size, 4);
    let first = desktop.welcome_heights.clone().expect("measured");
    frames_on(&mut desktop, &ctx, size, 3);
    assert_eq!(desktop.welcome_heights, Some(first));
}

#[test]
fn no_welcome_topic_label_is_wider_than_its_button_and_no_paragraph_leaves_the_panel() {
    let (mut desktop, ctx, _) = desktop();
    frames(&mut desktop, &ctx, 1);
    let font = crate::theme::font();
    for t in 0..lanlord_app::welcome::Welcome::topics() {
        let w = lanlord_app::welcome::Welcome::new(None);
        for node in w.view_of(t, &[], 0.0) {
            if let lanlord_app::view::Node::Columns(cols) = node {
                let (panel, _) = &cols[0];
                let (width, buttons) = &cols[1];
                assert_eq!(*panel, Some(lanlord_app::welcome::PANEL));
                for b in buttons {
                    if let lanlord_app::view::Node::Button(b) = b {
                        let wide = ctx.fonts_mut(|f| {
                            f.layout_no_wrap(b.label.clone(), font.clone(), egui::Color32::BLACK)
                                .size()
                                .x
                        });
                        assert!(wide + 22.0 <= width.unwrap(), "{}", b.label);
                    }
                }
            }
        }
    }
}

#[test]
fn a_growable_window_opens_as_tall_as_its_contents_up_to_the_desktop() {
    use eframe::egui::Vec2;
    for size in [Vec2::new(1280.0, 720.0), Vec2::new(800.0, 500.0)] {
        let (mut desktop, ctx, _) = desktop();
        for target in [Target::Changelog, Target::Hosts] {
            desktop.open(target);
        }
        frames_on(&mut desktop, &ctx, size, 8);
        let area = super::desktop_area(&ctx);
        let height = |d: &Desktop, t| d.measured.iter().find(|m| m.0 == t).unwrap().1.height();
        // the changelog is long: it fills the desktop, no more
        let log = height(&desktop, Target::Changelog);
        assert!(log <= area.height() + 1.0, "{size:?}: {log}");
        assert!(log >= area.height().min(400.0) - 1.0, "{size:?}: {log}");
        // the hosts list has few entries: it opens at its usual 280, not as a one line strip
        let hosts = height(&desktop, Target::Hosts);
        assert!((hosts - 280.0).abs() <= 1.0, "{size:?}: {hosts}");
    }
}

#[test]
fn a_link_opens_the_changelog_the_welcome_and_the_start_menu() {
    let (mut desktop, ctx, _fake) = desktop();
    desktop.open_spec("changelog, welcome,menu,,developer");
    frames(&mut desktop, &ctx, 2);
    for target in [Target::Changelog, Target::Welcome] {
        assert!(desktop.windows.iter().any(|w| w.target == target));
    }
    assert!(desktop.start_open && desktop.dev_open);
}

#[test]
fn the_users_progress_and_task_windows_are_resizable_and_their_lists_fill() {
    use eframe::egui::Vec2;
    let size = Vec2::new(1280.0, 720.0);
    for target in [Target::Permissions, Target::Progress, Target::TaskManager] {
        assert!(target.growable(), "{target:?}");
        let (mut desktop, ctx, _) = desktop();
        with_host(&mut desktop, &ctx);
        desktop.open(target);
        frames_on(&mut desktop, &ctx, size, 12);
        let measured = |d: &Desktop| *d.measured.iter().find(|m| m.0 == target).unwrap();
        let (_, opened, content) = measured(&desktop);
        drag_corner(&mut desktop, &ctx, size, target, Vec2::new(0.0, 100.0));
        frames_on(&mut desktop, &ctx, size, 6);
        let (_, taller, filled) = measured(&desktop);
        assert!(
            taller.height() > opened.height() + 60.0,
            "{target:?}: {opened:?} {taller:?}"
        );
        // the apply log has nothing else that grows, so it takes all of the extra room
        if target == Target::Progress {
            assert!(filled > content + 60.0, "{content} {filled}");
        }
    }
}

#[test]
fn my_server_is_a_fixed_dialog_of_one_size_on_every_tab() {
    use super::my_server::Msg;
    use eframe::egui::Vec2;
    let size = Vec2::new(1280.0, 720.0);
    assert!(Target::MyServer.fixed() && !Target::MyServer.growable());
    // only the windows that mark themselves fixed are
    assert!([
        Target::Networks,
        Target::Hosts,
        Target::Console,
        Target::Welcome
    ]
    .iter()
    .all(|t| !t.fixed()));
    let (mut desktop, ctx, _) = desktop();
    with_host(&mut desktop, &ctx);
    desktop.open(Target::MyServer);
    until(&mut desktop, &ctx, |d| {
        d.my_server.facts().belongs_to("me@gw") && !d.services.busy("me@gw")
    });
    // the checks are asked for on the Compatibility tab, which is left before they arrive: they
    // fill their page while General is open, and the window is as big on Compatibility as it is
    desktop.my_server.update(
        Msg::Tab(super::my_server::COMPATIBILITY),
        &desktop.services,
        "me@gw",
    );
    frames_on(&mut desktop, &ctx, size, 1);
    desktop.my_server.update(
        Msg::Tab(super::my_server::GENERAL),
        &desktop.services,
        "me@gw",
    );
    until(&mut desktop, &ctx, |d| d.my_server.report().is_some());
    still_on(&mut desktop, &ctx, size);
    let measured = |d: &Desktop| *d.measured.iter().find(|m| m.0 == Target::MyServer).unwrap();
    let (_, opened, _) = measured(&desktop);
    for tab in [
        super::my_server::ADVANCED,
        super::my_server::COMPATIBILITY,
        super::my_server::GENERAL,
    ] {
        desktop
            .my_server
            .update(Msg::Tab(tab), &desktop.services, "me@gw");
        frames_on(&mut desktop, &ctx, size, 8);
        let (_, rect, content) = measured(&desktop);
        assert_eq!(rect, opened, "tab {tab}");
        // what the tab holds fits without scrolling
        assert!(
            content <= rect.height() - super::windows::CHROME,
            "{tab}: {content} in {rect:?}"
        );
    }
    // dragging its corner moves it, as any press on a window does, but does not size it
    drag_corner(
        &mut desktop,
        &ctx,
        size,
        Target::MyServer,
        Vec2::new(80.0, 100.0),
    );
    assert_eq!(measured(&desktop).1.size(), opened.size());
}

#[test]
fn picking_a_check_never_changes_the_window_or_the_page_frame_of_my_server() {
    use super::my_server::{Msg, ADVANCED, COMPATIBILITY, GENERAL};
    let size = egui::Vec2::new(1280.0, 720.0);
    let (mut desktop, ctx, _) = desktop();
    with_host(&mut desktop, &ctx);
    desktop.open(Target::MyServer);
    until(&mut desktop, &ctx, |d| {
        d.my_server.facts().belongs_to("me@gw") && !d.services.busy("me@gw")
    });
    still_on(&mut desktop, &ctx, size);
    let target = Target::MyServer;
    let window = measured_of(&desktop, target);
    let frame = super::controls::tab_frame(&ctx).expect("a tab frame");
    let same = |desktop: &Desktop, what: &str| {
        let (rect, content) = measured_of(desktop, target);
        assert_eq!(rect, window.0, "{what}");
        assert_eq!(super::controls::tab_frame(&ctx), Some(frame), "{what}");
        // no scroll bar took room from the page: it spans the window's whole width
        let spans = rect.width() - super::windows::chrome().x;
        assert!((frame.width() - spans).abs() < 1.0, "{what}: {frame:?}");
        assert!(content <= rect.height() - super::windows::CHROME, "{what}");
    };
    // every tab, from the first frame, before the checks have been read and after
    for tab in [ADVANCED, COMPATIBILITY, GENERAL, ADVANCED, COMPATIBILITY] {
        desktop
            .my_server
            .update(Msg::Tab(tab), &desktop.services, "me@gw");
        frames_on(&mut desktop, &ctx, size, 1);
        still_on(&mut desktop, &ctx, size);
        same(&desktop, &format!("tab {tab}"));
    }
    until(&mut desktop, &ctx, |d| d.my_server.report().is_some());
    still_on(&mut desktop, &ctx, size);
    same(&desktop, "the checks read");
    let checks = desktop.my_server.report().map_or(0, |r| r.checks.len());
    assert!(checks > 2, "{checks}");
    for row in (0..checks).chain([0]) {
        desktop
            .my_server
            .update(Msg::Check(row), &desktop.services, "me@gw");
        still_on(&mut desktop, &ctx, size);
        same(&desktop, &format!("row {row}"));
    }
    for tab in [GENERAL, COMPATIBILITY] {
        desktop
            .my_server
            .update(Msg::Tab(tab), &desktop.services, "me@gw");
        still_on(&mut desktop, &ctx, size);
        same(&desktop, &format!("tab {tab} again"));
    }
}

fn measured_of(desktop: &Desktop, target: Target) -> (egui::Rect, f32) {
    let m = desktop.measured.iter().find(|m| m.0 == target).unwrap();
    (m.1, m.2)
}

/// draw frames until the windows stop changing: a status bar slides in or out over a few frames
/// when a page has a message and the last one did not.
fn still_on(desktop: &mut Desktop, ctx: &egui::Context, size: egui::Vec2) {
    let mut still = 0;
    let mut last = desktop.measured.clone();
    for _ in 0..300 {
        frames_on(desktop, ctx, size, 1);
        still = if desktop.measured == last {
            still + 1
        } else {
            0
        };
        last = desktop.measured.clone();
        if still >= 5 {
            return;
        }
    }
    panic!("the windows never settled");
}

#[test]
fn nt4_domain_keeps_one_rect_on_every_tab_and_its_pages_stretch() {
    let size = egui::Vec2::new(1280.0, 720.0);
    let (mut desktop, ctx, _fake) = logins();
    let target = module("nt4-domain");
    // the window as it opens, before any other tab has been looked at
    still_on(&mut desktop, &ctx, size);
    let (opened, content) = measured_of(&desktop, target);
    for tab in [Tab::Logon, Tab::Settings, Tab::Logins, Tab::Logon] {
        desktop.nt4.tab = tab;
        still_on(&mut desktop, &ctx, size);
        assert_eq!(measured_of(&desktop, target).0, opened, "{tab:?}");
    }
    // sized taller, the list takes the room, and the size is the same on the other tabs
    drag_corner(
        &mut desktop,
        &ctx,
        size,
        target,
        egui::Vec2::new(0.0, 120.0),
    );
    desktop.nt4.tab = Tab::Logins;
    still_on(&mut desktop, &ctx, size);
    let (taller, filled) = measured_of(&desktop, target);
    assert!(
        taller.height() > opened.height() + 60.0,
        "{opened:?} {taller:?}"
    );
    assert!(filled > content + 60.0, "{content} {filled}");
    for tab in [Tab::Logon, Tab::Settings] {
        desktop.nt4.tab = tab;
        still_on(&mut desktop, &ctx, size);
        assert_eq!(measured_of(&desktop, target).0, taller, "{tab:?}");
    }
}

#[test]
fn date_time_keeps_one_rect_on_every_tab_and_keeps_its_place() {
    use lanlord_app::clock_window::Msg;
    let size = egui::Vec2::new(1280.0, 720.0);
    let (mut desktop, ctx, _fake) = desktop();
    with_host(&mut desktop, &ctx);
    desktop.open(Target::DateTime);
    until(&mut desktop, &ctx, |d| d.clock.is_loaded());
    frames_on(&mut desktop, &ctx, size, 12);
    let (opened, _) = measured_of(&desktop, Target::DateTime);
    for tab in [1, 2, 0, 1] {
        desktop
            .clock
            .update(Msg::Tab(tab), &desktop.services, "me@gw");
        frames_on(&mut desktop, &ctx, size, 6);
        assert_eq!(
            measured_of(&desktop, Target::DateTime).0,
            opened,
            "tab {tab}"
        );
    }
}

#[test]
fn the_pages_are_measured_once_not_every_frame() {
    let size = egui::Vec2::new(1280.0, 720.0);
    let (mut desktop, ctx, _fake) = logins();
    frames_on(&mut desktop, &ctx, size, 12);
    let passes = crate::controls::measure_passes(&ctx);
    assert!(passes > 0);
    frames_on(&mut desktop, &ctx, size, 20);
    assert_eq!(crate::controls::measure_passes(&ctx), passes);
    // a tab switch does not measure either, once what the tab shows has been read
    for tab in [Tab::Settings, Tab::Logon, Tab::Logins] {
        desktop.nt4.tab = tab;
        until(&mut desktop, &ctx, idle("me@gw"));
        still_on(&mut desktop, &ctx, size);
    }
    let passes = crate::controls::measure_passes(&ctx);
    for tab in [Tab::Settings, Tab::Logins, Tab::Logon] {
        desktop.nt4.tab = tab;
        frames_on(&mut desktop, &ctx, size, 6);
        assert_eq!(crate::controls::measure_passes(&ctx), passes, "{tab:?}");
    }
}

/// a modal box's title bar spans the box inside its border, and its contents stay inside it.
fn assert_modal_fits(ctx: &egui::Context, id: &'static str) {
    let [bar, outer, body] = crate::dialogs::modal_rects(ctx, id).expect("the dialog was drawn");
    assert_eq!(bar.left(), outer.left() + super::windows::FRAME, "{id}");
    assert_eq!(bar.right(), outer.right() - super::windows::FRAME, "{id}");
    assert!(body.right() <= bar.right(), "{id}: {body:?} in {bar:?}");
    // every text box is inside the box's border and as wide as it asked to be: one squeezed into
    // what room is left is cut short, and its text with it
    let inner = outer.shrink(super::windows::FRAME);
    for (rect, wanted) in crate::dialogs::boxes(ctx) {
        assert!(
            inner.contains_rect(rect),
            "{id}: a text box {rect:?} pokes out of {inner:?}"
        );
        assert!(
            rect.width() >= wanted - 0.5,
            "{id}: a text box {rect:?} was squeezed from {wanted}"
        );
    }
}

#[test]
fn the_new_network_dialog_has_a_full_title_bar_and_every_field_inside_it() {
    let (mut desktop, ctx, _) = desktop();
    with_host(&mut desktop, &ctx);
    desktop.open(Target::Networks);
    until(&mut desktop, &ctx, idle("me@gw"));
    assert!(press(&mut desktop, &ctx, "New network..."));
    frames(&mut desktop, &ctx, 4);
    assert_modal_fits(&ctx, "network-form");
}
