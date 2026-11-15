//! headless tests for the logon box on the desktop: it comes first, signing in reveals the
//! desktop for that server, and logging off brings it back.

use std::sync::Arc;

use lanlord_core::signin::SigninError;

use super::harness::*;
use super::menu::Action;
use super::*;
use crate::login::fake::{id, FakeAuth};
use crate::login::Stage;

/// a desktop that starts at the logon box, with a fake server behind it.
fn logon() -> (Desktop, egui::Context, Arc<FakeAuth>) {
    let (mut desktop, ctx, _) = desktop();
    let auth = Arc::new(FakeAuth::default());
    desktop.require_login(auth.clone());
    frames(&mut desktop, &ctx, 3);
    (desktop, ctx, auth)
}

fn sign_in(desktop: &mut Desktop, ctx: &egui::Context, user: &str, host: &str) {
    let login = desktop.login.as_mut().expect("the logon box is open");
    login.user = user.into();
    login.host = host.into();
    assert!(press(desktop, ctx, "OK"));
}

fn logged_on(desktop: &Desktop) -> bool {
    desktop.login.is_none() && desktop.signed_in.is_some()
}

#[test]
fn the_logon_box_is_all_there_is_until_someone_signs_in() {
    let (mut desktop, ctx, _) = logon();
    assert!(desktop.login.is_some() && desktop.signed_in.is_none());
    for button in ["OK", "Cancel", "Shut Down"] {
        assert!(enabled(&ctx, button).is_some(), "{button}");
    }
    desktop.open(Target::Console);
    frames(&mut desktop, &ctx, 2);
    assert!(
        desktop.login.is_some(),
        "windows cannot be opened from the box"
    );
}

#[test]
fn signing_in_shows_the_desktop_for_that_server() {
    let (mut desktop, ctx, auth) = logon();
    auth.push(Ok(id("alice", 1000)));
    sign_in(&mut desktop, &ctx, "alice", "gw.lan");
    until(&mut desktop, &ctx, logged_on);
    assert_eq!(desktop.host().as_deref(), Some("alice@gw.lan"));
    assert_eq!(auth.calls(), ["attempt alice@gw.lan - false"]);
    assert!(desktop.root_notice.is_none(), "alice is not root");
    desktop.open(Target::Console);
    frames(&mut desktop, &ctx, 2);
    assert_eq!(desktop.windows.len(), 1);
}

#[test]
fn the_last_server_is_filled_in_next_time() {
    let (mut desktop, ctx, _) = desktop();
    desktop.hosts.add("bob@one.lan").unwrap();
    desktop.require_login(Arc::new(FakeAuth::default()));
    frames(&mut desktop, &ctx, 2);
    let login = desktop.login.as_ref().unwrap();
    assert_eq!(
        (login.user.as_str(), login.host.as_str()),
        ("bob", "one.lan")
    );
}

#[test]
fn a_password_server_goes_through_the_password_step() {
    let (mut desktop, ctx, auth) = logon();
    auth.push(Err(SigninError::NeedsPassword));
    auth.push(Ok(id("alice", 1000)));
    sign_in(&mut desktop, &ctx, "alice", "gw");
    until(&mut desktop, &ctx, |d| {
        d.login
            .as_ref()
            .is_some_and(|l| l.stage() == Stage::Password)
    });
    desktop.login.as_mut().unwrap().set_password("hunter2");
    assert!(press(&mut desktop, &ctx, "OK"));
    until(&mut desktop, &ctx, logged_on);
    assert_eq!(
        auth.calls(),
        ["attempt alice@gw - false", "attempt alice@gw hunter2 false"]
    );
}

#[test]
fn shut_down_closes_the_window() {
    let (mut desktop, ctx, _) = logon();
    assert!(press(&mut desktop, &ctx, "Shut Down"));
    assert!(
        desktop.login.is_some(),
        "the close request ends the app, not the box"
    );
}

