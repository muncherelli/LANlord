//! the native window, drawn with egui so it looks the same on every platform.

use anyhow::{anyhow, Result};
use eframe::egui;

/// classic desktop teal.
const DESKTOP: egui::Color32 = egui::Color32::from_rgb(0x00, 0x80, 0x80);

struct App;

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Frame::new().fill(DESKTOP).show(ui, |ui| {
            ui.centered_and_justified(|ui| {
                ui.heading("LANlord");
            });
        });
    }
}

/// open the window and block until it is closed.
pub fn run() -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("LANlord")
            .with_inner_size([800.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native("LANlord", options, Box::new(|_cc| Ok(Box::new(App))))
        .map_err(|err| anyhow!("could not open the window: {err}"))
}
