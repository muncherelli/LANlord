//! headless tests for the components wizard and the desktop icons that follow it.

use std::sync::Arc;

use eframe::egui;

use super::harness::{desktop, enabled, frames, press, until, with_host};
use super::{menu, Desktop, Target, MODULES};
use lanlord_app::components::{Msg, Step};
use lanlord_app::gateway::Fake;
use lanlord_app::networks_window::{Field, Msg as Net};

const SETTINGS: &str = "networks:\n  - {id: retronet, name: Retro LAN, subnet: 192.0.2.0/24, gateway: 192.0.2.1}\ncomponents:\n  docker: {}\n  dns:\n    attach: [{network: retronet}]\n";

/// a desktop that reads its components from the server, signed in to one whose settings switch
/// on docker and dns.
fn reading() -> (Desktop, egui::Context, Arc<Fake>) {
    reading_with(SETTINGS)
}

fn reading_with(settings: &str) -> (Desktop, egui::Context, Arc<Fake>) {
    let (mut desktop, ctx, fake) = desktop();
    desktop.pinned = None;
    *fake.stored.lock().unwrap() = settings.into();
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
            "dhcp",
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
    frames(&mut desktop, &ctx, 4);
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

/// add the network `name` through the Networks window and wait until it is saved.
fn add_network(desktop: &mut Desktop, ctx: &egui::Context, name: &str) {
    desktop.open_spec("networks");
    until(desktop, ctx, |d| d.networks.rows().data.is_some());
    desktop
        .networks
        .update(Net::New, &desktop.services, "me@gw", 0.0);
    let name = Net::Edit(Field::Name, name.into());
    desktop
        .networks
        .update(name, &desktop.services, "me@gw", 0.0);
    desktop
        .networks
        .update(Net::Save, &desktop.services, "me@gw", 0.0);
    until(desktop, ctx, |d| {
        !d.services.busy("me@gw")
            && d.networks
                .rows()
                .data
                .as_ref()
                .is_some_and(|n| n.rows.len() > 1)
    });
    frames(desktop, ctx, 2);
}

fn lans(desktop: &Desktop) -> Vec<&str> {
    desktop
        .components
        .networks()
        .iter()
        .filter(|n| !n.external)
        .map(|n| n.name.as_str())
        .collect()
}

#[test]
fn a_network_added_in_the_networks_window_is_listed_by_the_wizard() {
    let (mut desktop, ctx, _fake) = reading_with("docker: {}\n");
    add_network(&mut desktop, &ctx, "retronet");
    desktop.open_spec("components");
    until(&mut desktop, &ctx, |d| !lans(d).is_empty());
    assert_eq!(lans(&desktop), ["retronet"]);
    let dns = lanlord_model_row("dns");
    desktop
        .components
        .update(Msg::Toggle(dns), &desktop.services, "me@gw");
    assert!(press(&mut desktop, &ctx, "Next >"), "to the networks page");
    frames(&mut desktop, &ctx, 4);
    assert_eq!(desktop.components.step, Step::Attach);
    assert_eq!(enabled(&ctx, "Next >"), Some(true));
}

#[test]
fn a_network_added_while_the_wizard_is_open_shows_up_in_it() {
    let (mut desktop, ctx, _fake) = reading_with("docker: {}\n");
    desktop.open_spec("components");
    frames(&mut desktop, &ctx, 4);
    assert!(lans(&desktop).is_empty());
    let dns = lanlord_model_row("dns");
    desktop
        .components
        .update(Msg::Toggle(dns), &desktop.services, "me@gw");
    assert!(press(&mut desktop, &ctx, "Next >"), "to the networks page");
    frames(&mut desktop, &ctx, 4);
    assert_eq!(enabled(&ctx, "Next >"), Some(false), "dns needs a network");
    add_network(&mut desktop, &ctx, "retronet");
    until(&mut desktop, &ctx, |d| !lans(d).is_empty());
    assert_eq!(lans(&desktop), ["retronet"]);
    desktop
        .components
        .update(Msg::ToggleNet(0, 1), &desktop.services, "me@gw");
    frames(&mut desktop, &ctx, 4);
    assert_eq!(enabled(&ctx, "Next >"), Some(true));
}

/// the wizard's window and where its buttons are, once nothing is moving.
fn wizard_place(desktop: &mut Desktop, ctx: &egui::Context) -> (egui::Rect, egui::Rect) {
    let mut still = 0;
    let mut last = desktop.measured.clone();
    for _ in 0..300 {
        frames(desktop, ctx, 1);
        std::thread::sleep(std::time::Duration::from_millis(2));
        still = if desktop.measured == last {
            still + 1
        } else {
            0
        };
        last = desktop.measured.clone();
        if still >= 5 {
            break;
        }
    }
    let rect = desktop
        .measured
        .iter()
        .find(|m| m.0 == Target::Components)
        .expect("the wizard is drawn")
        .1;
    let cancel = crate::theme::probe::find(ctx, "Cancel")
        .expect("a Cancel button")
        .0;
    (rect, cancel)
}

#[test]
fn the_wizard_keeps_one_size_and_its_buttons_in_place_on_every_page() {
    let (mut desktop, ctx, _fake) = reading();
    desktop.open_spec("components");
    let first = wizard_place(&mut desktop, &ctx);
    // ticking any component on and off, and picking any row, changes nothing
    let rows = lanlord_model::components::catalogue().len();
    for row in (0..rows).chain([0]) {
        for _ in 0..2 {
            desktop
                .components
                .update(Msg::Toggle(row), &desktop.services, "me@gw");
            assert_eq!(wizard_place(&mut desktop, &ctx), first, "row {row}");
        }
    }
    let row = lanlord_model_row("web-proxy");
    desktop
        .components
        .update(Msg::Toggle(row), &desktop.services, "me@gw");
    assert_eq!(wizard_place(&mut desktop, &ctx), first, "web proxy ticked");
    let passes = crate::controls::measure_passes(&ctx);
    assert!(passes > 0);
    // idle frames measure nothing
    frames(&mut desktop, &ctx, 10);
    assert_eq!(crate::controls::measure_passes(&ctx), passes, "idle frames");
    assert!(press(&mut desktop, &ctx, "Next >"), "to the networks page");
    let on_networks = wizard_place(&mut desktop, &ctx);
    assert_eq!(desktop.components.step, Step::Attach);
    assert_eq!(on_networks, first, "the networks page");
    assert!(press(&mut desktop, &ctx, "< Back"), "back to the list");
    let back = wizard_place(&mut desktop, &ctx);
    assert_eq!(desktop.components.step, Step::Choose);
    assert_eq!(back, first, "back on the list");
    // going to and fro does not measure the pages again
    assert_eq!(
        crate::controls::measure_passes(&ctx),
        passes,
        "next and back"
    );
}
