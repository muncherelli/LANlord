//! a modal box with several labelled fields, for adding a login, a lease or a record. a secret
//! field is masked, can be filled with a generated password and can be shown while you copy it
//! down.

use lanlord_core::passwords;

use crate::view::{Button, Dialog, Node};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormAnswer {
    Ok,
    Cancel,
}

/// what a click or a keystroke in the box means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormMsg {
    /// field `0` now holds this text.
    Edit(usize, String),
    Generate(usize),
    /// show or hide what was typed in a secret field.
    Reveal(usize),
    /// flip tick box `0`.
    Check(usize),
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

    /// is secret field `i` shown in clear?
    pub fn shown(&self, i: usize) -> bool {
        self.fields.get(i).is_some_and(|f| f.shown)
    }

    /// the box; the first field gets the cursor the first time it is drawn.
    pub fn view(&mut self) -> Dialog<FormMsg> {
        let mut body = Vec::new();
        if !self.message.is_empty() {
            body.push(Node::Text(self.message.clone()));
        }
        for (i, field) in self.fields.iter().enumerate() {
            body.push(Node::Text(field.label.clone()));
            let mut row = vec![Node::Entry {
                value: field.value.clone(),
                room: if field.secret { 176.0 } else { 10.0 },
                masked: field.secret && !field.shown,
                focus: i == 0 && !self.focused,
                hint: String::new(),
                edit: Box::new(move |text| FormMsg::Edit(i, text)),
                enter: None,
            }];
            if field.secret {
                let reveal = if field.shown { "Hide" } else { "Show" };
                row.push(Node::Button(Button::new(
                    "Generate",
                    true,
                    FormMsg::Generate(i),
                )));
                row.push(Node::Button(Button::new(reveal, true, FormMsg::Reveal(i))));
            }
            body.push(Node::Row(row));
        }
        self.focused = true;
        for (i, (label, checked)) in self.checks.iter().enumerate() {
            body.push(Node::Check {
                label: label.clone(),
                checked: *checked,
                enabled: true,
                toggle: FormMsg::Check(i),
            });
        }
        if let Some(error) = &self.error {
            body.push(Node::Warning(error.clone()));
        }
        body.push(Node::Gap(8.0));
        body.push(Node::Buttons(vec![
            Button::new("Cancel", true, FormMsg::Cancel),
            Button::new("OK", true, FormMsg::Ok),
        ]));
        Dialog {
            id: self.id,
            title: self.title.clone(),
            body,
            escape: Some(FormMsg::Cancel),
        }
    }

    /// act on a message; `Some` once the box was answered.
    pub fn update(&mut self, msg: FormMsg) -> Option<FormAnswer> {
        match msg {
            FormMsg::Edit(i, text) => {
                if let Some(field) = self.fields.get_mut(i) {
                    field.value = text;
                }
            }
            FormMsg::Generate(i) => {
                if let Some(field) = self.fields.get_mut(i) {
                    field.value = passwords::generate(passwords::DEFAULT_LEN);
                    field.shown = true;
                }
            }
            FormMsg::Reveal(i) => {
                if let Some(field) = self.fields.get_mut(i) {
                    field.shown = !field.shown;
                }
            }
            FormMsg::Check(i) => {
                if let Some(check) = self.checks.get_mut(i) {
                    check.1 = !check.1;
                }
            }
            FormMsg::Ok => return Some(FormAnswer::Ok),
            FormMsg::Cancel => return Some(FormAnswer::Cancel),
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open() -> Form {
        Form::new("test-form", "A form", "fill it")
            .text("Name", " bob ")
            .secret("Password")
            .check("Admin", true)
    }

    #[test]
    fn values_are_trimmed_but_raw_keeps_spaces() {
        let form = open();
        assert_eq!((form.value(0), form.raw(0)), ("bob", " bob "));
        assert_eq!(form.value(9), "");
        assert!(form.checked(0) && !form.checked(3));
    }

    #[test]
    fn only_the_first_field_asks_for_the_cursor_and_only_once() {
        let mut form = open();
        let entries = |dialog: &Dialog<FormMsg>| -> Vec<bool> {
            dialog
                .body
                .iter()
                .filter_map(|n| match n {
                    Node::Row(row) => row.iter().find_map(|n| match n {
                        Node::Entry { focus, .. } => Some(*focus),
                        _ => None,
                    }),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(entries(&form.view()), [true, false]);
        assert_eq!(entries(&form.view()), [false, false]);
    }

    #[test]
    fn messages_edit_generate_reveal_tick_and_answer() {
        let mut form = open();
        assert_eq!(form.update(FormMsg::Edit(0, "ann".into())), None);
        assert_eq!(form.value(0), "ann");
        form.update(FormMsg::Generate(1));
        assert_eq!(form.raw(1).len(), passwords::DEFAULT_LEN);
        assert!(form.shown(1));
        form.update(FormMsg::Reveal(1));
        assert!(!form.shown(1));
        form.update(FormMsg::Check(0));
        assert!(!form.checked(0));
        assert_eq!(form.update(FormMsg::Ok), Some(FormAnswer::Ok));
        assert_eq!(form.update(FormMsg::Cancel), Some(FormAnswer::Cancel));
    }
}
