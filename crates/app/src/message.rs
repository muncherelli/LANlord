//! a box with a warning, a message and a row of buttons; the last button is the way out.

use crate::view::{Button, Dialog, Node};

pub struct MessageBox {
    id: &'static str,
    title: String,
    text: String,
    buttons: Vec<String>,
}

impl MessageBox {
    pub fn new(id: &'static str, title: &str, text: &str, buttons: &[&str]) -> Self {
        Self {
            id,
            title: title.into(),
            text: text.into(),
            buttons: buttons.iter().map(ToString::to_string).collect(),
        }
    }

    /// the box; a click sends the number of its button, Escape the last one.
    pub fn view(&self) -> Dialog<usize> {
        // buttons line up from the right, so the first one is listed last to end up leftmost
        let buttons = self
            .buttons
            .iter()
            .enumerate()
            .rev()
            .map(|(n, label)| Button::new(label, true, n))
            .collect();
        Dialog {
            id: self.id,
            title: self.title.clone(),
            body: vec![
                Node::Alert(self.text.clone()),
                Node::Gap(8.0),
                Node::Buttons(buttons),
            ],
            escape: self.buttons.len().checked_sub(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buttons_send_their_number_and_escape_the_last() {
        let message = MessageBox::new("m", "Title", "Sure?", &["Yes", "No"]);
        let dialog = message.view();
        assert_eq!(dialog.escape, Some(1));
        let Node::Buttons(buttons) = &dialog.body[2] else {
            panic!("a row of buttons");
        };
        let pressed: Vec<(&str, usize)> = buttons
            .iter()
            .map(|b| (b.label.as_str(), b.press))
            .collect();
        assert_eq!(pressed, [("No", 1), ("Yes", 0)]);
    }
}
