//! a box with one line of text and OK / Cancel: the sudo password (masked), a new host, a
//! secret's new value.

use crate::view::{Button, Dialog, Node};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Ok(String),
    Cancel,
}

/// what a keystroke or a click in the box means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptMsg {
    /// the field now holds this text.
    Edit(String),
    Ok,
    Cancel,
}

pub struct Prompt {
    id: &'static str,
    title: String,
    prompt: String,
    input: String,
    masked: bool,
    /// shown under the text box, for input that was refused.
    pub error: Option<String>,
    focused: bool,
}

impl Prompt {
    pub fn new(id: &'static str, title: &str, prompt: &str, masked: bool) -> Self {
        Self {
            id,
            title: title.into(),
            prompt: prompt.into(),
            input: String::new(),
            masked,
            error: None,
            focused: false,
        }
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    /// the box; the field gets the cursor the first time it is drawn.
    pub fn view(&mut self) -> Dialog<PromptMsg> {
        let mut body = vec![
            Node::Text(self.prompt.clone()),
            Node::Entry {
                value: self.input.clone(),
                room: 10.0,
                masked: self.masked,
                focus: !self.focused,
                edit: Box::new(PromptMsg::Edit),
                enter: Some(PromptMsg::Ok),
            },
        ];
        self.focused = true;
        if let Some(error) = &self.error {
            body.push(Node::Warning(error.clone()));
        }
        body.push(Node::Gap(8.0));
        body.push(Node::Toolbar(vec![
            Button::new("OK", true, PromptMsg::Ok),
            Button::new("Cancel", true, PromptMsg::Cancel),
        ]));
        Dialog {
            id: self.id,
            title: self.title.clone(),
            body,
            escape: Some(PromptMsg::Cancel),
        }
    }

    /// act on a message; `Some` once the box was answered.
    pub fn update(&mut self, msg: PromptMsg) -> Option<Answer> {
        match msg {
            PromptMsg::Edit(text) => {
                self.input = text;
                None
            }
            PromptMsg::Ok => Some(Answer::Ok(self.input.clone())),
            PromptMsg::Cancel => Some(Answer::Cancel),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_then_ok_answers_with_the_text() {
        let mut prompt = Prompt::new("p", "Title", "Say it", true);
        assert_eq!(prompt.prompt(), "Say it");
        assert_eq!(prompt.update(PromptMsg::Edit("pw".into())), None);
        assert_eq!(prompt.update(PromptMsg::Ok), Some(Answer::Ok("pw".into())));
        assert_eq!(prompt.update(PromptMsg::Cancel), Some(Answer::Cancel));
    }

    #[test]
    fn the_field_is_masked_focused_once_and_enter_means_ok() {
        let mut prompt = Prompt::new("p", "Title", "Say it", true);
        let entry = |dialog: &Dialog<PromptMsg>| {
            dialog.body.iter().find_map(|n| match n {
                Node::Entry {
                    masked,
                    focus,
                    enter,
                    ..
                } => Some((*masked, *focus, enter.clone())),
                _ => None,
            })
        };
        assert_eq!(
            entry(&prompt.view()),
            Some((true, true, Some(PromptMsg::Ok)))
        );
        assert_eq!(entry(&prompt.view()).map(|e| e.1), Some(false));
    }
}
