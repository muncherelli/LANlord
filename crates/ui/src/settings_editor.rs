//! the Settings window: its contents and behaviour live in `lanlord_app::settings_window`;
//! this draws them with egui and shows the masked box that asks for a secret.

use std::ops::{Deref, DerefMut};

use eframe::egui::{self, Ui};
use lanlord_app::services::Services;
use lanlord_app::settings_window::SettingsWindow;

use crate::dialogs::{Answer, TextDialog};
use crate::view;

#[derive(Default)]
pub struct SettingsEditor {
    window: SettingsWindow,
    /// the box asking for the secret the window wants, while it is open.
    secret_dialog: Option<TextDialog>,
}

impl Deref for SettingsEditor {
    type Target = SettingsWindow;

    fn deref(&self) -> &SettingsWindow {
        &self.window
    }
}

impl DerefMut for SettingsEditor {
    fn deref_mut(&mut self) -> &mut SettingsWindow {
        &mut self.window
    }
}

impl SettingsEditor {
    #[cfg(test)]
    pub fn secret_dialog_open(&self) -> bool {
        self.secret_dialog.is_some() || self.window.asking().is_some()
    }

    /// the masked box for a secret, drawn on top of everything.
    pub fn dialog(&mut self, ctx: &egui::Context) {
        let Some(path) = self.window.asking().map(str::to_string) else {
            self.secret_dialog = None;
            return;
        };
        let dialog = self.secret_dialog.get_or_insert_with(|| {
            let prompt = format!("New value for {path}\n(blank keeps the current one):");
            TextDialog::new("secret", "Set a secret", &prompt, true)
        });
        match dialog.show(ctx) {
            Some(Answer::Ok(value)) => {
                self.secret_dialog = None;
                self.window.answer_secret(Some(value));
            }
            Some(Answer::Cancel) => {
                self.secret_dialog = None;
                self.window.answer_secret(None);
            }
            None => {}
        }
    }

    /// draw the window's contents for the selected `host`.
    pub fn show(&mut self, ui: &mut Ui, services: &Services, host: Option<&str>) {
        let nodes = self.window.view(services, host);
        if let (Some(msg), Some(host)) = (view::show(ui, &nodes), host) {
            self.window.update(msg, services, host);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;

    #[test]
    fn the_secret_dialog_collects_a_value() {
        let mut editor = SettingsEditor::default();
        let ctx = egui::Context::default();
        let frame = |editor: &mut SettingsEditor, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::new(800.0, 600.0),
                )),
                events,
                ..Default::default()
            };
            ctx.run_ui(input, |ui| {
                theme::apply(ui.ctx());
                editor.dialog(ui.ctx());
            })
            .textures_delta
            .clear();
        };
        frame(&mut editor, Vec::new());
        editor.ask_secret("a.password");
        frame(&mut editor, Vec::new());
        frame(&mut editor, Vec::new());
        frame(&mut editor, vec![egui::Event::Text("s3".into())]);
        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut editor, vec![enter]);
        assert!(editor.pending_update("a.password"));
        assert!(!editor.secret_dialog_open());
    }
}
