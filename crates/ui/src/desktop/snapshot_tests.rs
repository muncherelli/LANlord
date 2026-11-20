//! image snapshots of today's nt4 look, compared with zero tolerance, so a refactor that moves one
//! pixel fails. the goldens live in `crates/ui/tests/snapshots` and are made on linux x86_64 with a
//! software vulkan adapter (lavapipe), without the devtools feature, so the module is compiled
//! there only.
//!
//! to regenerate them after a deliberate change to the look, run
//! `UPDATE_SNAPSHOTS=1 cargo test -p lanlord-ui snapshot_tests` and review the png diff.

use eframe::egui::Vec2;
use egui_kittest::{Harness, SnapshotOptions, SnapshotResults};
use std::time::{Duration, Instant};

use super::{Desktop, Target};

const SIZE: Vec2 = Vec2::new(760.0, 480.0);

/// a desktop on the fake gateway and one fake server, whose clock the taskbar shows, so the pixels
/// do not depend on the time of day. `setup` opens what the fixture shows.
fn shot(name: &str, setup: impl Fn(&mut Desktop)) {
    let mut results = SnapshotResults::new();
    for ppp in [1.0, 2.0] {
        let mut harness = Harness::builder()
            .with_size(SIZE)
            .with_pixels_per_point(ppp)
            .wgpu()
            .build_ui_state(
                |ui, desktop: &mut Option<Desktop>| {
                    if let Some(desktop) = desktop {
                        desktop.show(ui);
                    }
                },
                None,
            );
        let mut desktop = super::harness::desktop().0;
        desktop.ctx = harness.ctx.clone();
        super::theme::apply(&harness.ctx);
        desktop.hosts.add("me@gw").unwrap();
        *harness.state_mut() = Some(desktop);
        setup(harness.state_mut().as_mut().unwrap());
        // the fake server answers at once, but on another thread: wait for it to go quiet
        let start = Instant::now();
        while {
            harness.step();
            let d = harness.state().as_ref().unwrap();
            !d.feed.has_reading() || d.services.busy("me@gw")
        } {
            assert!(start.elapsed() < Duration::from_secs(5), "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
        for _ in 0..6 {
            harness.step();
        }
        harness.snapshot_options(
            format!("{name}@{ppp}x"),
            &SnapshotOptions::new().threshold(0.0).max_failed_pixels(0),
        );
        results.extend_harness(&mut harness);
    }
    results.unwrap();
}

#[test]
fn a_window_active() {
    shot("window_active", |d| d.open(Target::Hosts));
}

#[test]
fn a_window_inactive() {
    shot("window_inactive", |d| {
        d.open(Target::Hosts);
        d.active = None;
    });
}

#[test]
fn a_fixed_dialog() {
    shot("dialog", |d| d.open(Target::Components));
}

#[test]
fn a_property_sheet_with_tabs() {
    shot("property_sheet", |d| d.open_spec("nt4-domain@1"));
}

#[test]
fn the_desktop_with_the_start_menu() {
    shot("desktop_start_menu", |d| d.start_open = true);
}

#[test]
fn the_settings_window() {
    shot("settings", |d| d.open(Target::Settings));
}
