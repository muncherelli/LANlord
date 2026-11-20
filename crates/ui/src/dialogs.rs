//! the modal box frame every dialog is drawn in, and the one-line text box (the sudo password,
//! a new host, a secret), whose contents live in `lanlord_app::prompt`.

use std::ops::{Deref, DerefMut};

use eframe::egui::{self, Color32, Frame, Id, Margin, Pos2, Rect, Vec2};

use crate::theme::{self, GRAY, NAVY, WHITE};
use crate::windows;

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

/// a modal box with a navy title bar and a close button, drawn on top of everything. the bar
/// spans the box whatever its contents do, so it is drawn once they are laid out. gives back what
/// `body` did and whether the close button was clicked; a box that `closable` is not has none.
pub fn modal<R>(
    ctx: &egui::Context,
    id: &'static str,
    title: &str,
    closable: bool,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> (R, bool) {
    let frame = Frame::new().fill(GRAY).inner_margin(Margin::same(3));
    let mut closed = false;
    let mut rects = [Rect::NOTHING; 2];
    let shown = egui::Modal::new(Id::unique(id))
        .frame(frame)
        .backdrop_color(Color32::from_black_alpha(60))
        .show(ctx, |ui| {
            ui.set_width(380.0);
            #[cfg(test)]
            ui.ctx()
                .data_mut(|d| d.insert_temp(Id::unique("text-boxes"), Vec::<(Rect, f32)>::new()));
            let top = ui.cursor().min;
            ui.allocate_space(Vec2::new(0.0, 20.0));
            ui.add_space(6.0);
            let framed = Frame::new()
                .inner_margin(Margin::symmetric(8, 2))
                .show(ui, body);
            let bar = Rect::from_min_max(top, Pos2::new(ui.min_rect().right(), top.y + 20.0));
            ui.painter().rect_filled(bar, 0.0, NAVY);
            ui.painter().text(
                bar.left_center() + Vec2::new(6.0, 0.0),
                egui::Align2::LEFT_CENTER,
                title,
                theme::font(),
                WHITE,
            );
            closed = closable && windows::dialog_close(ui, bar, Id::unique((id, "close")));
            rects = [bar, framed.response.rect];
            framed.inner
        });
    let painter = ctx.layer_painter(shown.response.layer_id);
    theme::bevel(&painter, shown.response.rect, true);
    #[cfg(test)]
    ctx.data_mut(|d| {
        d.insert_temp(
            Id::unique(("modal-rects", id)),
            [rects[0], shown.response.rect, rects[1]],
        )
    });
    (shown.inner, closed)
}

/// the rects text boxes took since the last modal box began its contents, as laid out and not as
/// clipped.
#[cfg(test)]
pub(crate) fn boxes(ctx: &egui::Context) -> Vec<(Rect, f32)> {
    ctx.data(|d| d.get_temp(Id::unique("text-boxes")).unwrap_or_default())
}

#[cfg(test)]
pub(crate) fn note_box(ctx: &egui::Context, rect: Rect, wanted: f32) {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<(Rect, f32)>>(Id::unique("text-boxes"))
            .push((rect, wanted))
    });
}

/// where the last modal box with `id` put its title bar, its whole box and its contents.
#[cfg(test)]
pub(crate) fn modal_rects(ctx: &egui::Context, id: &'static str) -> Option<[Rect; 3]> {
    ctx.data(|d| d.get_temp(Id::unique(("modal-rects", id))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect};
    use zeroize::Zeroizing;

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
        assert_eq!(answer, Some(Answer::Ok(Zeroizing::new("pw".into()))));
    }

    #[test]
    fn text_typed_in_the_same_frame_as_enter_is_kept() {
        let (mut dialog, ctx) = open(true);
        let answer = frame(
            &mut dialog,
            &ctx,
            vec![Event::Text("pass".into()), key(Key::Enter)],
        );
        assert_eq!(answer, Some(Answer::Ok(Zeroizing::new("pass".into()))));
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
        assert!(answers.contains(&Some(Answer::Ok(Zeroizing::new("x".into())))));
    }

    #[test]
    fn errors_are_drawn() {
        let (mut dialog, ctx) = open(false);
        dialog.error = Some("nope".into());
        assert!(frame(&mut dialog, &ctx, Vec::new()).is_none());
    }

    /// one frame of a box whose contents are `wide` across, clicking at `at` if given.
    fn modal_frame(ctx: &egui::Context, wide: f32, events: Vec<Event>) -> bool {
        modal_frame_with(ctx, wide, true, events)
    }

    fn modal_frame_with(
        ctx: &egui::Context,
        wide: f32,
        closable: bool,
        events: Vec<Event>,
    ) -> bool {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut closed = false;
        ctx.run_ui(input, |ui| {
            theme::apply(ui.ctx());
            closed = modal(ui.ctx(), "wide", "Wide", closable, |ui| {
                ui.allocate_space(Vec2::new(wide, 20.0));
            })
            .1;
        })
        .textures_delta
        .clear();
        closed
    }

    #[test]
    fn the_title_bar_spans_a_box_wider_than_it_opens_and_its_close_button_ends_it() {
        let ctx = egui::Context::default();
        for _ in 0..3 {
            modal_frame(&ctx, 500.0, Vec::new());
        }
        let [bar, outer, body] = modal_rects(&ctx, "wide").expect("drawn");
        assert_eq!(bar.left(), outer.left() + 3.0);
        assert_eq!(bar.right(), outer.right() - 3.0);
        assert!(body.right() <= bar.right());
        // the close button is drawn at the bar's right end and answers a click there
        let at = Pos2::new(bar.right() - 2.0 - 8.0, bar.top() + 3.0 + 7.0);
        let mut closed = false;
        for events in click(at).into_iter().map(|e| vec![e]) {
            closed |= modal_frame(&ctx, 500.0, events);
        }
        assert!(closed);
    }

    #[test]
    fn a_box_that_cannot_be_closed_has_no_close_button_to_click() {
        let ctx = egui::Context::default();
        for _ in 0..3 {
            modal_frame_with(&ctx, 500.0, false, Vec::new());
        }
        let [bar, ..] = modal_rects(&ctx, "wide").expect("drawn");
        let at = Pos2::new(bar.right() - 2.0 - 8.0, bar.top() + 3.0 + 7.0);
        let mut closed = false;
        for events in click(at).into_iter().map(|e| vec![e]) {
            closed |= modal_frame_with(&ctx, 500.0, false, events);
        }
        assert!(!closed);
    }
}
