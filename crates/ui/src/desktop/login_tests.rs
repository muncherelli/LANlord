//! headless tests for the logon box on the desktop: it comes first, signing in reveals the
//! desktop for that server, and logging off brings it back.

use std::sync::Arc;

use lanlord_core::signin::SigninError;

use eframe::egui;

use super::harness::{desktop, enabled, frames, press, screen, until};
use super::menu::{Action, Row};
use super::{Desktop, Target};
use crate::admin_setup::AdminSetup;
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
    assert!(desktop.admin_setup.is_none(), "alice is not root");
    desktop.open(Target::Console);
    frames(&mut desktop, &ctx, 2);
    assert_eq!(
        desktop.windows.len(),
        2,
        "the console and the welcome screen"
    );
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

/// sign in as root on the fake server; the create-an-administrator screen comes up.
fn root_screen() -> (
    Desktop,
    egui::Context,
    Arc<FakeAuth>,
    Arc<lanlord_app::gateway::Fake>,
) {
    let (mut desktop, ctx, fake) = desktop();
    let auth = Arc::new(FakeAuth::default());
    desktop.require_login(auth.clone());
    frames(&mut desktop, &ctx, 3);
    auth.push(Ok(id("root", 0)));
    sign_in(&mut desktop, &ctx, "root", "gw");
    until(&mut desktop, &ctx, |d| d.admin_setup.is_some());
    // no agent on the test machine matters: use known keys
    desktop.admin_setup = Some(AdminSetup::with_test_keys(&["ssh-ed25519 AAAA me@laptop"]));
    frames(&mut desktop, &ctx, 3);
    (desktop, ctx, auth, fake)
}

fn fill_admin(desktop: &mut Desktop, name: &str, password: &str, confirm: &str) {
    let setup = desktop.admin_setup.as_mut().unwrap();
    setup.name = name.into();
    setup.set_passwords(password, confirm);
}

#[test]
fn root_lands_on_the_create_administrator_screen_before_the_desktop() {
    let (mut desktop, ctx, _, _) = root_screen();
    assert!(desktop.login.is_none() && desktop.signed_in.is_some());
    for button in ["Create", "Skip for now"] {
        assert_eq!(enabled(&ctx, button), Some(true), "{button}");
    }
    assert!(desktop.login.is_none(), "this is not the logon box");
    // the desktop is not drawn: its menu has nothing to open yet
    desktop.open(Target::Console);
    frames(&mut desktop, &ctx, 2);
    assert!(desktop.admin_setup.is_some());
}

#[test]
fn skipping_carries_on_as_root_on_the_desktop() {
    let (mut desktop, ctx, _, _) = root_screen();
    assert!(press(&mut desktop, &ctx, "Skip for now"));
    frames(&mut desktop, &ctx, 2);
    assert!(desktop.admin_setup.is_none());
    assert_eq!(desktop.host().as_deref(), Some("root@gw"));
}

#[test]
fn the_form_is_checked_before_anything_is_sent() {
    let (mut desktop, ctx, _, fake) = root_screen();
    assert!(press(&mut desktop, &ctx, "Create"));
    assert_eq!(
        desktop.admin_setup.as_ref().unwrap().status(),
        "Type a username for the new administrator."
    );
    fill_admin(&mut desktop, "alice", "", "");
    assert!(press(&mut desktop, &ctx, "Create"));
    assert!(desktop
        .admin_setup
        .as_ref()
        .unwrap()
        .status()
        .contains("Type a password"));
    fill_admin(&mut desktop, "alice", "one", "two");
    assert!(press(&mut desktop, &ctx, "Create"));
    assert!(desktop
        .admin_setup
        .as_ref()
        .unwrap()
        .status()
        .contains("not the same"));
    assert!(fake.calls().is_empty());
}

