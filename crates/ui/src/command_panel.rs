//! the remote commands window: the panel lives in `lanlord_app::command_panel`; this draws it
//! with egui.

use std::ops::{Deref, DerefMut};

use eframe::egui::Ui;
use lanlord_app::services::Services;

use crate::view;

#[derive(Default)]
pub struct CommandPanel(lanlord_app::command_panel::CommandPanel);

impl Deref for CommandPanel {
    type Target = lanlord_app::command_panel::CommandPanel;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for CommandPanel {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl CommandPanel {
    pub fn show(
        &mut self,
        ui: &mut Ui,
        services: &Services,
        host: Option<&str>,
        machines: &[String],
    ) {
        let nodes = self.0.view(services, host, machines);
        if let (Some(msg), Some(host)) = (view::show(ui, &nodes), host) {
            self.0.update(msg, services, host);
        }
    }
}
