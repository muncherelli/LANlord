//! the dns & dhcp window: the panel lives in `lanlord_app::dns_panel`; this draws it and its
//! box with egui.

use std::ops::{Deref, DerefMut};

use eframe::egui::{self, Ui};
use lanlord_app::services::Services;

use crate::view;

#[derive(Default)]
pub struct DnsPanel(lanlord_app::dns_panel::DnsPanel);

impl Deref for DnsPanel {
    type Target = lanlord_app::dns_panel::DnsPanel;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for DnsPanel {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl DnsPanel {
    pub fn show(&mut self, ui: &mut Ui, services: &Services, host: Option<&str>) {
        let nodes = self.0.view(services, host);
        if let (Some(msg), Some(host)) = (view::show(ui, &nodes), host) {
            self.0.update(msg, services, host);
        }
    }

    /// the box for a lease or a record, drawn on top of everything.
    pub fn dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.0.dialog() else {
            return;
        };
        if let Some(msg) = view::dialog(ctx, &dialog) {
            self.0.update_dialog(msg);
        }
    }
}
