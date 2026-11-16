//! headless tests for the update notice and the Developer menu. the rebuild runs against a
//! scratch folder with a fake build script, so no real build or network call happens.

use std::path::Path;
use std::sync::Arc;

use lanlord_core::log;
use lanlord_core::update::{Runner, Source, Update};
use lanlord_core::updater::State;

use eframe::egui;

use super::harness::{click_at, desktop, frames, press, screen, until, SCREEN};
use super::menu::Action;
#[cfg(unix)]
use super::menu::Confirm;
use super::{theme, Desktop, TASKBAR};
use crate::login::fake::{id, FakeAuth};
use crate::update_panel::Updates;

/// a checkout that is never behind trunk.
struct Level;

impl Runner for Level {
    fn run(&self, _p: &str, args: &[&str], _d: Option<&Path>) -> anyhow::Result<String> {
        Ok(match args.first().copied() {
            Some("rev-list") => "0\n".into(),
            _ => String::new(),
        })
    }
}

#[cfg(unix)]
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lanlord-dev-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(unix)]
fn script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

/// a desktop that is a development build: a scratch checkout whose app build script succeeds
/// or fails as asked, and a fake installed app to start afterwards.
#[cfg(unix)]
fn dev(name: &str, build_ok: bool) -> (Desktop, egui::Context, std::path::PathBuf) {
    let repo = scratch(name);
    script(
        &repo.join("tools/build-app.sh"),
        if build_ok { "exit 0" } else { "exit 1" },
    );
    let exe = repo.join("Fake.app/Contents/MacOS/lanlord-bin");
    // the new build stays up for a moment, as a real one would
    script(&exe, "sleep 1");
    let (mut desktop, ctx, _) = desktop();
    desktop.relaunch_grace = lanlord_core::update::Grace {
        total: std::time::Duration::from_millis(150),
        poll: std::time::Duration::from_millis(10),
    };
    desktop.watch_updates(
        Updates::new(Arc::new(Level), None, true),
        Some(repo.clone()),
        Some(exe),
    );
    frames(&mut desktop, &ctx, 2);
    (desktop, ctx, repo)
}

fn open_menu(desktop: &mut Desktop, ctx: &egui::Context) {
    desktop.start_open = true;
    frames(desktop, ctx, 3);
}

fn choose(desktop: &mut Desktop, ctx: &egui::Context, action: &Action) {
    frames(desktop, ctx, 2);
    let at = desktop
        .menu_item_at(screen(), action)
        .unwrap_or_else(|| panic!("no menu item for {action:?}"));
    click_at(desktop, ctx, at);
}

#[cfg(unix)]
fn choose_dev(desktop: &mut Desktop, ctx: &egui::Context, action: &Action) {
    frames(desktop, ctx, 2);
    let at = desktop
        .dev_item_at(screen(), action)
        .unwrap_or_else(|| panic!("no Developer item for {action:?}"));
    click_at(desktop, ctx, at);
}

fn lines(seen: &std::sync::mpsc::Receiver<log::Line>) -> Vec<String> {
    seen.try_iter().map(|l| l.text).collect()
}

fn found(source: Source) -> Update {
    Update {
        source,
        headline: "1 new change on trunk".into(),
        lines: vec!["abc feat: something".into()],
    }
}

#[test]
fn a_release_build_has_no_developer_menu() {
    let (mut desktop, ctx, _) = desktop();
    open_menu(&mut desktop, &ctx);
    assert!(desktop.menu_item_at(screen(), &Action::Developer).is_none());
    assert!(desktop
        .menu_item_at(screen(), &Action::ToggleUpdates)
        .is_some());
}

#[cfg(unix)]
#[test]
fn the_developer_submenu_opens_beside_its_row_and_closes_with_the_menu() {
    let (mut desktop, ctx, _) = dev("submenu", true);
    open_menu(&mut desktop, &ctx);
    assert!(desktop.dev_item_at(screen(), &Action::Rebuild).is_none());
    choose(&mut desktop, &ctx, &Action::Developer);
    assert!(desktop.start_open && desktop.dev_open, "the menu stays up");
    for action in [Action::Rebuild, Action::CheckUpdates] {
        assert!(
            desktop.dev_item_at(screen(), &action).is_some(),
            "{action:?}"
        );
    }
    click_at(&mut desktop, &ctx, egui::Pos2::new(900.0, 300.0));
    assert!(!desktop.start_open && !desktop.dev_open);
}

