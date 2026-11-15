//! headless tests for the management windows, with a fake gateway.

use std::net::Ipv4Addr;
use std::sync::Arc;

use eframe::egui;
use lanlord_core::leases::Lease;
use lanlord_core::network::Internal;

use super::harness::*;
use super::*;
use crate::gateway::Fake;
use crate::nt4_window::Tab;

const NT4: &str =
    "nt4-domain:\n  mode: domain\n  name: RETRONET\n  users:\n    - name: james\n      admin: true\n";
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
    frames(desktop, ctx, 2);
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
    frames(&mut desktop, &ctx, 2);
    (desktop, ctx, fake)
}

/// fill the open form's fields.
fn fill(form: Option<&mut crate::form::Form>, values: &[&str]) {
    let form = form.expect("a form is open");
    for (field, value) in form.fields.iter_mut().zip(values) {
        field.value = (*value).into();
    }
}

#[test]
fn logins_are_listed_added_with_a_generated_password_and_kept_out_of_the_plain_text() {
    let (mut desktop, ctx, fake) = logins();
    assert_eq!(desktop.nt4.users.names(), ["james"]);
    assert!(press(&mut desktop, &ctx, "Add login..."));
    fill(desktop.nt4.users.form_mut(), &["olive"]);
    assert!(press(&mut desktop, &ctx, "Generate"));
    let generated = desktop.nt4.users.form_mut().unwrap().raw(1).to_string();
    assert_eq!(generated.len(), 12);
    desktop.nt4.users.form_mut().unwrap().checks[0].1 = true;
    assert!(press(&mut desktop, &ctx, "OK"));
    settled(&mut desktop, &ctx);
    assert_eq!(desktop.nt4.users.names(), ["james", "olive"]);
    let plain = stored(&fake);
    assert!(
        plain.contains("olive") && !plain.contains(&generated),
        "{plain}"
    );
    assert_eq!(
        fake.secrets.lock().unwrap().clone(),
        [("nt4-domain.users[olive].password".to_string(), generated)]
    );
    assert_eq!(
        fake.calls(),
        [
            "load me@gw",
            "users me@gw 2",
            "apply me@gw [nt4-domain]",
            "load me@gw"
        ]
    );
}

#[test]
fn a_bad_add_keeps_the_form_open_and_cancel_changes_nothing() {
    let (mut desktop, ctx, fake) = logins();
    assert!(press(&mut desktop, &ctx, "Add login..."));
    for (name, password) in [("Bad Name", "x"), ("james", "x"), ("olive", "")] {
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
    desktop.nt4.users.select("james");
    frames(&mut desktop, &ctx, 2);

    assert!(press(&mut desktop, &ctx, "Password..."));
    fill(desktop.nt4.users.form_mut(), &["N3WPASS"]);
    assert!(press(&mut desktop, &ctx, "OK"));
    settled(&mut desktop, &ctx);
    assert_eq!(
        fake.secrets.lock().unwrap()[0],
        (
            "nt4-domain.users[james].password".to_string(),
            "N3WPASS".to_string()
        )
    );

    assert!(press(&mut desktop, &ctx, "Remove domain admin"));
    settled(&mut desktop, &ctx);
    assert!(!stored(&fake).contains("admin: true"));
    assert!(enabled(&ctx, "Make domain admin").is_some());

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
    until(&mut desktop, &ctx, |d| {
        d.nt4.users.note().is_some_and(|n| n.0)
    });
    assert!(desktop
        .nt4
        .users
        .note()
        .unwrap()
        .1
        .contains("no nt4-domain section"));
    *fake.stored.lock().unwrap() = NT4.into();
    assert!(press(&mut desktop, &ctx, "Reload logins"));
    until(&mut desktop, &ctx, |d| d.nt4.users.belongs_to("me@gw"));
    assert_eq!(desktop.nt4.users.names(), ["james"]);
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
    assert_eq!(enabled(&ctx, "Make static"), Some(false), "no row picked");
    desktop.dns.select_current(0);
    assert!(press(&mut desktop, &ctx, "Make static"));
    let form = desktop.dns.form_mut().unwrap();
    assert_eq!(
        (form.value(0), form.value(1)),
        ("win98", "aa:bb:cc:dd:ee:00")
    );
    form.fields[2].value = "172.16.120.101".into();
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
        .push(lanlord_core::settings::Record {
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
    assert_eq!(enabled(&ctx, "Add lease..."), Some(false));
    assert_eq!(
        enabled(&ctx, "Save DNS"),
        Some(true),
        "unchecked addresses still save"
    );
    desktop.dns.draft.dhcp = Some(lanlord_core::settings::Dhcp::default());
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
        plain.contains("james") && !plain.contains("Adm1nPass"),
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
    frames(&mut desktop, &ctx, 3);
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
    frames(&mut desktop, &ctx, 3);
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
