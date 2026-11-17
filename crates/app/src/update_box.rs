//! the box listing what is new in an update, and what its buttons do in each state of the
//! updater.

use lanlord_model::update::{Pause, Source, State, Update};

use crate::view::{Button, Dialog, Node};

/// what the box was answered with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Install,
    Later,
    Pause,
    Resume,
    Cancel,
    Restart,
}

/// what a click in the box means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Msg {
    Choose(Choice),
    ToggleAutomatic,
}

/// the buttons the box shows in `state`; the first is the main one.
pub fn buttons_for(state: &State) -> Vec<Choice> {
    match state {
        State::Idle | State::Available => vec![Choice::Install, Choice::Later],
        State::Downloading => vec![Choice::Pause, Choice::Cancel],
        State::Installing => vec![Choice::Cancel],
        State::Paused(_) => vec![Choice::Resume, Choice::Cancel],
        State::Failed(_) => vec![Choice::Install, Choice::Cancel],
        State::Done => vec![Choice::Restart, Choice::Later],
    }
}

pub fn label(choice: Choice, state: &State) -> &'static str {
    match choice {
        Choice::Install if matches!(state, State::Failed(_)) => "Try again",
        Choice::Install => "Install",
        Choice::Later => "Remind me later",
        Choice::Pause => "Pause",
        Choice::Resume => "Resume",
        Choice::Cancel if matches!(state, State::Failed(_)) => "Dismiss",
        Choice::Cancel => "Cancel",
        Choice::Restart => "Restart now",
    }
}

/// the line in the box that says what is going on.
pub fn status_text(state: &State) -> String {
    match state {
        State::Idle | State::Available => String::new(),
        State::Downloading => "Downloading the update...".into(),
        State::Installing => "Installing the update...".into(),
        State::Paused(Pause::Manual) => "Paused.".into(),
        State::Paused(Pause::Offline { .. }) => {
            "Waiting for the network; it tries again by itself.".into()
        }
        State::Failed(why) => format!("The update failed.\n{why}"),
        State::Done => "Installed. Restart LANlord to use the new version.".into(),
    }
}

/// the box for `update` while the updater is in `state`; `automatic` is the check box.
pub fn view(update: &Update, state: &State, automatic: bool) -> Dialog<Msg> {
    let mut lines: Vec<Node<Msg>> = update.lines.iter().cloned().map(Node::Text).collect();
    if lines.is_empty() {
        lines.push(Node::Text("(no details available)".into()));
    }
    let mut body = vec![
        Node::Row(vec![
            Node::Icon("update"),
            Node::Heading(update.headline.clone()),
        ]),
        Node::Text("The following updates are available:".into()),
        Node::Scroll {
            id: "update-list",
            max_height: 140.0,
            body: lines,
        },
        Node::Gap(4.0),
        Node::Text(
            match update.source {
                Source::Git => {
                    "Install updates LANlord from trunk, builds it, and then asks to restart."
                }
                Source::Release { .. } => "Install opens the download page.",
            }
            .into(),
        ),
    ];
    let status = status_text(state);
    if !status.is_empty() {
        body.push(Node::Text(status));
    }
    body.push(Node::Check {
        label: "Check for updates automatically".into(),
        checked: automatic,
        enabled: true,
        toggle: Msg::ToggleAutomatic,
    });
    body.push(Node::Gap(6.0));
    // buttons line up from the right, so the main one is listed last to end up leftmost
    body.push(Node::Buttons(
        buttons_for(state)
            .into_iter()
            .rev()
            .map(|c| Button::new(label(c, state), true, Msg::Choose(c)))
            .collect(),
    ));
    Dialog {
        id: "update-box",
        title: "LANlord Update".into(),
        body,
        escape: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_main_button_comes_first_and_says_what_it_does() {
        let update = Update {
            source: Source::Git,
            headline: "LANlord 0.2.0".into(),
            lines: Vec::new(),
        };
        let dialog = view(&update, &State::Failed("boom".into()), true);
        let Some(Node::Buttons(buttons)) = dialog.body.last() else {
            panic!("buttons last");
        };
        let labels: Vec<&str> = buttons.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(labels, ["Dismiss", "Try again"]);
        assert!(dialog
            .body
            .iter()
            .any(|n| matches!(n, Node::Text(t) if t.starts_with("The update failed"))));
    }
}
