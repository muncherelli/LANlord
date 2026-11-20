//! the nt4 domain's settings tab: the panel lives in `lanlord_app::nt4_panel`; this draws it
//! and its box with egui.

use std::ops::{Deref, DerefMut};

use eframe::egui::{self, Ui};
use lanlord_app::services::Services;

use crate::view;

#[derive(Default)]
pub struct Nt4Panel(lanlord_app::nt4_panel::Nt4Panel);

impl Deref for Nt4Panel {
    type Target = lanlord_app::nt4_panel::Nt4Panel;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Nt4Panel {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Nt4Panel {
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

    /// the box for a machine name or the administrator password, drawn on top of everything.
    pub fn dialog(&mut self, ctx: &egui::Context, services: &Services, host: Option<&str>) {
        let Some(dialog) = self.0.dialog() else {
            return;
        };
        if let Some(msg) = view::dialog(ctx, &dialog) {
            self.0.update_dialog(msg, services, host);
        }
    }
}
