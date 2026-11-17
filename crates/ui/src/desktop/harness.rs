//! shared helpers for the desktop's headless tests.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Pos2};

use super::*;
use crate::testkit;
pub use crate::testkit::{click, key, SCREEN};
use lanlord_app::gateway::Fake;

pub fn desktop() -> (Desktop, egui::Context, Arc<Fake>) {
    let ctx = egui::Context::default();
    let fake = Arc::new(Fake::new());
    let mut desktop = Desktop::new(fake.clone(), &ctx, None);
    desktop.pinned = Some(every_component());
    desktop.icons = lanlord_app::desktop::icons(desktop.pinned.as_ref());
    (desktop, ctx, fake)
}

/// every component switched on, so the windows of all of them can be tried.
pub fn every_component() -> lanlord_app::components::Selection {
    lanlord_app::components::Selection {
        enabled: lanlord_core::modules::MODULES
            .iter()
            .map(|m| (m.id.to_string(), Default::default()))
            .collect(),
    }
}

pub fn frame(desktop: &mut Desktop, ctx: &egui::Context, events: Vec<egui::Event>) {
    testkit::run(ctx, events, |ui| desktop.show(ui));
}

pub fn frames(desktop: &mut Desktop, ctx: &egui::Context, n: usize) {
    for _ in 0..n {
        frame(desktop, ctx, Vec::new());
    }
}

pub fn click_at(desktop: &mut Desktop, ctx: &egui::Context, at: Pos2) {
    for event in click(at) {
        frame(desktop, ctx, vec![event]);
    }
    frames(desktop, ctx, 2);
}

/// click the button labelled `text`; false when it is missing or disabled.
pub fn press(desktop: &mut Desktop, ctx: &egui::Context, text: &str) -> bool {
    // draw first, so the button reflects what the tasks did meanwhile
    frames(desktop, ctx, 2);
    match theme::probe::find(ctx, text) {
        Some((rect, true)) => {
            click_at(desktop, ctx, rect.center());
            true
        }
        _ => false,
    }
}

pub fn enabled(ctx: &egui::Context, text: &str) -> Option<bool> {
    theme::probe::find(ctx, text).map(|(_, on)| on)
}

pub fn type_text(desktop: &mut Desktop, ctx: &egui::Context, text: &str) {
    frame(desktop, ctx, vec![egui::Event::Text(text.into())]);
    frame(desktop, ctx, vec![key(egui::Key::Enter)]);
    frames(desktop, ctx, 2);
}

/// draw frames until `done` holds.
pub fn until(desktop: &mut Desktop, ctx: &egui::Context, done: impl Fn(&Desktop) -> bool) {
    let start = Instant::now();
    while !done(desktop) {
        assert!(start.elapsed() < Duration::from_secs(5), "timed out");
        frames(desktop, ctx, 1);
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub fn idle(host: &'static str) -> impl Fn(&Desktop) -> bool {
    move |d| !d.services.busy(host)
}

pub fn with_host(desktop: &mut Desktop, ctx: &egui::Context) {
    desktop.hosts.add("me@gw").unwrap();
    frames(desktop, ctx, 2);
}

pub fn screen() -> Rect {
    Rect::from_min_size(Pos2::ZERO, SCREEN)
}
