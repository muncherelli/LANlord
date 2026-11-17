//! headless tests for the components wizard and the desktop icons that follow it.

use std::sync::Arc;

use eframe::egui;

use super::harness::*;
use super::*;
use lanlord_app::components::Msg;
use lanlord_app::gateway::Fake;

const SETTINGS: &str = "networks:\n  - {id: retronet, name: Retro LAN, subnet: 192.0.2.0/24, gateway: 192.0.2.1}\ncomponents:\n  docker: {}\n  dns:\n    attach: [{network: retronet}]\n";

/// a desktop that reads its components from the server, signed in to one whose settings switch
/// on docker and dns.
fn reading() -> (Desktop, egui::Context, Arc<Fake>) {
    let (mut desktop, ctx, fake) = desktop();
    desktop.pinned = None;
    *fake.stored.lock().unwrap() = SETTINGS.into();
    with_host(&mut desktop, &ctx);
    until(&mut desktop, &ctx, |d| d.components.current().is_some());
    frames(&mut desktop, &ctx, 2);
    (desktop, ctx, fake)
}

fn ids(desktop: &Desktop) -> Vec<&'static str> {
    desktop.icons.iter().map(|t| t.id()).collect()
}

fn labels(desktop: &Desktop) -> Vec<String> {
    desktop
        .menu_rows()
        .iter()
        .filter_map(|row| match row {
            menu::Row::Item(item) => Some(item.label().to_string()),
            menu::Row::Separator => None,
        })
        .collect()
}

#[test]
fn only_the_switched_on_components_get_icons_and_menu_items() {
    let (desktop, _ctx, _fake) = reading();
    assert_eq!(
        ids(&desktop),
        [
            "my-server",
            "networks",
            "gateway",
            "docker",
            "dns",
            "settings",
            "console",
            "hosts"
        ]
    );
    let labels = labels(&desktop);
    assert!(labels.iter().any(|l| l == "DNS"));
    assert!(!labels.iter().any(|l| l == "NT4 Domain"));
    assert!(labels.iter().any(|l| l == "Add/Remove Components..."));
}

#[test]
fn without_a_server_there_are_only_the_built_in_icons() {
    let (mut desktop, ctx, fake) = desktop();
    desktop.pinned = None;
    frames(&mut desktop, &ctx, 2);
    assert_eq!(
        ids(&desktop),
        ["my-server", "networks", "settings", "console", "hosts"]
    );
    assert!(fake.calls().is_empty());
}

#[test]
fn the_wizard_adds_a_component_and_its_icon_appears() {
    let (mut desktop, ctx, fake) = reading();
    desktop.open_spec("components");
    frames(&mut desktop, &ctx, 2);
    assert!(desktop
        .windows
        .iter()
        .any(|w| w.target == Target::Components));
    let row = lanlord_model_row("web-proxy");
    desktop
        .components
        .update(Msg::Toggle(row), &desktop.services, "me@gw");
    assert!(press(&mut desktop, &ctx, "Next >"), "to the networks page");
    for _ in 0..4 {
        frames(&mut desktop, &ctx, 1);
        eprintln!("F {:?}", desktop.measured);
    }
    eprintln!(
        "AFTER1 {:?} {:?}",
        desktop.measured,
        crate::theme::probe::find(&ctx, "Next >")
    );
    assert!(press(&mut desktop, &ctx, "Next >"), "to the summary");
    assert!(press(&mut desktop, &ctx, "Finish"));
    until(&mut desktop, &ctx, |d| {
        !d.windows.iter().any(|w| w.target == Target::Components)
    });
    frames(&mut desktop, &ctx, 2);
    assert!(ids(&desktop).contains(&"web-proxy"), "{:?}", ids(&desktop));
    assert!(fake.stored.lock().unwrap().contains("web-proxy"));
}

fn lanlord_model_row(id: &str) -> usize {
    MODULES.iter().position(|m| m.id == id).unwrap()
}
