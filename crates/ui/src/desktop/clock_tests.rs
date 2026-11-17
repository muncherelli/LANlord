//! headless tests for the taskbar clock and the Date/Time Properties window, against the fake
//! gateway.

use std::sync::Arc;

use eframe::egui;

use super::harness::{click_at, desktop, enabled, frames, idle, press, until, with_host};
use super::{theme, Desktop, Target};
use crate::gateway::Fake;

fn open(desktop: &mut Desktop, ctx: &egui::Context) {
    desktop.open(Target::DateTime);
    until(desktop, ctx, |d| d.clock.is_loaded());
}

fn is_open(desktop: &Desktop) -> bool {
    desktop.windows.iter().any(|w| w.target == Target::DateTime)
}

fn setup() -> (Desktop, egui::Context, Arc<Fake>) {
    let (mut desktop, ctx, fake) = desktop();
    with_host(&mut desktop, &ctx);
    (desktop, ctx, fake)
}

#[test]
fn the_taskbar_clock_shows_the_servers_time_once_it_is_read() {
    let (mut desktop, ctx, fake) = setup();
    until(&mut desktop, &ctx, |d| d.feed.has_reading());
    // the fake server is on utc and its clock says 16:12
    assert_eq!(desktop.feed.text(), "4:12 PM");
    assert!(desktop.feed.tooltip().contains("me@gw") && desktop.feed.tooltip().contains("Etc/UTC"));
    assert_eq!(fake.reads.lock().unwrap().as_slice(), ["clock-state me@gw"]);
    assert!(
        fake.calls().iter().all(|c| !c.starts_with("clock")),
        "a quiet read is not a task: {:?}",
        fake.calls()
    );
    assert!(!desktop.services.busy("me@gw"));
}

#[test]
fn without_a_server_the_clock_is_this_computers() {
    let (mut desktop, ctx, fake) = desktop();
    frames(&mut desktop, &ctx, 3);
    assert!(!desktop.feed.has_reading());
    assert!(desktop.feed.tooltip().contains("This computer"));
    assert!(fake.reads.lock().unwrap().is_empty());
}

#[test]
fn a_failed_read_is_not_retried_every_frame() {
    let (mut desktop, ctx, fake) = desktop();
    *fake.fail.lock().unwrap() = Some("no route".into());
    with_host(&mut desktop, &ctx);
    until(&mut desktop, &ctx, |d| d.feed.job_is_idle());
    frames(&mut desktop, &ctx, 10);
    assert_eq!(fake.reads.lock().unwrap().len(), 1);
    assert!(!desktop.feed.has_reading());
}

#[test]
fn double_clicking_the_clock_opens_the_window_and_one_click_does_not() {
    let (mut desktop, ctx, _) = setup();
    frames(&mut desktop, &ctx, 2);
    let (clock, _) = theme::probe::find(&ctx, "clock").expect("the clock");
    assert!(
        clock.center().y > super::harness::SCREEN.y - super::TASKBAR,
        "it is in the taskbar"
    );
    click_at(&mut desktop, &ctx, clock.center());
    assert!(!is_open(&desktop), "one click only");
    click_at(&mut desktop, &ctx, clock.center());
    assert!(
        is_open(&desktop),
        "a double click opens Date/Time Properties"
    );
}

#[test]
fn the_window_reads_the_server_and_cancel_just_closes_it() {
    let (mut desktop, ctx, fake) = setup();
    open(&mut desktop, &ctx);
    assert!(
        enabled(&ctx, "Apply") == Some(false),
        "nothing to apply yet"
    );
    assert!(press(&mut desktop, &ctx, "Cancel"));
    frames(&mut desktop, &ctx, 3);
    assert!(!is_open(&desktop));
    assert!(fake.calls().iter().all(|c| !c.starts_with("clock-apply")));
}

#[test]
fn ok_with_nothing_changed_just_closes() {
    let (mut desktop, ctx, fake) = setup();
    open(&mut desktop, &ctx);
    assert!(press(&mut desktop, &ctx, "OK"));
    frames(&mut desktop, &ctx, 3);
    assert!(!is_open(&desktop));
    assert!(fake.calls().iter().all(|c| !c.starts_with("clock-apply")));
}