#[cfg(unix)]
#[test]
fn rebuilding_asks_first_and_cancel_runs_nothing() {
    let (mut desktop, ctx, repo) = dev("ask", true);
    let seen = log::subscribe();
    open_menu(&mut desktop, &ctx);
    choose(&mut desktop, &ctx, &Action::Developer);
    choose_dev(&mut desktop, &ctx, &Action::Rebuild);
    assert!(desktop.confirm.is_some() && !desktop.is_rebuilding());
    assert!(press(&mut desktop, &ctx, "Cancel"));
    frames(&mut desktop, &ctx, 2);
    assert!(desktop.confirm.is_none() && !desktop.is_rebuilding());
    // the log is shared with the other tests, so look only for this checkout's commands
    let here = repo.display().to_string();
    assert!(!lines(&seen).iter().any(|l| l.contains(&here)));
    let _ = std::fs::remove_dir_all(repo);
}

#[cfg(unix)]
#[test]
fn a_rebuild_streams_into_the_console_and_starts_the_new_build() {
    let (mut desktop, ctx, repo) = dev("works", true);
    let seen = log::subscribe();
    open_menu(&mut desktop, &ctx);
    choose(&mut desktop, &ctx, &Action::Developer);
    choose_dev(&mut desktop, &ctx, &Action::Rebuild);
    assert!(press(&mut desktop, &ctx, "OK"));
    until(&mut desktop, &ctx, |d| !d.is_rebuilding());
    let said = lines(&seen);
    assert!(
        said.iter().any(|l| l.contains("tools/build-app.sh")),
        "{said:?}"
    );
    assert!(said.iter().any(|l| l.contains("rebuilt")), "{said:?}");
    assert!(
        said.iter().any(|l| l.contains("the new build is running")),
        "{said:?}"
    );
    assert!(
        desktop.confirm.is_none(),
        "a working rebuild shows no error"
    );
    assert!(
        desktop
            .windows
            .iter()
            .any(|w| w.target == super::Target::Console),
        "the console opens to show the build"
    );
    let _ = std::fs::remove_dir_all(repo);
}

#[cfg(unix)]
#[test]
fn a_failed_rebuild_says_why_and_stays_open() {
    let (mut desktop, ctx, repo) = dev("fails", false);
    let seen = log::subscribe();
    desktop.start_rebuild(false);
    assert!(desktop.is_rebuilding());
    desktop.start_rebuild(false);
    until(&mut desktop, &ctx, |d| !d.is_rebuilding());
    let said = lines(&seen);
    let here = repo.display().to_string();
    assert!(
        said.iter()
            .any(|l| l.contains("the rebuild failed") && l.contains(&here)),
        "{said:?}"
    );
    assert!(matches!(desktop.confirm, Some((Confirm::Failure, _))));
    let _ = std::fs::remove_dir_all(repo);
}

/// the path of the fake installed app the `dev` desktop starts.
#[cfg(unix)]
fn fake_exe(repo: &Path) -> std::path::PathBuf {
    repo.join("Fake.app/Contents/MacOS/lanlord-bin")
}

#[cfg(unix)]
#[test]
fn a_missing_checkout_is_caught_before_a_build_starts() {
    let (mut desktop, ctx, repo) = dev("gone", true);
    let seen = log::subscribe();
    std::fs::remove_dir_all(&repo).unwrap();
    desktop.start_rebuild(true);
    frames(&mut desktop, &ctx, 2);
    assert!(!desktop.is_rebuilding(), "nothing was started");
    let said = lines(&seen);
    let here = repo.display().to_string();
    assert!(
        said.iter()
            .any(|l| l.contains("is gone") && l.contains(&here)),
        "{said:?}"
    );
    assert!(
        !said
            .iter()
            .any(|l| l.starts_with("$ ") && l.contains(&here)),
        "no command ran"
    );
    assert!(matches!(desktop.confirm, Some((Confirm::Failure, _))));
}

#[cfg(unix)]
#[test]
fn a_new_build_that_exits_at_once_leaves_this_window_open_and_says_why() {
    let (mut desktop, ctx, repo) = dev("dies", true);
    let seen = log::subscribe();
    script(&fake_exe(&repo), "echo no display >&2; exit 4");
    desktop.start_rebuild(false);
    until(&mut desktop, &ctx, |d| !d.is_rebuilding());
    let said = lines(&seen);
    assert!(
        said.iter().any(|l| l.contains("exited at once")),
        "{said:?}"
    );
    assert!(said.iter().any(|l| l.contains("no display")), "{said:?}");
    assert!(!said.iter().any(|l| l.contains("the new build is running")));
    assert!(matches!(desktop.confirm, Some((Confirm::Failure, _))));
    let _ = std::fs::remove_dir_all(repo);
}

