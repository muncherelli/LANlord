//! the form box lives in `lanlord_app::form`; this draws it with egui.

pub use lanlord_app::form::{Form, FormAnswer};

use eframe::egui;

use crate::view;

/// drawing a form as a modal box.
pub trait ShowForm {
    /// draw the form; `Some` once the user answered it.
    fn show(&mut self, ctx: &egui::Context) -> Option<FormAnswer>;
}

impl ShowForm for Form {
    fn show(&mut self, ctx: &egui::Context) -> Option<FormAnswer> {
        let dialog = self.view();
        view::dialog(ctx, &dialog).and_then(|msg| self.update(msg))
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui::Key;

    use super::*;
    use crate::{testkit, theme};

    fn show(form: &mut Form, ctx: &egui::Context, events: Vec<egui::Event>) -> Option<FormAnswer> {
        let mut answer = None;
        testkit::run(ctx, events, |ui| {
            theme::apply(ui.ctx());
            answer = form.show(ui.ctx());
        });
        answer
    }

    fn press(form: &mut Form, ctx: &egui::Context, label: &str) -> Option<FormAnswer> {
        for _ in 0..3 {
            show(form, ctx, Vec::new());
        }
        let (rect, _) = theme::probe::find(ctx, label).expect(label);
        let mut result = None;
        for event in testkit::click(rect.center()) {
            result = result.or(show(form, ctx, vec![event]));
        }
        result
    }

    fn open() -> (Form, egui::Context) {
        let form = Form::new("test-form", "A form", "fill it")
            .text("Name", " bob ")
            .secret("Password")
            .check("Admin", true);
        (form, egui::Context::default())
    }

    #[test]
    fn generate_fills_and_shows_the_password_and_show_toggles() {
        let (mut form, ctx) = open();
        assert!(press(&mut form, &ctx, "Generate").is_none());
        assert_eq!(form.raw(1).len(), 12);
        assert!(form.shown(1));
        press(&mut form, &ctx, "Hide");
        assert!(!form.shown(1));
        press(&mut form, &ctx, "Show");
        assert!(form.shown(1));
    }

    #[test]
    fn ok_and_cancel_answer() {
        let (mut form, ctx) = open();
        form.error = Some("nope".into());
        assert_eq!(press(&mut form, &ctx, "OK"), Some(FormAnswer::Ok));
        assert_eq!(press(&mut form, &ctx, "Cancel"), Some(FormAnswer::Cancel));
        let (mut form, ctx) = open();
        show(&mut form, &ctx, Vec::new());
        let escape = testkit::key(Key::Escape);
        assert_eq!(
            show(&mut form, &ctx, vec![escape]),
            Some(FormAnswer::Cancel)
        );
    }
}
