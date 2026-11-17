//! how a window draws what it knows about the gateway (the model is
//! `lanlord_app::remote::Remote`): the red used for errors and the status bar note.

use eframe::egui::{Color32, Ui};

pub use lanlord_app::remote::Remote;

pub const RED: Color32 = Color32::from_rgb(160, 0, 0);

/// draws a `Remote`'s note in a status bar.
pub trait ShowNote {
    fn show_note(&self, ui: &mut Ui);
}

impl<T> ShowNote for Remote<T> {
    fn show_note(&self, ui: &mut Ui) {
        match &self.note {
            Some((error, text)) => crate::controls::status_bar(ui, text, *error),
            None => crate::controls::status_bar(ui, "", false),
        }
    }
}