#[cfg(unix)]
#[test]
fn a_build_that_leaves_no_program_behind_leaves_this_window_open() {
    let (mut desktop, ctx, repo) = dev("nobin", true);
    let seen = log::subscribe();
    std::fs::remove_file(fake_exe(&repo)).unwrap();
    desktop.start_rebuild(false);
    until(&mut desktop, &ctx, |d| !d.is_rebuilding());
    let said = lines(&seen);
    assert!(said.iter().any(|l| l.contains("is missing")), "{said:?}");
    assert!(matches!(desktop.confirm, Some((Confirm::Failure, _))));
    let _ = std::fs::remove_dir_all(repo);
}

#[cfg(unix)]
#[test]
fn restarting_into_the_new_build_quits_only_when_it_is_up() {
    let (mut desktop, ctx, repo) = dev("restart", true);
    let seen = log::subscribe();
    // a build that stays up: the restart finishes without an error box
    desktop.restart_into_new_build();
    assert!(desktop.is_rebuilding());
    until(&mut desktop, &ctx, |d| !d.is_rebuilding());
    assert!(desktop.confirm.is_none());
    // a build that dies at once: the window stays and says why
    script(&fake_exe(&repo), "echo broken >&2; exit 5");
    desktop.restart_into_new_build();
    until(&mut desktop, &ctx, |d| !d.is_rebuilding());
    let said = lines(&seen);
    assert!(
        said.iter()
            .any(|l| l.contains("exited at once") && l.contains("broken")),
        "{said:?}"
    );
    assert!(matches!(desktop.confirm, Some((Confirm::Failure, _))));
    // and with nothing to start there is nothing to do
    let (mut bare, _ctx, _) = super::harness::desktop();
    bare.restart_into_new_build();
    assert!(!bare.is_rebuilding());
    let _ = std::fs::remove_dir_all(repo);
}

#[cfg(unix)]
#[test]
fn the_failure_box_closes_with_ok() {
    let (mut desktop, ctx, repo) = dev("box", false);
    desktop.start_rebuild(false);
    until(&mut desktop, &ctx, |d| !d.is_rebuilding());
    assert!(desktop.confirm.is_some());
    assert!(press(&mut desktop, &ctx, "OK"));
    frames(&mut desktop, &ctx, 2);
    assert!(desktop.confirm.is_none());
    let _ = std::fs::remove_dir_all(repo);
}

#[cfg(unix)]
#[test]
fn the_pull_tick_adds_a_pull_before_the_build() {
    let (mut desktop, ctx, repo) = dev("pull", true);
    let seen = log::subscribe();
    open_menu(&mut desktop, &ctx);
    choose(&mut desktop, &ctx, &Action::Developer);
    choose_dev(&mut desktop, &ctx, &Action::Rebuild);
    desktop.confirm.as_mut().unwrap().1.checks[0].1 = true;
    assert!(press(&mut desktop, &ctx, "OK"));
    until(&mut desktop, &ctx, |d| !d.is_rebuilding());
    let said = lines(&seen);
    assert!(said.iter().any(|l| l.starts_with("$ git pull")), "{said:?}");
    // the scratch folder is no checkout, so the pull fails and the build never starts
    assert!(said.iter().any(|l| l.contains("the rebuild failed")));
    let _ = std::fs::remove_dir_all(repo);
}

#[test]
fn a_build_that_cannot_rebuild_says_so() {
    let (mut desktop, _ctx, _) = desktop();
    let seen = log::subscribe();
    desktop.start_rebuild(false);
    assert!(!desktop.is_rebuilding());
    assert!(lines(&seen).iter().any(|l| l.contains("cannot be rebuilt")));
}

#[test]
fn the_check_for_updates_item_switches_the_background_check() {
    let (mut desktop, ctx, _) = desktop();
    desktop.watch_updates(Updates::new(Arc::new(Level), None, true), None, None);
    open_menu(&mut desktop, &ctx);
    choose(&mut desktop, &ctx, &Action::ToggleUpdates);
    assert!(!desktop.updates.enabled());
    open_menu(&mut desktop, &ctx);
    choose(&mut desktop, &ctx, &Action::ToggleUpdates);
    assert!(desktop.updates.enabled());
}

