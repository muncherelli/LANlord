//! the LANlord Components window: its pages and behaviour live in `lanlord_app::components`; this
//! only draws them with egui.

use eframe::egui::{self, Ui};
pub use lanlord_app::components::Components;
use lanlord_app::services::Services;

use crate::view;

/// draw the window for `host`.
pub fn show(window: &mut Components, ui: &mut Ui, services: &Services, host: Option<&str>) {
    let Some(host) = host else {
        ui.label("No server yet. Sign in to one first.");
        return;
    };
    if let Some(msg) = view::show(ui, &window.view(!services.busy(host))) {
        window.update(msg, services, host);
    }
}

/// draw the Details box over everything while it is open.
pub fn dialog(
    window: &mut Components,
    ctx: &egui::Context,
    services: &Services,
    host: Option<&str>,
) {
    let (Some(dialog), Some(host)) = (window.dialog(), host) else {
        return;
    };
    if let Some(msg) = view::dialog(ctx, &dialog) {
        window.update(msg, services, host);
    }
}
