//! the My Server window: its contents and behaviour live in `lanlord_app::my_server`; this only
//! draws them with egui.

use eframe::egui::{self, Ui};
pub use lanlord_app::my_server::{Msg, MyServer, ADVANCED, COMPATIBILITY, GENERAL, SSH};
use lanlord_app::services::Services;

use crate::view;

/// draw the window for `host`.
pub fn show(window: &mut MyServer, ui: &mut Ui, services: &Services, host: Option<&str>) {
    let Some(host) = host else {
        ui.label("No server yet. Sign in to one first.");
        return;
    };
    window.refresh(services, host);
    if let Some(msg) = view::show(ui, &window.view(!services.busy(host))) {
        window.update(msg, services, host);
    }
}

/// draw the box over everything while a purge or an ssh question is being asked or run.
pub fn dialog(window: &mut MyServer, ctx: &egui::Context, services: &Services) {
    window.refresh_purge();
    let Some(dialog) = window.dialog(services) else {
        return;
    };
    let host = window.dialog_host();
    if let Some(msg) = view::dialog(ctx, &dialog) {
        window.update(msg, services, &host);
    }
}
