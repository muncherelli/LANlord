//! the native window, drawn with egui so it looks the same on every platform
//! (and, later, in a browser).

mod desktop;
mod icons;
mod theme;
mod windows;

use anyhow::{anyhow, Result};
use eframe::egui;

/// open the window and block until it is closed.
pub fn run() -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("LANlord")
            .with_inner_size([960.0, 640.0])
            .with_min_inner_size([640.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "LANlord",
        options,
        Box::new(|_cc| Ok(Box::new(desktop::Desktop::default()))),
    )
    .map_err(|err| anyhow!("could not open the window: {err}"))
}
