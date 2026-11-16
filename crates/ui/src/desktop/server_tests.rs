//! headless tests for the My Server and Networks windows, with a fake gateway.

use std::sync::Arc;

use eframe::egui;

use super::harness::*;
use super::*;
use crate::gateway::Fake;

fn server() -> (Desktop, egui::Context, Arc<Fake>) {
    let (mut desktop, ctx, fake) = desktop();
    desktop.open(Target::MyServer);
    with_host(&mut desktop, &ctx);
    until(&mut desktop, &ctx, |d| {
        d.my_server.facts.belongs_to("me@gw")
    });
    frames(&mut desktop, &ctx, 2);
    (desktop, ctx, fake)
}

/// open the box that asks before a purge, and wait for its preview.
fn ask_to_purge(desktop: &mut Desktop, ctx: &egui::Context) {
    desktop.my_server.tab = 1;
    frames(desktop, ctx, 2);
    assert!(press(desktop, ctx, "Remove LANlord from this server..."));
    until(desktop, ctx, |d| {
        d.my_server
            .purge
            .as_ref()
            .is_some_and(|p| p.preview.data.is_some())
    });
    frames(desktop, ctx, 2);
}

#[test]
fn the_icons_come_first_and_the_menu_lists_them() {
    let (desktop, _ctx, _fake) = desktop();
    assert_eq!(desktop.icons[0], Target::MyServer);
    assert_eq!(desktop.icons[1], Target::Networks);
    assert_eq!(
        (Target::MyServer.id(), Target::Networks.title()),
        ("my-server", "Networks")
    );
}

#[test]
fn general_reads_what_the_server_is() {
    let (desktop, _ctx, fake) = server();
    assert_eq!(fake.calls(), ["facts me@gw"]);
    let facts = desktop.my_server.facts.data.clone().unwrap();
    assert_eq!(facts.hostname, "demo");
    assert!(facts.is_root && facts.lanlord_installed);
}

#[test]
fn nothing_is_purged_before_the_host_name_is_typed() {
    let (mut desktop, ctx, fake) = server();
    ask_to_purge(&mut desktop, &ctx);
    assert!(desktop.my_server.purging());
    assert_eq!(enabled(&ctx, "Remove"), Some(false), "needs the name");
    desktop.my_server.purge.as_mut().unwrap().typed = "me@gx".into();
    frames(&mut desktop, &ctx, 2);
    assert_eq!(enabled(&ctx, "Remove"), Some(false), "a wrong name");
    // only the preview has run so far
    assert_eq!(
        fake.calls(),
        ["facts me@gw", "purge-preview me@gw admins=false"]
    );
    desktop.my_server.purge.as_mut().unwrap().typed = "me@gw".into();
    frames(&mut desktop, &ctx, 2);
    assert_eq!(enabled(&ctx, "Remove"), Some(true));
    assert!(press(&mut desktop, &ctx, "Remove"));
    until(&mut desktop, &ctx, |d| !d.my_server.purging());
    assert!(fake
        .calls()
        .contains(&"purge me@gw admins=false".to_string()));
    // the host is forgotten once it is purged
    assert!(desktop.hosts.hosts().is_empty());
}

#[test]
fn cancel_closes_the_box_without_removing_anything() {
    let (mut desktop, ctx, fake) = server();
    ask_to_purge(&mut desktop, &ctx);
    assert!(press(&mut desktop, &ctx, "Cancel"));
    assert!(!desktop.my_server.purging());
    assert!(!fake.calls().iter().any(|c| c.starts_with("purge me@gw")));
}

#[test]
fn the_admin_accounts_tick_needs_a_root_login() {
    let (mut desktop, ctx, fake) = desktop();
    let mut facts = lanlord_core::server::demo();
    facts.is_root = false;
    facts.user = "olive".into();
    *fake.facts.lock().unwrap() = Some(facts);
    desktop.open(Target::MyServer);
    with_host(&mut desktop, &ctx);
    until(&mut desktop, &ctx, |d| {
        d.my_server.facts.belongs_to("me@gw")
    });
    ask_to_purge(&mut desktop, &ctx);
    assert_eq!(
        enabled(&ctx, "Also remove the administrator accounts"),
        Some(false)
    );
}

#[test]
fn the_admin_accounts_tick_asks_for_a_new_preview() {
    let (mut desktop, ctx, fake) = server();
    ask_to_purge(&mut desktop, &ctx);
    assert!(press(
        &mut desktop,
        &ctx,
        "Also remove the administrator accounts"
    ));
    until(&mut desktop, &ctx, |d| {
        d.my_server
            .purge
            .as_ref()
            .is_some_and(|p| p.preview.data.is_some())
    });
    assert!(fake
        .calls()
        .contains(&"purge-preview me@gw admins=true".to_string()));
}