#[cfg(unix)]
#[test]
fn check_for_updates_now_looks_and_says_so() {
    let (mut desktop, ctx, repo) = dev("now", true);
    let seen = log::subscribe();
    open_menu(&mut desktop, &ctx);
    choose(&mut desktop, &ctx, &Action::Developer);
    choose_dev(&mut desktop, &ctx, &Action::CheckUpdates);
    assert!(lines(&seen)
        .iter()
        .any(|l| l.contains("looking for updates")));
    let _ = std::fs::remove_dir_all(repo);
}

#[test]
fn an_update_shows_a_shield_by_the_clock_and_a_balloon() {
    let (mut desktop, ctx, _) = desktop();
    frames(&mut desktop, &ctx, 2);
    assert!(theme::probe::find(&ctx, "shield").is_none());
    desktop.updates.pretend(found(Source::Git));
    frames(&mut desktop, &ctx, 3);
    let (shield, _) = theme::probe::find(&ctx, "shield").expect("the shield");
    assert!(
        shield.center().y > SCREEN.y - TASKBAR,
        "it is in the taskbar"
    );
    assert!(theme::probe::find(&ctx, "balloon").is_some());
    // clicking the balloon opens the box
    let (balloon, _) = theme::probe::find(&ctx, "balloon").unwrap();
    click_at(&mut desktop, &ctx, balloon.center());
    assert!(desktop.updates.dialog_open());
}

#[test]
fn the_shield_opens_the_box_and_remind_me_later_hides_it() {
    let (mut desktop, ctx, _) = desktop();
    desktop.updates.pretend(found(Source::Git));
    frames(&mut desktop, &ctx, 3);
    let (shield, _) = theme::probe::find(&ctx, "shield").unwrap();
    click_at(&mut desktop, &ctx, shield.center());
    assert!(desktop.updates.dialog_open());
    assert!(press(&mut desktop, &ctx, "Remind me later"));
    frames(&mut desktop, &ctx, 2);
    assert!(!desktop.updates.visible() && !desktop.updates.dialog_open());
}

#[cfg(unix)]
#[test]
fn installing_a_trunk_update_rebuilds() {
    let (mut desktop, ctx, repo) = dev("install", true);
    let seen = log::subscribe();
    desktop.updates.pretend(found(Source::Git));
    frames(&mut desktop, &ctx, 3);
    desktop.updates.open_dialog();
    assert!(press(&mut desktop, &ctx, "Install"));
    // the pull runs in the background; the scratch folder is no checkout, so it ends in a failure
    until(&mut desktop, &ctx, |d| {
        matches!(d.updates.state(), State::Failed(_))
    });
    let said = lines(&seen);
    assert!(said.iter().any(|l| l.starts_with("$ git pull")), "{said:?}");
    assert!(
        desktop
            .windows
            .iter()
            .any(|w| w.target == super::Target::Console),
        "the console opens to show the work"
    );
    // the shield stays with its failure, and the box offers to try again or dismiss it
    desktop.updates.open_dialog();
    frames(&mut desktop, &ctx, 3);
    assert!(press(&mut desktop, &ctx, "Dismiss"));
    frames(&mut desktop, &ctx, 2);
    assert_eq!(*desktop.updates.state(), State::Available);
    let _ = std::fs::remove_dir_all(repo);
}

#[test]
fn installing_a_release_only_opens_a_project_page() {
    let (mut desktop, ctx, _) = desktop();
    let seen = log::subscribe();
    let release = Source::Release {
        url: "https://example.com/not-this-project".into(),
    };
    desktop.updates.pretend(found(release));
    frames(&mut desktop, &ctx, 3);
    desktop.updates.open_dialog();
    assert!(press(&mut desktop, &ctx, "Install"));
    let said = lines(&seen);
    assert!(
        said.iter()
            .any(|l| l.contains("could not open the download page")),
        "{said:?}"
    );
}

#[test]
fn a_relaunched_build_signs_in_to_the_last_server_on_its_own() {
    let (mut desktop, ctx, _) = desktop();
    let auth = Arc::new(FakeAuth::default());
    desktop.require_login(auth.clone());
    auth.push(Ok(id("alice", 1000)));
    desktop.resume("alice@gw.lan");
    until(&mut desktop, &ctx, |d| {
        d.login.is_none() && d.signed_in.is_some()
    });
    assert_eq!(desktop.host().as_deref(), Some("alice@gw.lan"));
    assert_eq!(auth.calls(), ["attempt alice@gw.lan - false"]);
    // with no logon box there is nothing to resume
    let (mut local, _, _) = super::harness::desktop();
    local.resume("alice@gw.lan");
    assert!(local.login.is_none());
}
