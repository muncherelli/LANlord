//! a modal box with one line of text and OK / Cancel; used for the sudo
//! password (masked) and for adding a host.

use eframe::egui::{self, Color32, Frame, Id, Key, Margin, Rect, TextEdit, Vec2};

use crate::theme::{self, GRAY, NAVY, WHITE};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Ok(String),
    Cancel,
}

pub struct TextDialog {
    id: &'static str,
    title: String,
    prompt: String,
    input: String,
    masked: bool,
    /// shown under the text box, for input that was refused.
    pub error: Option<String>,
    /// where the OK and Cancel buttons were drawn last frame.
    buttons: Option<(Rect, Rect)>,
    focused: bool,
}

impl TextDialog {
    pub fn new(id: &'static str, title: &str, prompt: &str, masked: bool) -> Self {
        Self {
            id,
            title: title.into(),
            prompt: prompt.into(),
            input: String::new(),
            masked,
            error: None,
            buttons: None,
            focused: false,
        }
    }

    #[cfg(test)]
    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    /// draw the dialog; `Some` once the user answered it.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Answer> {
        let frame = Frame::new()
            .fill(GRAY)
            .inner_margin(Margin::same(4))
            .stroke(egui::Stroke::new(1.0, Color32::BLACK));
        let mut answer = None;
        egui::Modal::new(Id::unique(self.id))
            .frame(frame)
            .backdrop_color(Color32::from_black_alpha(60))
            .show(ctx, |ui| {
                ui.set_width(300.0);
                let (bar, _) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), 20.0),
                    egui::Sense::hover(),
                );
                ui.painter().rect_filled(bar, 0.0, NAVY);
                ui.painter().text(
                    bar.left_center() + Vec2::new(6.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    &self.title,
                    theme::font(),
                    WHITE,
                );
                ui.add_space(8.0);
                ui.label(&self.prompt);
                let edit = TextEdit::singleline(&mut self.input)
                    .password(self.masked)
                    .desired_width(ui.available_width());
                let edit = ui.add(edit);
                if !self.focused {
                    edit.request_focus();
                    self.focused = true;
                }
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if let Some(error) = &self.error {
                    ui.colored_label(Color32::from_rgb(160, 0, 0), error);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let ok = theme::button(ui, "OK", false);
                    let cancel = theme::button(ui, "Cancel", false);
                    self.buttons = Some((ok.rect, cancel.rect));
                    if ok.clicked() || enter {
                        answer = Some(Answer::Ok(self.input.clone()));
                    } else if cancel.clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                        answer = Some(Answer::Cancel);
                    }
                });
            });
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, Modifiers, PointerButton, Pos2, RawInput};

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
        let (ok, cancel) = dialog.buttons.unwrap();
        let mut answers = Vec::new();
        for events in click(cancel.center()).into_iter().map(|e| vec![e]) {
            answers.push(frame(&mut dialog, &ctx, events));
        }
        assert!(answers.contains(&Some(Answer::Cancel)));
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
