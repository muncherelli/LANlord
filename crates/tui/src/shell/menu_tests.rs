//! the LANlord menu's upkeep actions and the developer tools driven through the terminal desktop:
//! the updates switch and "check for updates now", the Developer submenu, rebuilding, the update
//! box and, in a development build, the red bug and its menu.

use std::path::Path;
use std::sync::Arc;

use lanlord_app::gateway::Fake;
use lanlord_app::menu::{Action, Row};
use lanlord_app::update_box::{Choice, Msg};
use lanlord_app::updates::Notice;
use lanlord_core::update::Runner;
use lanlord_core::updater::{Pause, RealClock, State};
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Position;

use super::screens::{develop, developer_row, news};
use super::tests::{click, frame, key, shell_with};
use super::window_tests::{find, press, until};
use super::{maintenance, Modal, Shell};

/// pretends to be git: always one new commit.
struct Behind;

impl Runner for Behind {
    fn run(&self, _program: &str, args: &[&str], _dir: Option<&Path>) -> anyhow::Result<String> {
        Ok(match args.first().copied() {
            Some("rev-list") => "1\n".into(),
            Some("log") => "abc feat: something new\n".into(),
            _ => String::new(),
        })
    }
}

/// a desktop whose update check asks the pretend git, with the check `enabled` or not.
fn checking(enabled: bool) -> Shell {
    let mut shell = shell_with(Fake::new());
    shell.use_demo_server();
    shell.hide_bug();
    let notice = Notice::new(
        Arc::new(Behind),
        None,
        enabled,
        Arc::new(RealClock::new()),
        Arc::new(|| {}),
    );
    shell.updates = notice;
    frame(&mut shell);
    shell
}

/// the same, as a development build that can rebuild from a checkout.
fn developing() -> Shell {
    let mut shell = checking(false);
    develop(&mut shell);
    frame(&mut shell);
    shell
}

fn open_menu(shell: &mut Shell) -> String {
    let text = frame(shell);
    click(shell, find(&text, "LANlord"));
    frame(shell)
}

#[test]
fn the_updates_switch_turns_the_background_check_on_and_off() {
    let mut shell = checking(false);
    assert!(!shell.updates.enabled());
    let text = open_menu(&mut shell);
    assert!(text.contains("Automatic updates"), "{text}");
    press(&mut shell, "Automatic updates");
    assert!(shell.updates.enabled());
    assert!(shell.menu.is_none(), "the menu closes after the pick");
    // turning it on looks at once, and the answer shows in the tray
    let text = until(&mut shell, "Update ready");
    assert!(text.contains("Update ready"), "{text}");
    open_menu(&mut shell);
    press(&mut shell, "Automatic updates");
    assert!(!shell.updates.enabled());
    let text = frame(&mut shell);
    assert!(
        !text.contains("Update ready"),
        "off forgets the update:\n{text}"
    );
}

#[test]
fn a_release_build_has_no_developer_row_and_cannot_rebuild() {
    let mut shell = checking(false);
    let text = open_menu(&mut shell);
    assert!(!text.contains("Developer"), "{text}");
    shell.do_action(Action::Rebuild);
    assert!(shell.modal.is_none());
    assert!(shell.restart.is_none());
}

#[test]
fn the_developer_submenu_opens_beside_the_menu_and_closes_with_it() {
    let mut shell = developing();
    let text = open_menu(&mut shell);
    assert!(text.contains("Developer"), "{text}");
    let text = press(&mut shell, "Developer");
    assert!(text.contains("Rebuild and relaunch..."), "{text}");
    assert!(text.contains("Check for updates now"), "{text}");
    // the same row again folds it away
    let text = press(&mut shell, "Developer");
    assert!(!text.contains("Check for updates now"), "{text}");
    press(&mut shell, "Developer");
    // a click elsewhere closes both
    click(&mut shell, Position::new(70, 3));
    let text = frame(&mut shell);
    assert!(
        !text.contains("Check for updates now") && shell.menu.is_none(),
        "{text}"
    );
}

