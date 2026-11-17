//! the server's clock: the taskbar time and the Date/Time Properties window live in
//! `lanlord_app::clock_window`; this draws the window with egui and keeps it ticking.

use std::ops::{Deref, DerefMut};
use std::time::Duration;

use eframe::egui::Ui;
use lanlord_app::services::Services;

pub use lanlord_app::clock_window::ClockFeed;

use crate::view;

#[derive(Default)]
pub struct ClockPanel(lanlord_app::clock_window::ClockPanel);

impl Deref for ClockPanel {
    type Target = lanlord_app::clock_window::ClockPanel;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ClockPanel {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl ClockPanel {
    /// draw the window's contents; true when it should be closed.
    pub fn show(&mut self, ui: &mut Ui, services: &Services, host: Option<&str>) -> bool {
        let nodes = self.0.view(services, host);
        if self.0.is_loaded() {
            // the clock face and the spinners follow the server's time
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
        match (view::show(ui, &nodes), host) {
            (Some(msg), Some(host)) => self.0.update(msg, services, host),
            _ => self.0.closing(),
        }
    }
}
