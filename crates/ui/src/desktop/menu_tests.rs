//! headless tests for the LANlord menu.

use lanlord_app::log;
use lanlord_model::power::Power;

use super::harness::*;
use super::menu::Action;
use super::*;

fn open_menu(desktop: &mut Desktop, ctx: &egui::Context) {
    desktop.start_open = true;
    frames(desktop, ctx, 3);
}

/// click the menu item for `action`, drawing the menu first.
fn choose(desktop: &mut Desktop, ctx: &egui::Context, action: &Action) {
    frames(desktop, ctx, 2);
    let at = desktop
        .menu_item_at(screen(), action)
        .unwrap_or_else(|| panic!("no menu item for {action:?}"));
    click_at(desktop, ctx, at);
}

#[test]
fn the_menu_draws_with_its_sections() {
    let (mut desktop, ctx, _) = desktop();
    open_menu(&mut desktop, &ctx);
    for action in [
        Action::Open(Target::Settings),
        Action::Power(Power::Reboot),
        Action::Power(Power::PowerOff),
        Action::About,
        Action::Quit,
    ] {
        assert!(
            desktop.menu_item_at(screen(), &action).is_some(),
            "{action:?}"
        );
    }
}

#[test]
fn reboot_needs_the_confirmation_and_cancel_does_nothing() {
    let (mut desktop, ctx, fake) = desktop();
    with_host(&mut desktop, &ctx);
    open_menu(&mut desktop, &ctx);
    choose(&mut desktop, &ctx, &Action::Power(Power::Reboot));
    assert!(desktop.confirm.is_some());
    assert!(fake.calls().is_empty(), "nothing runs before the click");
    assert!(press(&mut desktop, &ctx, "Cancel"));
    frames(&mut desktop, &ctx, 2);
    assert!(desktop.confirm.is_none() && fake.calls().is_empty());

    let seen = log::subscribe();
    open_menu(&mut desktop, &ctx);
    choose(&mut desktop, &ctx, &Action::Power(Power::Reboot));
    *fake.password_prompt.lock().unwrap() = Some("sudo password for me".into());
    assert!(press(&mut desktop, &ctx, "OK"));
    until(&mut desktop, &ctx, |d| d.password.is_some());
    type_text(&mut desktop, &ctx, "pw");
    until(&mut desktop, &ctx, idle("me@gw"));
    assert_eq!(fake.calls(), ["power me@gw rebooting"]);
    let lines: Vec<String> = seen.try_iter().map(|l| l.text).collect();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("rebooting") && l.contains("as expected")),
        "{lines:?}"
    );
    assert!(
        lines.iter().all(|l| l != "the connection dropped"),
        "not a failure"
    );
}

#[test]
fn shut_down_asks_with_the_host_named_and_runs_after_ok() {
    let (mut desktop, ctx, fake) = desktop();
    with_host(&mut desktop, &ctx);
    open_menu(&mut desktop, &ctx);
    choose(&mut desktop, &ctx, &Action::Power(Power::PowerOff));
    assert!(desktop.confirm.is_some());
    assert!(press(&mut desktop, &ctx, "OK"));
    until(&mut desktop, &ctx, idle("me@gw"));
    assert_eq!(fake.calls(), ["power me@gw shutting down"]);
}

#[test]
fn power_items_do_nothing_without_a_server() {
    let (mut desktop, ctx, fake) = desktop();
    open_menu(&mut desktop, &ctx);
    choose(&mut desktop, &ctx, &Action::Power(Power::Reboot));
    assert!(desktop.confirm.is_none() && fake.calls().is_empty());
    desktop.do_action(&ctx, Action::Power(Power::Reboot));
    assert!(desktop.confirm.is_none());
}

#[test]
fn about_shows_the_version_and_quit_closes() {
    let (mut desktop, ctx, _) = desktop();
    open_menu(&mut desktop, &ctx);
    choose(&mut desktop, &ctx, &Action::About);
    assert!(desktop.confirm.is_some());
    assert!(press(&mut desktop, &ctx, "OK"));
    frames(&mut desktop, &ctx, 2);
    assert!(desktop.confirm.is_none());
    desktop.do_action(&ctx, Action::Quit);
}

#[test]
fn the_local_mode_hides_the_logon_items_but_keeps_power() {
    let (mut desktop, ctx, fake) = desktop();
    *fake.local.lock().unwrap() = true;
    open_menu(&mut desktop, &ctx);
    assert!(desktop.menu_item_at(screen(), &Action::LogOff).is_none());
    assert!(desktop
        .menu_item_at(screen(), &Action::SwitchServer)
        .is_none());
    assert!(desktop
        .menu_item_at(screen(), &Action::Power(Power::Reboot))
        .is_some());
    assert_eq!(desktop.host().as_deref(), Some("local"));
    desktop.start_open = false;
    desktop.open(Target::Hosts);
    frames(&mut desktop, &ctx, 3);
    assert!(theme::probe::find(&ctx, "Add...").is_none(), "no host list");
    desktop.start_open = true;
    choose(&mut desktop, &ctx, &Action::Power(Power::Reboot));
    assert!(press(&mut desktop, &ctx, "OK"));
    until(&mut desktop, &ctx, idle("local"));
    assert_eq!(fake.calls(), ["power local rebooting"]);
}

#[test]
fn the_client_mode_hosts_window_has_its_list() {
    let (mut desktop, ctx, _) = desktop();
    desktop.open(Target::Hosts);
    frames(&mut desktop, &ctx, 3);
    assert!(theme::probe::find(&ctx, "Add...").is_some());
}