#[test]
fn the_developer_submenu_works_from_the_keyboard() {
    let mut shell = developing();
    key(&mut shell, KeyCode::F(10));
    shell.menu = Some(developer_row(&shell));
    key(&mut shell, KeyCode::Right);
    assert_eq!(shell.submenu, Some(0));
    key(&mut shell, KeyCode::Down);
    assert_eq!(shell.submenu, Some(1));
    key(&mut shell, KeyCode::Down);
    assert_eq!(shell.submenu, Some(0), "it wraps");
    key(&mut shell, KeyCode::Up);
    assert_eq!(shell.submenu, Some(1));
    key(&mut shell, KeyCode::Left);
    assert_eq!(shell.submenu, None);
    key(&mut shell, KeyCode::Right);
    key(&mut shell, KeyCode::Esc);
    assert_eq!(shell.submenu, None);
    assert!(shell.menu.is_some(), "Escape closes one level at a time");
    // Right on any other row does nothing
    shell.menu = Some(0);
    key(&mut shell, KeyCode::Right);
    assert_eq!(shell.submenu, None);
}

#[test]
fn check_for_updates_now_finds_the_update_and_the_tray_opens_its_box() {
    // the background check is on, so the answer is collected
    let mut shell = developing();
    shell.updates.set_enabled(true);
    open_menu(&mut shell);
    press(&mut shell, "Developer");
    press(&mut shell, "Check for updates now");
    assert!(shell.menu.is_none() && shell.submenu.is_none());
    let text = until(&mut shell, "Update ready");
    assert!(
        !text.contains("LANlord Update"),
        "the box waits for a click:\n{text}"
    );
    let text = press(&mut shell, "Update ready");
    assert!(text.contains("LANlord Update"), "{text}");
    assert!(
        text.contains("Install") && text.contains("Remind me later"),
        "{text}"
    );
    // closing the box with Escape is "remind me later": the tray label goes
    key(&mut shell, KeyCode::Esc);
    let text = frame(&mut shell);
    assert!(
        !text.contains("LANlord Update") && !text.contains("Update ready"),
        "{text}"
    );
}

#[test]
fn the_update_box_buttons_drive_the_updater() {
    let mut shell = checking(true);
    shell.updates.arrived(Some(news()));
    frame(&mut shell);
    key(&mut shell, KeyCode::F(9));
    let text = frame(&mut shell);
    assert!(text.contains("LANlord Update"), "{text}");
    assert!(text.contains("Check for updates automatically"), "{text}");
    // the check box in the box is the same switch as the menu's
    shell.answer_update(Msg::ToggleAutomatic);
    assert!(!shell.updates.enabled());
    shell.answer_update(Msg::ToggleAutomatic);
    assert!(shell.updates.enabled());
    // pause, resume and cancel go to the updater; with nothing running they change nothing
    for choice in [Choice::Pause, Choice::Resume, Choice::Cancel] {
        shell.answer_update(Msg::Choose(choice));
    }
    shell.answer_update(Msg::Choose(Choice::Later));
    assert!(!shell.update_dialog_open());
    assert!(shell.update_label().is_none(), "snoozed");
}

#[test]
fn install_opens_the_console_and_restart_asks_the_rebuild_to_verify() {
    let mut shell = checking(true);
    shell.updates.arrived(Some(news()));
    shell.updates.open_dialog();
    shell.answer_update(Msg::Choose(Choice::Install));
    assert!(!shell.update_dialog_open());
    assert!(shell
        .windows
        .iter()
        .any(|w| w.target == super::Target::Console));
    // restarting with no build to start says so in the console and starts nothing
    shell.answer_update(Msg::Choose(Choice::Restart));
    assert!(!shell.rebuild.busy());
}

#[test]
fn every_state_of_the_updater_has_a_tray_label_but_idle() {
    assert_eq!(maintenance::label_for(&State::Idle), None);
    for (state, label) in [
        (State::Available, "Update ready"),
        (State::Downloading, "Downloading"),
        (State::Installing, "Installing"),
        (State::Paused(Pause::Manual), "Update paused"),
        (State::Failed("no network".into()), "Update failed"),
        (State::Done, "Restart now"),
    ] {
        assert_eq!(maintenance::label_for(&state), Some(label));
    }
}