#[test]
fn a_server_without_lanlord_cannot_be_purged() {
    let (mut desktop, ctx, fake) = desktop();
    let mut facts = lanlord_core::server::demo();
    facts.lanlord_installed = false;
    *fake.facts.lock().unwrap() = Some(facts);
    desktop.open(Target::MyServer);
    with_host(&mut desktop, &ctx);
    until(&mut desktop, &ctx, |d| {
        d.my_server.facts.belongs_to("me@gw")
    });
    desktop.my_server.tab = 1;
    frames(&mut desktop, &ctx, 3);
    assert_eq!(
        enabled(&ctx, "Remove LANlord from this server..."),
        Some(false)
    );
    assert!(!press(
        &mut desktop,
        &ctx,
        "Remove LANlord from this server..."
    ));
}

const NETWORKS: &str = "docker:\n  internal_address: 172.16.120.1/22\ndns:\n  zone: retronet\n  dhcp:\n    range: [172.16.120.100, 172.16.120.200]\n    lease: 7d\n";

#[test]
fn networks_lists_the_external_and_the_lanlord_network() {
    let (mut desktop, ctx, fake) = desktop();
    *fake.stored.lock().unwrap() = NETWORKS.into();
    desktop.open(Target::Networks);
    with_host(&mut desktop, &ctx);
    until(&mut desktop, &ctx, |d| d.networks.rows.belongs_to("me@gw"));
    frames(&mut desktop, &ctx, 2);
    let rows = desktop.networks.rows.data.clone().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].interface, "eth0");
    assert_eq!(rows[1].name, "retronet");
    // the properties box is read-only and closes with OK
    assert_eq!(enabled(&ctx, "Properties..."), Some(true));
    assert!(press(&mut desktop, &ctx, "Properties..."));
    assert!(desktop.networks.details.is_some());
    assert!(press(&mut desktop, &ctx, "OK"));
    assert!(desktop.networks.details.is_none());
}

#[test]
fn networks_opens_dns_and_dhcp_for_a_lanlord_network_only() {
    let (mut desktop, ctx, fake) = desktop();
    *fake.stored.lock().unwrap() = NETWORKS.into();
    desktop.open(Target::Networks);
    with_host(&mut desktop, &ctx);
    until(&mut desktop, &ctx, |d| d.networks.rows.belongs_to("me@gw"));
    frames(&mut desktop, &ctx, 2);
    // the first row is the external network, which has no dns window
    assert_eq!(enabled(&ctx, "DNS & DHCP..."), Some(false));
    desktop.networks.select(1);
    frames(&mut desktop, &ctx, 3);
    assert_eq!(enabled(&ctx, "DNS & DHCP..."), Some(true));
    assert!(press(&mut desktop, &ctx, "DNS & DHCP..."));
    assert!(desktop.windows.iter().any(|w| w.target.id() == "dns"));
}

#[test]
fn networks_has_a_tab_per_network_with_its_connection_status() {
    let (mut desktop, ctx, fake) = desktop();
    *fake.stored.lock().unwrap() = NETWORKS.into();
    desktop.open(Target::Networks);
    with_host(&mut desktop, &ctx);
    until(&mut desktop, &ctx, |d| {
        d.networks.rows.belongs_to("me@gw") && d.networks.status.data.is_some()
    });
    frames(&mut desktop, &ctx, 2);
    let status = desktop.networks.status.data.clone().unwrap();
    let external = status.iter().find(|s| s.default_route).unwrap();
    assert_eq!(external.name, "eth0");
    assert!(fake
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c == "network-status me@gw"));
    // the other network's tab is a click away
    assert_eq!(desktop.networks.selected, 0);
    assert!(press(&mut desktop, &ctx, "retronet"));
    frames(&mut desktop, &ctx, 2);
    assert_eq!(desktop.networks.selected, 1);
    assert_eq!(enabled(&ctx, "DNS & DHCP..."), Some(true));
    // details opens the connection's property list and closes again
    assert!(press(&mut desktop, &ctx, "Details..."));
    assert_eq!(desktop.networks.connection.as_ref().unwrap().name, "eth1");
    assert!(press(&mut desktop, &ctx, "Close"));
    assert!(desktop.networks.connection.is_none());
}

#[test]
fn the_server_windows_say_so_without_a_server() {
    let (mut desktop, ctx, _fake) = desktop();
    desktop.open(Target::MyServer);
    desktop.open(Target::Networks);
    frames(&mut desktop, &ctx, 3);
    assert!(desktop.my_server.facts.data.is_none());
    assert!(desktop.networks.rows.data.is_none());
}