#[test]
fn creating_the_administrator_asks_for_a_logon_as_that_account() {
    let (mut desktop, ctx, auth, fake) = root_screen();
    fill_admin(&mut desktop, " alice ", "secret", "secret");
    desktop.admin_setup.as_mut().unwrap().key = "  ssh-ed25519 AAAA me@laptop \n".into();
    assert!(press(&mut desktop, &ctx, "Create"));
    until(&mut desktop, &ctx, |d| d.admin_setup.is_none());
    assert_eq!(
        fake.calls(),
        ["create-admin root@gw alice key=true nopasswd=false pw=secret"]
    );
    // the box is back with the new account filled in, and root is forgotten
    let login = desktop.login.as_ref().expect("back at the logon box");
    assert_eq!((login.user.as_str(), login.host.as_str()), ("alice", "gw"));
    assert_eq!(login.status(), "Created alice. Log on with it.");
    assert!(desktop.signed_in.is_none());
    assert!(desktop.hosts.hosts().is_empty(), "root@gw is not kept");
    let start = std::time::Instant::now();
    while !auth.calls().iter().any(|c| c == "sign_out root@gw") {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "no sign out"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn a_key_is_optional() {
    let (mut desktop, ctx, _, fake) = root_screen();
    fill_admin(&mut desktop, "bob", "pw", "pw");
    assert!(press(&mut desktop, &ctx, "Create"));
    until(&mut desktop, &ctx, |d| d.admin_setup.is_none());
    assert_eq!(
        fake.calls(),
        ["create-admin root@gw bob key=false nopasswd=false pw=pw"]
    );
}

#[test]
fn a_refused_account_shows_why_and_stays_on_the_screen() {
    let (mut desktop, ctx, _, fake) = root_screen();
    *fake.fail.lock().unwrap() = Some("the user alice already exists".into());
    fill_admin(&mut desktop, "alice", "pw", "pw");
    assert!(press(&mut desktop, &ctx, "Create"));
    until(&mut desktop, &ctx, |d| {
        d.admin_setup.as_ref().is_some_and(|s| !s.working())
    });
    let setup = desktop.admin_setup.as_ref().unwrap();
    assert!(
        setup.status().contains("already exists"),
        "{}",
        setup.status()
    );
    assert!(desktop.login.is_none());
}

#[test]
fn passwordless_sudo_needs_the_warning_to_be_accepted() {
    let (mut desktop, ctx, _, fake) = root_screen();
    let tick = "Allow sudo without a password (dangerous)";
    assert!(press(&mut desktop, &ctx, tick));
    let setup = desktop.admin_setup.as_ref().unwrap();
    assert!(setup.warning_open() && !setup.passwordless());
    // declining keeps the password
    assert!(press(&mut desktop, &ctx, "Keep the password"));
    assert!(!desktop.admin_setup.as_ref().unwrap().warning_open());
    assert!(!desktop.admin_setup.as_ref().unwrap().passwordless());
    // accepting turns it on, and unticking turns it off at once
    assert!(press(&mut desktop, &ctx, tick));
    assert!(press(&mut desktop, &ctx, "Allow it"));
    assert!(desktop.admin_setup.as_ref().unwrap().passwordless());
    fill_admin(&mut desktop, "alice", "pw", "pw");
    assert!(press(&mut desktop, &ctx, "Create"));
    until(&mut desktop, &ctx, |d| d.admin_setup.is_none());
    assert_eq!(
        fake.calls(),
        ["create-admin root@gw alice key=false nopasswd=true pw=pw"]
    );
}

#[test]
fn unticking_passwordless_sudo_needs_no_warning() {
    let (mut desktop, ctx, _, _) = root_screen();
    let tick = "Allow sudo without a password (dangerous)";
    assert!(press(&mut desktop, &ctx, tick));
    assert!(press(&mut desktop, &ctx, "Allow it"));
    assert!(press(&mut desktop, &ctx, tick));
    let setup = desktop.admin_setup.as_ref().unwrap();
    assert!(!setup.passwordless() && !setup.warning_open());
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
        .any(|r| matches!(r, Row::Item(i) if i.label() == "Log off alice...")));
}

#[test]
fn the_server_sits_at_the_top_of_the_menu_and_in_the_window_title() {
    let (mut desktop, ctx, auth) = logon();
    assert_eq!(desktop.server_line(), "No server");
    let client = format!("LANlord Client v{}", env!("CARGO_PKG_VERSION"));
    assert_eq!(desktop.window_title, client);
    auth.push(Ok(id("alice", 1000)));
    sign_in(&mut desktop, &ctx, "alice", "gw");
    until(&mut desktop, &ctx, logged_on);
    desktop.start_open = true;
    frames(&mut desktop, &ctx, 3);
    let rows = desktop.menu_rows();
    let Some(Row::Item(first)) = rows.first() else {
        panic!("the menu starts with the server line");
    };
    assert!(first.label().starts_with("Server: ") && first.label().contains("gw"));
    assert_eq!(
        desktop.window_title,
        format!("{client} - {}", desktop.host().unwrap())
    );
    // the line only names the server: clicking it does nothing
    let before = desktop.windows.len();
    desktop.do_action(&ctx, Action::ServerLine);
    assert_eq!(desktop.windows.len(), before);
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
fn a_demo_desktop_says_so_in_its_title() {
    let (mut desktop, ctx, fake) = desktop();
    *fake.local.lock().unwrap() = true;
    desktop.mark_demo();
    frames(&mut desktop, &ctx, 2);
    assert_eq!(
        desktop.window_title,
        format!("LANlord Server v{} (demo)", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn the_local_mode_never_shows_the_box() {
    let (mut desktop, ctx, fake) = desktop();
    *fake.local.lock().unwrap() = true;
    desktop.require_login(Arc::new(FakeAuth::default()));
    assert!(desktop.login.is_none() && desktop.auth.is_none());
    frames(&mut desktop, &ctx, 2);
    assert_eq!(
        desktop.window_title,
        format!("LANlord Server v{}", env!("CARGO_PKG_VERSION"))
    );
}

fn welcome_open(desktop: &Desktop) -> bool {
    desktop.windows.iter().any(|w| w.target == Target::Welcome)
}

#[test]
fn the_welcome_window_opens_after_logon_only_while_its_box_is_ticked() {
    let dir = std::env::temp_dir().join(format!("lanlord-ui-welcome-{}", std::process::id()));
    let file = dir.join("prefs");
    // the first logon: nothing remembered, so it opens
    let (mut desktop, ctx, auth) = logon();
    desktop.set_prefs(Some(file.clone()));
    auth.push(Ok(id("alice", 1000)));
    sign_in(&mut desktop, &ctx, "alice", "gw.lan");
    until(&mut desktop, &ctx, logged_on);
    assert!(welcome_open(&desktop));
    // clearing the box is remembered for alice, and Close closes the window
    desktop
        .welcome
        .update(lanlord_app::welcome::Msg::Toggle, &[Target::Networks]);
    assert!(press(&mut desktop, &ctx, "Close"));
    frames(&mut desktop, &ctx, 2);
    assert!(!welcome_open(&desktop));
    // the next logon of alice does not show it, bob's does
    for (user, shown) in [("alice", false), ("bob", true)] {
        let (mut desktop, ctx, auth) = logon();
        desktop.set_prefs(Some(file.clone()));
        auth.push(Ok(id(user, 1000)));
        sign_in(&mut desktop, &ctx, user, "gw.lan");
        until(&mut desktop, &ctx, logged_on);
        assert_eq!(welcome_open(&desktop), shown, "{user}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_menu_has_a_welcome_row_that_opens_the_window() {
    let (mut desktop, ctx, _) = desktop();
    assert!(!welcome_open(&desktop));
    desktop.do_action(&ctx, Action::Open(Target::Welcome));
    frames(&mut desktop, &ctx, 2);
    assert!(welcome_open(&desktop));
    assert!(press(&mut desktop, &ctx, "Close"));
    frames(&mut desktop, &ctx, 2);
    assert!(!welcome_open(&desktop));
}