#[test]
fn rebuild_asks_first_and_a_missing_checkout_is_reported_in_a_box() {
    let mut shell = developing();
    open_menu(&mut shell);
    press(&mut shell, "Developer");
    let text = press(&mut shell, "Rebuild and relaunch...");
    assert!(text.contains("Rebuild LANlord from /srv/lanlord"), "{text}");
    assert!(text.contains("Pull the latest trunk first"), "{text}");
    assert!(
        text.contains("new build starts"),
        "the tui takes over its own terminal:\n{text}"
    );
    // Cancel leaves everything alone
    let text = press(&mut shell, "Cancel");
    assert!(!text.contains("Rebuild LANlord from"), "{text}");
    assert!(!shell.rebuild.busy() && shell.modal.is_none());
    // OK with no checkout on disk stops before anything runs
    shell.do_action(Action::Rebuild);
    frame(&mut shell);
    let text = press(&mut shell, "OK");
    assert!(text.contains("The rebuild did not work"), "{text}");
    assert!(matches!(shell.modal, Some(Modal::Failure(_))));
    assert!(!shell.quit && shell.restart.is_none());
    let text = press(&mut shell, "OK");
    assert!(!text.contains("The rebuild did not work"), "{text}");
}

#[test]
fn a_finished_rebuild_ends_the_tui_and_hands_the_terminal_to_the_new_build() {
    let mut shell = developing();
    let exe = std::env::current_exe().unwrap();
    shell
        .rebuild
        .watch(Some("/srv/lanlord".into()), Some(exe.clone()));
    assert!(shell
        .rebuild
        .restart(None, &lanlord_app::wake::Wake::none()));
    let started = std::time::Instant::now();
    while !shell.quit && started.elapsed() < std::time::Duration::from_secs(5) {
        shell.poll();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(shell.quit, "a build that is there ends this one");
    assert_eq!(shell.take_restart(), Some((exe, None)));
    assert_eq!(shell.take_restart(), None);
}

#[test]
fn a_rebuild_that_fails_at_the_end_leaves_the_tui_open_with_the_reason() {
    let mut shell = developing();
    shell.rebuild.watch(None, Some("/no/such/lanlord".into()));
    assert!(shell
        .rebuild
        .restart(None, &lanlord_app::wake::Wake::none()));
    let text = until(&mut shell, "The rebuild did not work");
    assert!(text.contains("is missing"), "{text}");
    assert!(!shell.quit);
}

#[test]
fn the_developer_rows_are_there_only_with_a_checkout() {
    let mut shell = checking(false);
    let has = |shell: &Shell| {
        shell
            .menu_state()
            .iter()
            .any(|row| matches!(row, Row::Item(item) if item.action == Action::Developer))
    };
    assert!(!has(&shell));
    develop(&mut shell);
    assert!(has(&shell));
    shell.watch_updates(Notice::quiet(), None, None);
    assert!(!has(&shell), "a release build has no checkout");
}

#[test]
fn watching_updates_points_the_updater_at_the_checkout() {
    let mut shell = checking(false);
    shell.watch_updates(
        Notice::quiet(),
        Some("/srv/lanlord".into()),
        Some("/srv/lanlord/target/debug/lanlord".into()),
    );
    assert_eq!(shell.rebuild.repo(), Some(Path::new("/srv/lanlord")));
    assert!(!shell.updates.enabled());
}

#[test]
fn a_rebuilt_build_signs_in_again_to_the_server_it_was_on() {
    let mut shell = shell_with(Fake::new());
    // without a way to sign in there is nothing to resume
    shell.resume("ann@gateway.example");
    assert!(shell.login.is_none());
    shell.require_login(Arc::new(lanlord_app::login::fake::FakeAuth::default()));
    shell.login = None;
    shell.resume("ann@gateway.example");
    assert!(shell.login.is_some());
}

#[cfg(feature = "devtools")]
mod devtools {
    use ratatui::crossterm::event::KeyCode;

    use super::super::tests::{click, frame, key, shell_with};
    use super::super::window_tests::{find, press};
    use super::super::{Shell, Target};
    use lanlord_app::gateway::Fake;

    fn desktop() -> Shell {
        let mut shell = shell_with(Fake::new());
        shell.use_demo_server();
        frame(&mut shell);
        shell
    }

    #[test]
    fn the_bug_is_on_the_taskbar_and_opens_the_debug_menu() {
        let mut shell = desktop();
        let text = frame(&mut shell);
        assert!(text.contains("Ж Debug"), "{text}");
        let text = press(&mut shell, "Ж Debug");
        assert!(text.contains("Demo desktop"), "{text}");
        assert!(text.contains("Window manager debug"), "{text}");
        assert!(text.contains("Open all windows"), "{text}");
        assert!(shell.debug_menu_open());
        // the bug again folds it away; so does a click anywhere else
        let text = press(&mut shell, "Ж Debug");
        assert!(!text.contains("Open all windows"), "{text}");
        press(&mut shell, "Ж Debug");
        click(&mut shell, ratatui::layout::Position::new(70, 3));
        assert!(!shell.debug_menu_open());
    }

    #[test]
    fn open_all_windows_opens_every_window_there_is() {
        let mut shell = desktop();
        press(&mut shell, "Ж Debug");
        press(&mut shell, "Open all windows");
        for target in [
            Target::MyServer,
            Target::Welcome,
            Target::Changelog,
            Target::DateTime,
            Target::Components,
        ] {
            assert!(
                shell.windows.iter().any(|w| w.target == target),
                "{target:?} is open"
            );
        }
        assert!(shell.debug_menu_open(), "the menu stays up, as in the gui");
    }

    #[test]
    fn window_manager_debug_outlines_the_windows_and_says_their_size() {
        let mut shell = desktop();
        shell.open(Target::MyServer);
        let plain = frame(&mut shell);
        assert!(!plain.contains("my-server: "), "{plain}");
        press(&mut shell, "Ж Debug");
        let text = press(&mut shell, "Window manager debug");
        assert!(shell.wm_debug());
        assert!(text.contains("my-server: "), "{text}");
        let text = press(&mut shell, "Window manager debug");
        assert!(!shell.wm_debug() && !text.contains("my-server: "), "{text}");
    }

    #[test]
    fn the_demo_desktop_is_a_switch_and_leaving_it_brings_the_real_desktop_back() {
        let mut shell = shell_with(Fake::new());
        shell.require_login(std::sync::Arc::new(
            lanlord_app::login::fake::FakeAuth::default(),
        ));
        frame(&mut shell);
        assert!(shell.before_desktop());
        // the logon screen has the bug in its corner
        let text = press(&mut shell, "Ж Debug");
        assert!(text.contains("Demo desktop"), "{text}");
        let text = press(&mut shell, "Demo desktop");
        assert!(shell.in_demo() && !shell.before_desktop());
        assert!(text.contains("Open all windows"), "{text}");
        shell.open(Target::MyServer);
        let text = press(&mut shell, "Demo desktop");
        assert!(!shell.in_demo() && shell.before_desktop());
        assert!(shell.windows.is_empty(), "the demo's windows are gone");
        assert!(
            !text.contains("LANlord Client") || text.contains("Log"),
            "{text}"
        );
        // leaving when not in the demo does nothing
        assert!(!shell.leave_demo());
    }

    #[test]
    fn logging_off_inside_the_demo_returns_to_the_real_logon_and_signs_nothing_out() {
        let mut shell = shell_with(Fake::new());
        shell.require_login(std::sync::Arc::new(
            lanlord_app::login::fake::FakeAuth::default(),
        ));
        shell.pick_debug(0);
        assert!(shell.in_demo());
        shell.log_off();
        assert!(!shell.in_demo() && shell.before_desktop());
    }

    #[test]
    fn f12_and_the_arrow_keys_drive_the_debug_menu() {
        let mut shell = desktop();
        key(&mut shell, KeyCode::F(12));
        assert!(shell.debug_menu_open());
        key(&mut shell, KeyCode::Down);
        key(&mut shell, KeyCode::Down);
        key(&mut shell, KeyCode::Down);
        key(&mut shell, KeyCode::Up);
        key(&mut shell, KeyCode::Enter);
        assert!(shell.windows.len() > 3, "Enter on Open all windows");
        key(&mut shell, KeyCode::Esc);
        assert!(!shell.debug_menu_open());
        key(&mut shell, KeyCode::F(12));
        key(&mut shell, KeyCode::F(12));
        assert!(!shell.debug_menu_open());
        let text = frame(&mut shell);
        assert!(find(&text, "Ж Debug").y > 0);
    }
}

#[cfg(not(feature = "devtools"))]
#[test]
fn a_release_build_has_no_bug_and_no_debug_keys() {
    let mut shell = shell_with(Fake::new());
    shell.use_demo_server();
    let text = frame(&mut shell);
    assert!(!text.contains("Debug"), "{text}");
    key(&mut shell, KeyCode::F(12));
    assert!(!shell.leave_demo());
    let text = frame(&mut shell);
    assert!(!text.contains("Demo desktop"), "{text}");
}
