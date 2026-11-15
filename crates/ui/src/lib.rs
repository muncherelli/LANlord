//! the native window, drawn with egui so it looks the same on every platform
//! (and, later, in a browser).

mod console;
mod desktop;
mod dialogs;
mod gateway;
mod hostlist;
mod icons;
mod services;
mod settings_editor;
mod tasks;
mod theme;
mod windows;

use std::sync::Arc;

use anyhow::{anyhow, Result};
use eframe::egui;
use lanlord_core::hosts;

/// open the window and block until it is closed.
pub fn run() -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("LANlord")
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
                    .map_err(|e| anyhow!("{e}"))?,
            )
            .with_inner_size([960.0, 640.0])
            .with_min_inner_size([640.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "LANlord",
        options,
        Box::new(|cc| {
            let services = services::Services::new(Arc::new(gateway::Ssh), &cc.egui_ctx);
            Ok(Box::new(desktop::Desktop::new(
                services,
                hosts::config_path(),
            )))
        }),
    )
    .map_err(|err| anyhow!("could not open the window: {err}"))
}