#[test]
fn root_is_told_to_make_an_administrator_account() {
    let (mut desktop, ctx, auth) = logon();
    auth.push(Ok(id("root", 0)));
    sign_in(&mut desktop, &ctx, "root", "gw");
    until(&mut desktop, &ctx, logged_on);
    frames(&mut desktop, &ctx, 3);
    assert!(desktop.root_notice.is_some());
    assert!(enabled(&ctx, "Create account...").is_some());
    // the first button explains that it is not wired yet
    assert!(press(&mut desktop, &ctx, "Create account..."));
    frames(&mut desktop, &ctx, 3);
    assert!(desktop
        .root_notice
        .as_ref()
        .is_some_and(|(stage, _)| *stage == RootNotice::NotWired));
    assert!(press(&mut desktop, &ctx, "OK"));
    frames(&mut desktop, &ctx, 2);
    assert!(desktop.root_notice.is_none());
}

#[test]
fn not_now_dismisses_the_root_notice() {
    let (mut desktop, ctx, auth) = logon();
    auth.push(Ok(id("root", 0)));
    sign_in(&mut desktop, &ctx, "root", "gw");
    until(&mut desktop, &ctx, logged_on);
    frames(&mut desktop, &ctx, 3);
    assert!(press(&mut desktop, &ctx, "Not now"));
    frames(&mut desktop, &ctx, 2);
    assert!(desktop.root_notice.is_none());
}

#[test]
fn the_menu_offers_log_off_and_switch_server_once_signed_in() {
    let (mut desktop, ctx, auth) = logon();
    auth.push(Ok(id("alice", 1000)));
    sign_in(&mut desktop, &ctx, "alice", "gw");
    until(&mut desktop, &ctx, logged_on);
    desktop.start_open = true;
    frames(&mut desktop, &ctx, 3);
    assert!(desktop.menu_item_at(screen(), &Action::LogOff).is_some());
    assert!(desktop
        .menu_item_at(screen(), &Action::SwitchServer)
        .is_some());
    assert!(desktop
        .menu_rows()
        .iter()
        .any(|r| matches!(r, menu::Row::Item(i) if i.label() == "Log off alice...")));
}

#[test]
fn logging_off_closes_the_connection_and_returns_to_the_box() {
    let (mut desktop, ctx, auth) = logon();
    auth.push(Ok(id("alice", 1000)));
    sign_in(&mut desktop, &ctx, "alice", "gw");
    until(&mut desktop, &ctx, logged_on);
    desktop.open(Target::Console);
    frames(&mut desktop, &ctx, 2);
    desktop.do_action(&ctx, Action::LogOff);
    assert!(desktop.login.is_some() && desktop.signed_in.is_none());
    assert!(desktop.windows.is_empty() && !desktop.start_open);
    let login = desktop.login.as_ref().unwrap();
    assert_eq!(
        (login.user.as_str(), login.host.as_str()),
        ("alice", "gw"),
        "the box remembers who was signed in"
    );
    // the connection is closed from a thread
    let start = std::time::Instant::now();
    while !auth.calls().iter().any(|c| c == "sign_out alice@gw") {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "no sign out"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn switching_server_is_the_same_as_logging_off() {
    let (mut desktop, ctx, auth) = logon();
    auth.push(Ok(id("alice", 1000)));
    sign_in(&mut desktop, &ctx, "alice", "gw");
    until(&mut desktop, &ctx, logged_on);
    desktop.do_action(&ctx, Action::SwitchServer);
    assert!(desktop.login.is_some());
}

#[test]
fn log_off_does_nothing_without_a_logon() {
    let (mut desktop, ctx, _) = desktop();
    desktop.do_action(&ctx, Action::LogOff);
    assert!(desktop.login.is_none());
}

#[test]
fn the_local_mode_never_shows_the_box() {
    let (mut desktop, ctx, fake) = desktop();
    *fake.local.lock().unwrap() = true;
    desktop.require_login(Arc::new(FakeAuth::default()));
    assert!(desktop.login.is_none() && desktop.auth.is_none());
    frames(&mut desktop, &ctx, 2);
}