#[test]
fn applying_a_new_zone_goes_through_the_gateway_and_the_taskbar_follows() {
    let (mut desktop, ctx, fake) = setup();
    until(&mut desktop, &ctx, |d| d.feed.has_reading());
    open(&mut desktop, &ctx);
    desktop.clock.pretend_zone("Europe/Berlin");
    frames(&mut desktop, &ctx, 2);
    assert!(enabled(&ctx, "Apply") == Some(true));
    assert!(press(&mut desktop, &ctx, "Apply"));
    until(&mut desktop, &ctx, idle("me@gw"));
    frames(&mut desktop, &ctx, 3);
    let calls = fake.calls();
    assert!(
        calls
            .iter()
            .any(|c| c == "clock-apply me@gw zone=Some(\"Europe/Berlin\") time=None ntp=None"),
        "{calls:?}"
    );
    assert_eq!(fake.clock.lock().unwrap().timezone, "Europe/Berlin");
    assert!(desktop
        .clock
        .note()
        .unwrap()
        .contains("kept in the settings"));
    assert!(is_open(&desktop), "Apply keeps the window open");
    assert!(
        enabled(&ctx, "Apply") == Some(false),
        "what was applied is no longer a change"
    );
}

#[test]
fn ok_applies_and_closes_and_the_taskbar_reads_again() {
    let (mut desktop, ctx, fake) = setup();
    until(&mut desktop, &ctx, |d| d.feed.has_reading());
    open(&mut desktop, &ctx);
    desktop.clock.pretend_zone("UTC");
    assert!(press(&mut desktop, &ctx, "OK"));
    until(&mut desktop, &ctx, |d| !is_open(d));
    assert_eq!(fake.clock.lock().unwrap().timezone, "UTC");
    // closing the window leaves the taskbar clock to read the server again
    until(&mut desktop, &ctx, |_| {
        fake.reads.lock().unwrap().len() >= 2
    });
}

#[test]
fn a_time_is_set_by_clicking_a_day_and_a_spin_button_when_internet_time_is_off() {
    let (mut desktop, ctx, fake) = setup();
    fake.clock.lock().unwrap().ntp = false;
    open(&mut desktop, &ctx);
    frames(&mut desktop, &ctx, 2);
    assert_eq!(enabled(&ctx, "hour up"), Some(true));
    let (day, on) = theme::probe::find(&ctx, "day 2026-11-20").expect("the 20th");
    assert!(on);
    click_at(&mut desktop, &ctx, day.center());
    assert!(press(&mut desktop, &ctx, "hour up"));
    assert!(press(&mut desktop, &ctx, "Apply"));
    until(&mut desktop, &ctx, idle("me@gw"));
    let calls = fake.calls();
    let applied = calls.iter().find(|c| c.starts_with("clock-apply")).unwrap();
    assert!(
        applied.contains("time=Some(\"2026-11-20 17:12:"),
        "{applied}"
    );
    assert!(applied.contains("zone=None") && applied.contains("ntp=None"));
}

#[test]
fn the_time_cannot_be_edited_while_internet_time_is_on() {
    let (mut desktop, ctx, fake) = setup();
    open(&mut desktop, &ctx);
    frames(&mut desktop, &ctx, 2);
    assert_eq!(enabled(&ctx, "hour up"), Some(false));
    assert!(!press(&mut desktop, &ctx, "hour up"));
    assert!(fake.calls().iter().all(|c| !c.starts_with("clock-apply")));
}

#[test]
fn internet_time_is_changed_with_its_servers_and_updated_now() {
    let (mut desktop, ctx, fake) = setup();
    open(&mut desktop, &ctx);
    assert!(press(&mut desktop, &ctx, "Internet Time"));
    desktop
        .clock
        .pretend_ntp(true, "a.example.org b.example.org");
    assert!(press(&mut desktop, &ctx, "Apply"));
    until(&mut desktop, &ctx, idle("me@gw"));
    let calls = fake.calls();
    assert!(
        calls
            .iter()
            .any(|c| c.contains("ntp=Some((true, \"a.example.org b.example.org\"))")),
        "{calls:?}"
    );
    frames(&mut desktop, &ctx, 3);
    assert!(press(&mut desktop, &ctx, "Update now"));
    until(&mut desktop, &ctx, idle("me@gw"));
    assert!(fake.calls().contains(&"clock-update me@gw".to_string()));
}

#[test]
fn a_bad_server_name_is_refused_before_anything_is_sent() {
    let (mut desktop, ctx, fake) = setup();
    open(&mut desktop, &ctx);
    desktop.clock.pretend_ntp(true, "bad;name");
    assert!(press(&mut desktop, &ctx, "Apply"));
    frames(&mut desktop, &ctx, 3);
    assert!(desktop.clock.note().unwrap().contains("not a valid server"));
    assert!(fake.calls().iter().all(|c| !c.starts_with("clock-apply")));
}

