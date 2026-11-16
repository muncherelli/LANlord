//! the modal box frame every dialog is drawn in, and the one-line text box (the sudo password,
//! a new host, a secret), whose contents live in `lanlord_app::prompt`.

use std::ops::{Deref, DerefMut};

use eframe::egui::{self, Color32, Frame, Id, Margin, Vec2};

use crate::theme::{self, GRAY, NAVY, WHITE};

pub use lanlord_app::prompt::Answer;
use lanlord_app::prompt::Prompt;

/// the one-line box, drawn from `lanlord_app::prompt`.
pub struct TextDialog(Prompt);

impl Deref for TextDialog {
    type Target = Prompt;

    fn deref(&self) -> &Prompt {
        &self.0
    }
}

impl DerefMut for TextDialog {
    fn deref_mut(&mut self) -> &mut Prompt {
        &mut self.0
    }
}

impl TextDialog {
    pub fn new(id: &'static str, title: &str, prompt: &str, masked: bool) -> Self {
        Self(Prompt::new(id, title, prompt, masked))
    }

    /// draw the dialog; `Some` once the user answered it.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Answer> {
        let dialog = self.0.view();
        crate::view::dialog(ctx, &dialog).and_then(|msg| self.0.update(msg))
    }
}

/// a modal box with a navy title bar, drawn on top of everything.
pub fn modal<R>(
    ctx: &egui::Context,
    id: &'static str,
    title: &str,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let frame = Frame::new().fill(GRAY).inner_margin(Margin::same(3));
    let shown = egui::Modal::new(Id::unique(id))
        .frame(frame)
        .backdrop_color(Color32::from_black_alpha(60))
        .show(ctx, |ui| {
            ui.set_width(380.0);
            let (bar, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), egui::Sense::hover());
            ui.painter().rect_filled(bar, 0.0, NAVY);
            ui.painter().text(
                bar.left_center() + Vec2::new(6.0, 0.0),
                egui::Align2::LEFT_CENTER,
                title,
                theme::font(),
                WHITE,
            );
            ui.add_space(6.0);
            Frame::new()
                .inner_margin(Margin::symmetric(8, 2))
                .show(ui, body)
                .inner
        });
    let painter = ctx.layer_painter(shown.response.layer_id);
    theme::bevel(&painter, shown.response.rect, true);
    shown.inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect};

    fn frame(dialog: &mut TextDialog, ctx: &egui::Context, events: Vec<Event>) -> Option<Answer> {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut answer = None;
        ctx.run_ui(input, |ui| {
            theme::apply(ui.ctx());
            answer = dialog.show(ui.ctx());
        })
        .textures_delta
        .clear();
        answer
    }

    fn key(key: Key) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }
    }

    fn click(at: Pos2) -> Vec<Event> {
        let button = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        vec![Event::PointerMoved(at), button(true), button(false)]
    }

    fn open(masked: bool) -> (TextDialog, egui::Context) {
        let ctx = egui::Context::default();
        let mut dialog = TextDialog::new("test-dialog", "Title", "Say it", masked);
        frame(&mut dialog, &ctx, Vec::new());
        frame(&mut dialog, &ctx, Vec::new());
        (dialog, ctx)
    }

    #[test]
    fn typing_then_enter_answers_ok() {
        let (mut dialog, ctx) = open(true);
        assert_eq!(dialog.prompt(), "Say it");
        assert!(frame(&mut dialog, &ctx, vec![Event::Text("pw".into())]).is_none());
        let answer = frame(&mut dialog, &ctx, vec![key(Key::Enter)]);
        assert_eq!(answer, Some(Answer::Ok("pw".into())));
    }

    #[test]
    fn escape_cancels() {
        let (mut dialog, ctx) = open(false);
        assert_eq!(
            frame(&mut dialog, &ctx, vec![key(Key::Escape)]),
            Some(Answer::Cancel)
        );
    }

    #[test]
    fn the_buttons_answer() {
        let (mut dialog, ctx) = open(false);
        frame(&mut dialog, &ctx, vec![Event::Text("x".into())]);
        let (cancel, _) = theme::probe::find(&ctx, "Cancel").unwrap();
        let mut answers = Vec::new();
        for events in click(cancel.center()).into_iter().map(|e| vec![e]) {
            answers.push(frame(&mut dialog, &ctx, events));
        }
        assert!(answers.contains(&Some(Answer::Cancel)));
        let (ok, _) = theme::probe::find(&ctx, "OK").unwrap();
        let mut answers = Vec::new();
        for events in click(ok.center()).into_iter().map(|e| vec![e]) {
            answers.push(frame(&mut dialog, &ctx, events));
        }
        assert!(answers.contains(&Some(Answer::Ok("x".into()))));
    }

    #[test]
    fn errors_are_drawn() {
        let (mut dialog, ctx) = open(false);
        dialog.error = Some("nope".into());
        assert!(frame(&mut dialog, &ctx, Vec::new()).is_none());
    }
}
