//! the logon scripts tab: the panel lives in `lanlord_app::logon_panel`; this draws it and its
//! box with egui.

use std::ops::{Deref, DerefMut};

use eframe::egui::{self, Ui};
use lanlord_app::services::Services;

use crate::view;

#[derive(Default)]
pub struct LogonPanel(lanlord_app::logon_panel::LogonPanel);

impl Deref for LogonPanel {
    type Target = lanlord_app::logon_panel::LogonPanel;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for LogonPanel {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl LogonPanel {
    pub fn show(&mut self, ui: &mut Ui, services: &Services, host: Option<&str>) {
        // a page being measured is drawn unseen: it asks for nothing and answers nothing
        let measuring = !ui.is_visible();
        let nodes = if measuring {
            self.0.preview(services, host)
        } else {
            self.0.view(services, host)
        };
        if let (Some(msg), Some(host)) = (view::show(ui, &nodes), host) {
            if !measuring {
                self.0.update(msg, services, host);
            }
        }
    }

    /// the box that names a new script, drawn on top of everything.
    pub fn dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.0.dialog() else {
            return;
        };
        if let Some(msg) = view::dialog(ctx, &dialog) {
            self.0.update_dialog(msg);
        }
    }
}
