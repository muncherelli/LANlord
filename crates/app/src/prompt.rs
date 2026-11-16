//! a box with one line of text and OK / Cancel: the sudo password (masked), a new host, a
//! secret's new value.

use zeroize::Zeroizing;

use crate::view::{Button, Dialog, Node, Size};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Ok(String),
    Cancel,
}

/// what a keystroke or a click in the box means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptMsg {
    /// the field now holds this text.
    Edit(Zeroizing<String>),
    /// Enter with this text in the field.
    Submit(Zeroizing<String>),
    Ok,
    Cancel,
}

pub struct Prompt {
    id: &'static str,
    title: String,
    prompt: String,
    /// zeroized when the box goes, since it may hold a password.
    input: Zeroizing<String>,
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
            input: Zeroizing::new(String::new()),
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
            if self.masked {
                Node::Secret {
                    value: self.input.clone(),
                    size: Size::FillLess(10.0),
                    shown: false,
                    focus: !self.focused,
                    edit: Box::new(PromptMsg::Edit),
                    enter: Some(Box::new(PromptMsg::Submit)),
                }
            } else {
                Node::Input {
                    value: self.input.to_string(),
                    size: Size::FillLess(10.0),
                    focus: !self.focused,
                    hint: String::new(),
                    edit: Box::new(|text| PromptMsg::Edit(Zeroizing::new(text))),
                    enter: Some(Box::new(|text| PromptMsg::Submit(Zeroizing::new(text)))),
                }
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
            PromptMsg::Submit(text) => {
                self.input = text;
                Some(Answer::Ok(self.input.to_string()))
            }
            PromptMsg::Ok => Some(Answer::Ok(self.input.to_string())),
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
        assert_eq!(
            prompt.update(PromptMsg::Edit(Zeroizing::new("pw".into()))),
            None
        );
        assert_eq!(prompt.update(PromptMsg::Ok), Some(Answer::Ok("pw".into())));
        assert_eq!(prompt.update(PromptMsg::Cancel), Some(Answer::Cancel));
    }

    #[test]
    fn the_field_is_masked_focused_once_and_enter_sends_the_final_text() {
        let mut prompt = Prompt::new("p", "Title", "Say it", true);
        let field = |dialog: &Dialog<PromptMsg>| {
            dialog.body.iter().find_map(|n| match n {
                Node::Secret { focus, enter, .. } => Some((*focus, enter.is_some())),
                _ => None,
            })
        };
        assert_eq!(field(&prompt.view()), Some((true, true)));
        assert_eq!(field(&prompt.view()), Some((false, true)));
        // text typed in the same frame as Enter arrives with it
        let answer = prompt.update(PromptMsg::Submit(Zeroizing::new("late".into())));
        assert_eq!(answer, Some(Answer::Ok("late".into())));
    }
}
