//! how a window draws what it knows about the gateway (the model is
//! `lanlord_app::remote::Remote`): the red used for errors.

use eframe::egui::Color32;

pub const RED: Color32 = Color32::from_rgb(160, 0, 0);
