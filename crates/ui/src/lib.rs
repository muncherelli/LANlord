//! the native window, drawn with egui so it looks the same on every platform
//! (and, later, in a browser).

mod admin_setup;
mod command_panel;
mod console;
mod controls;
mod desktop;
mod dialogs;
mod dns_panel;
mod form;
mod gateway;
mod hostlist;
mod icons;
mod login;
mod logon_panel;
mod nt4_panel;
mod nt4_window;
mod remote;
mod services;
mod settings_editor;
mod tasks;
#[cfg(test)]
mod testkit;
mod theme;
mod users_panel;
mod widgets;
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
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([800.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "LANlord",
        options,
        Box::new(|cc| {
            let services = services::Services::new(Arc::new(gateway::Ssh), &cc.egui_ctx);
            let mut desktop = desktop::Desktop::new(services, hosts::config_path());
            // the logon box comes first; LANLORD_SCREEN=desktop skips it, to look at the windows
            if std::env::var("LANLORD_SCREEN").as_deref() != Ok("desktop") {
                desktop.require_login(Arc::new(login::SystemAuth::new()));
            }
            if let Ok(spec) = std::env::var("LANLORD_OPEN") {
                desktop.open_spec(&spec);
            }
            Ok(Box::new(desktop))
        }),
    )
    .map_err(|err| anyhow!("could not open the window: {err}"))
}
