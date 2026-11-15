//! a modal box with several labelled fields, for adding a login, a lease or a
//! record. a secret field is masked, can be filled with a generated password
//! and can be shown while you copy it down.

use eframe::egui::{self, Color32, Key, TextEdit, Ui};
use lanlord_core::passwords;

use crate::dialogs::modal;
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormAnswer {
    Ok,
    Cancel,
}

pub struct FormField {
    pub label: String,
    pub value: String,
    secret: bool,
    shown: bool,
}

pub struct Form {
    id: &'static str,
    title: String,
    message: String,
    pub fields: Vec<FormField>,
    pub checks: Vec<(String, bool)>,
    /// shown under the fields, for input that was refused.
    pub error: Option<String>,
    focused: bool,
}

impl Form {
    pub fn new(id: &'static str, title: &str, message: &str) -> Self {
        Self {
            id,
            title: title.into(),
            message: message.into(),
            fields: Vec::new(),
            checks: Vec::new(),
            error: None,
            focused: false,
        }
    }

    pub fn text(mut self, label: &str, value: &str) -> Self {
        self.fields.push(FormField {
            label: label.into(),
            value: value.into(),
            secret: false,
            shown: false,
        });
        self
    }

    pub fn secret(mut self, label: &str) -> Self {
        self.fields.push(FormField {
            label: label.into(),
            value: String::new(),
            secret: true,
            shown: false,
        });
        self
    }

    pub fn check(mut self, label: &str, value: bool) -> Self {
        self.checks.push((label.into(), value));
        self
    }

    /// the trimmed text of field `i`.
    pub fn value(&self, i: usize) -> &str {
        self.fields.get(i).map_or("", |f| f.value.trim())
    }

    /// the field `i` exactly as typed, which matters for passwords.
    pub fn raw(&self, i: usize) -> &str {
        self.fields.get(i).map_or("", |f| f.value.as_str())
    }

    pub fn checked(&self, i: usize) -> bool {
        self.checks.get(i).is_some_and(|c| c.1)
    }

    fn field_row(&mut self, ui: &mut Ui, i: usize, first: bool) {
        let focus_wanted = first && !self.focused;
        let field = &mut self.fields[i];
        ui.label(&field.label);
        ui.horizontal(|ui| {
            let room = if field.secret { 150.0 } else { 0.0 };
            let edit = TextEdit::singleline(&mut field.value)
                .password(field.secret && !field.shown)
                .desired_width(ui.available_width() - room);
            let response = ui.add(edit);
            if focus_wanted {
                response.request_focus();
            }
            if field.secret {
                if theme::action(ui, "Generate", true).clicked() {
                    field.value = passwords::generate(passwords::DEFAULT_LEN);
                    field.shown = true;
                }
                let label = if field.shown { "Hide" } else { "Show" };
                if theme::action(ui, label, true).clicked() {
                    field.shown = !field.shown;
                }
            }
        });
    }

    /// draw the form; `Some` once the user answered it.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<FormAnswer> {
        let mut answer = None;
        let (id, title) = (self.id, self.title.clone());
        modal(ctx, id, &title, |ui| {
            if !self.message.is_empty() {
                ui.label(&self.message);
            }
            for i in 0..self.fields.len() {
                self.field_row(ui, i, i == 0);
            }
            self.focused = true;
            for (label, value) in &mut self.checks {
                ui.checkbox(value, label.as_str());
            }
            if let Some(error) = &self.error {
                ui.colored_label(Color32::from_rgb(160, 0, 0), error);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let ok = theme::button(ui, "OK", false);
                let cancel = theme::button(ui, "Cancel", false);
                if ok.clicked() {
                    answer = Some(FormAnswer::Ok);
                } else if cancel.clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    answer = Some(FormAnswer::Cancel);
                }
            });
        });
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit;

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
    fn values_are_trimmed_but_raw_keeps_spaces() {
        let (form, _) = open();
        assert_eq!((form.value(0), form.raw(0)), ("bob", " bob "));
        assert_eq!(form.value(9), "");
        assert!(form.checked(0) && !form.checked(3));
    }

    #[test]
    fn generate_fills_and_shows_the_password_and_show_toggles() {
        let (mut form, ctx) = open();
        assert!(press(&mut form, &ctx, "Generate").is_none());
        assert_eq!(form.raw(1).len(), 12);
        assert!(form.fields[1].shown);
        press(&mut form, &ctx, "Hide");
        assert!(!form.fields[1].shown);
        press(&mut form, &ctx, "Show");
        assert!(form.fields[1].shown);
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