#[test]
fn a_container_disables_the_time_and_internet_controls_but_not_the_zone() {
    let (mut desktop, ctx, fake) = setup();
    {
        let mut clock = fake.clock.lock().unwrap();
        clock.container = true;
        clock.ntp = false;
    }
    open(&mut desktop, &ctx);
    frames(&mut desktop, &ctx, 2);
    assert_eq!(enabled(&ctx, "hour up"), Some(false));
    assert!(press(&mut desktop, &ctx, "Internet Time"));
    frames(&mut desktop, &ctx, 2);
    assert_eq!(enabled(&ctx, "Update now"), Some(false));
    desktop.clock.pretend_zone("Europe/Berlin");
    desktop.clock.pretend_ntp(true, "a.example.org");
    assert!(press(&mut desktop, &ctx, "Apply"));
    until(&mut desktop, &ctx, idle("me@gw"));
    let calls = fake.calls();
    assert!(
        calls
            .iter()
            .any(|c| c == "clock-apply me@gw zone=Some(\"Europe/Berlin\") time=None ntp=None"),
        "only the zone goes to a container: {calls:?}"
    );
}

#[test]
fn a_zone_the_server_is_asked_about_shows_its_time() {
    let (mut desktop, ctx, fake) = setup();
    open(&mut desktop, &ctx);
    assert!(press(&mut desktop, &ctx, "Time Zone"));
    desktop.clock.pretend_zone("Europe/Berlin");
    until(&mut desktop, &ctx, |_| {
        fake.reads
            .lock()
            .unwrap()
            .iter()
            .any(|r| r == "clock-zone-now me@gw Europe/Berlin")
    });
    frames(&mut desktop, &ctx, 4);
}

#[test]
fn reopening_the_window_reads_the_server_again() {
    let (mut desktop, ctx, fake) = setup();
    open(&mut desktop, &ctx);
    assert!(press(&mut desktop, &ctx, "Cancel"));
    frames(&mut desktop, &ctx, 3);
    fake.clock.lock().unwrap().timezone = "Asia/Kolkata".into();
    open(&mut desktop, &ctx);
    desktop.clock.pretend_ntp(true, "pool.ntp.org");
    assert_eq!(desktop.clock.note().as_deref(), Some("read the clock"));
    let zones = fake
        .reads
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.starts_with("clock-zones"))
        .count();
    assert_eq!(zones, 2, "every opening lists the zones again");
}

fn apply_berlin(kept: lanlord_core::clock::Stored) -> (Desktop, egui::Context, Arc<Fake>) {
    let (mut desktop, ctx, fake) = setup();
    *fake.kept.lock().unwrap() = kept;
    until(&mut desktop, &ctx, |d| d.feed.has_reading());
    open(&mut desktop, &ctx);
    desktop.clock.pretend_zone("Europe/Berlin");
    frames(&mut desktop, &ctx, 2);
    assert!(press(&mut desktop, &ctx, "Apply"));
    until(&mut desktop, &ctx, idle("me@gw"));
    frames(&mut desktop, &ctx, 3);
    (desktop, ctx, fake)
}

#[test]
fn a_change_on_a_server_without_lanlord_is_done_and_says_it_is_not_stored() {
    let (desktop, _ctx, fake) = apply_berlin(lanlord_core::clock::Stored::NotInstalled);
    assert_eq!(fake.clock.lock().unwrap().timezone, "Europe/Berlin");
    let note = desktop.clock.note().unwrap();
    assert!(
        note.contains("LANlord is not installed on this server, so the change is not stored"),
        "{note}"
    );
    assert!(!note.contains("127"), "{note}");
}

#[test]
fn a_failure_to_keep_the_change_is_a_warning_and_the_window_still_refreshes() {
    let failed = lanlord_core::clock::Stored::Failed("storing the settings failed".into());
    let (desktop, _ctx, fake) = apply_berlin(failed);
    assert_eq!(fake.clock.lock().unwrap().timezone, "Europe/Berlin");
    let note = desktop.clock.note().unwrap();
    assert!(
        note.contains("applied, but could not be remembered in LANlord's settings: storing the settings failed"),
        "{note}"
    );
    assert!(desktop.clock.is_loaded(), "the window shows the new state");
}
