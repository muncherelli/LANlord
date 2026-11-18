//! the layout language every front end draws. a window describes its contents as a tree of
//! `Node`s built from its state; egui (on the desktop and in the browser) and the tui each
//! interpret the tree in their own way and hand back the message of whatever was clicked or
//! typed, which goes to the window's `update`.
//!
//! sizes are layout units: one unit is one egui point at 100%. the tui divides them by the
//! size of a character cell.
//!
//! windows move to this tree one by one; the variants grow with them.

pub use crate::log::Kind;
use chrono::NaiveDate;
use lanlord_model::settings::ValidationError;
use zeroize::Zeroizing;

/// a piece of a window, carrying the message type `M` of the window it belongs to.
pub enum Node<M> {
    /// a line of plain text.
    Text(String),
    /// empty space after the previous node: below it in a column, beside it in a `Row`. a
    /// spacing hint; a front end with coarse units (the tui) may round it away.
    Gap(f32),
    /// a captioned frame around other nodes.
    Group { title: String, body: Vec<Node<M>> },
    /// a list with columns; clicking a row sends `select(row)`.
    Table(Table<M>),
    /// a row of buttons, right aligned, first one rightmost.
    Buttons(Vec<Button<M>>),
    /// a read-only "label: value" line.
    Field { label: String, value: String },
    /// the status bar at the bottom of a window; red when `error`.
    Status { text: String, error: bool },
    /// a line of bold text.
    Heading(String),
    /// a list item: a bullet, and the text wrapped under itself rather than under the bullet.
    Bullet(String),
    /// one button on its own line, left aligned.
    Button(Button<M>),
    /// a row of tabs over the page of the `current` one; clicking a tab sends `pick(tab)`.
    Tabs {
        titles: Vec<String>,
        current: usize,
        pick: fn(usize) -> M,
        body: Vec<Node<M>>,
    },
    /// a red border around things that cannot be undone.
    Danger(Vec<Node<M>>),
    /// a tick box; clicking it sends `toggle`.
    Check {
        label: String,
        checked: bool,
        enabled: bool,
        toggle: M,
    },
    /// a one-line text field; `focus` puts the cursor in it. each change sends `edit(new text)`
    /// and Enter sends `enter(final text)`, so text typed together with Enter is never lost.
    Input {
        value: String,
        size: Size,
        focus: bool,
        /// grey text shown while the field is empty.
        hint: String,
        edit: Box<dyn Fn(String) -> M>,
        enter: Option<Box<dyn Fn(String) -> M>>,
    },
    /// a one-line field for a password: masked unless `shown`, and what is typed only ever
    /// travels in zeroized memory.
    Secret {
        value: Zeroizing<String>,
        size: Size,
        shown: bool,
        focus: bool,
        edit: Box<dyn Fn(Zeroizing<String>) -> M>,
        enter: Option<Box<dyn Fn(Zeroizing<String>) -> M>>,
    },
    /// a scrolling box of fixed-width text lines.
    Lines {
        id: &'static str,
        lines: Vec<String>,
        height: f32,
    },
    /// the black console: coloured lines that follow the newest one, `height` tall or filling
    /// the window when `None`.
    Log {
        id: &'static str,
        lines: Vec<(Kind, String)>,
        height: Option<f32>,
    },
    /// a one-column list in a white box; clicking an item sends `select(index)`. `empty` is
    /// shown when there are no items.
    List {
        id: &'static str,
        items: Vec<String>,
        selected: Option<usize>,
        min_height: f32,
        empty: String,
        select: fn(usize) -> M,
    },
    /// a row of buttons, left aligned, in order.
    Toolbar(Vec<Button<M>>),
    /// a window's big icon (by icon id) beside its title and a few lines about it.
    Header {
        icon: &'static str,
        title: String,
        lines: Vec<String>,
    },
    /// fixed-width text in a white box, such as an example settings file.
    Code(String),
    /// a thin etched line across the window.
    Separator,
    /// a progress bar filled to `fraction` (0 to 1) with `text` on it.
    Bar { fraction: f32, text: String },
    /// nodes side by side, left to right; a `Gap` is horizontal space here.
    Row(Vec<Node<M>>),
    /// a box of editable fixed-width text, `height` tall; each change sends `edit(new text)`.
    TextArea {
        id: &'static str,
        value: String,
        height: f32,
        /// grey text shown while the box is empty.
        hint: String,
        edit: fn(String) -> M,
    },
    /// a drop-down list showing `current`; picking an option sends `pick(its key)`.
    Choice {
        id: &'static str,
        current: String,
        width: f32,
        options: Vec<(usize, String)>,
        pick: fn(usize) -> M,
    },
    /// a section under a title that opens and closes when the title is clicked; the front end
    /// remembers whether it is open.
    Fold {
        id: &'static str,
        title: String,
        body: Vec<Node<M>>,
    },
    /// nodes shown greyed out and not clickable.
    Disabled(Vec<Node<M>>),
    /// a form row: `label` in the label column, `body` beside it.
    Labelled { label: String, body: Vec<Node<M>> },
    /// a small red line under the field it is about.
    FieldError(String),
    /// a warning sign beside a message, as in a message box.
    Alert(String),
    /// a value with an up and a down button; a press sends `step(1)` or `step(-1)`.
    Spin {
        id: &'static str,
        text: String,
        width: f32,
        enabled: bool,
        step: fn(i32) -> M,
    },
    /// a month as a grid of days from Sunday, `selected` marked and `today` outlined;
    /// clicking a day sends `pick(day)`.
    Calendar {
        id: &'static str,
        year: i32,
        month: u32,
        selected: NaiveDate,
        today: NaiveDate,
        enabled: bool,
        pick: fn(NaiveDate) -> M,
    },
    /// shows a time of day: egui draws an analogue face `size` across, the tui may print it.
    ClockFace {
        hour: u32,
        minute: u32,
        second: u32,
        size: f32,
    },
    /// columns side by side, tops aligned; a column may ask for a width.
    Columns(Vec<(Option<f32>, Vec<Node<M>>)>),
    /// a small inline icon, by icon id.
    Icon(&'static str),
    /// a line of red text, for input that was refused.
    Warning(String),
    /// a white box that scrolls once its contents are taller than `max_height`.
    Scroll {
        id: &'static str,
        max_height: f32,
        body: Vec<Node<M>>,
    },
    /// the white band at the top of a wizard page.
    Banner(crate::view_wizard::Banner),
    /// a list of tick boxes with icons.
    CheckList(crate::view_wizard::CheckList<M>),
    /// a sunken box of wrapped text, like the description under a setup list.
    Description(String),
    /// a text field that says which one it is.
    KeyedInput(crate::view_wizard::KeyedInput<M>),
}

impl<M> Node<M> {
    pub fn field(label: &str, value: impl Into<String>) -> Self {
        Self::Field {
            label: label.into(),
            value: value.into(),
        }
    }
}

/// how wide a field is: a fixed number of layout units, or the rest of the row less some room.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Size {
    Points(f32),
    FillLess(f32),
}

pub struct Table<M> {
    /// keeps the list's scroll position between frames.
    pub id: &'static str,
    pub headings: Vec<&'static str>,
    /// the widths of all but the last column, which takes the rest.
    pub widths: Vec<f32>,
    pub rows: Vec<Vec<String>>,
    pub selected: Option<usize>,
    /// rows with a problem, drawn red.
    pub bad: Vec<bool>,
    pub height: f32,
    pub select: fn(usize) -> M,
}

pub struct Button<M> {
    pub label: String,
    pub enabled: bool,
    pub press: M,
    /// the one of a set that is chosen: drawn pressed, and does nothing when clicked. a front
    /// end without a pressed look shows it as disabled.
    pub selected: bool,
    /// a fixed width in points, so a stack of buttons lines up.
    pub width: Option<f32>,
}

impl<M> Button<M> {
    pub fn new(label: &str, enabled: bool, press: M) -> Self {
        Self {
            label: label.into(),
            enabled,
            press,
            selected: false,
            width: None,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }
}

/// a modal box over the desktop.
pub struct Dialog<M> {
    pub id: &'static str,
    pub title: String,
    pub body: Vec<Node<M>>,
    /// what pressing Escape sends, if it closes the box.
    pub escape: Option<M>,
}

/// the titles of a row of tabs, from their names.
pub fn titles(names: &[&str]) -> Vec<String> {
    names.iter().map(ToString::to_string).collect()
}

/// the status bar for a window following a `Remote`.
pub fn status<T, M>(remote: &crate::remote::Remote<T>) -> Node<M> {
    let (error, text) = remote.note.clone().unwrap_or_default();
    Node::Status { text, error }
}

/// the messages of the errors at `path` (or below it, like `a.b[2]` for `a.b[2].c`).
pub fn messages(errors: &[ValidationError], path: &str) -> Vec<String> {
    errors
        .iter()
        .filter(|e| matches_path(&e.path, path))
        .map(|e| e.message.clone())
        .collect()
}

fn matches_path(error: &str, path: &str) -> bool {
    error == path
        || error
            .strip_prefix(path)
            .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('['))
}

/// for each of `count` rows, does any error sit under `{prefix}[i]`?
pub fn bad_rows(errors: &[ValidationError], prefix: &str, count: usize) -> Vec<bool> {
    (0..count)
        .map(|i| !messages(errors, &format!("{prefix}[{i}]")).is_empty())
        .collect()
}

/// the problems with the field at `path`, one small red line each.
pub fn field_errors<M>(errors: &[ValidationError], path: &str) -> Vec<Node<M>> {
    messages(errors, path)
        .into_iter()
        .map(Node::FieldError)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(path: &str) -> ValidationError {
        ValidationError {
            path: path.into(),
            message: format!("bad {path}"),
        }
    }

    #[test]
    fn messages_match_a_path_and_what_is_below_it() {
        let errors = [
            error("dns.records[1].name"),
            error("dns.records[10].name"),
            error("dns.zone"),
            error("dns.dhcp.range[0]"),
        ];
        assert_eq!(messages(&errors, "dns.zone"), ["bad dns.zone"]);
        assert_eq!(messages(&errors, "dns.records[1]").len(), 1);
        assert_eq!(messages(&errors, "dns.dhcp.range").len(), 1);
        assert!(messages(&errors, "dns.zon").is_empty());
        assert_eq!(
            bad_rows(&errors, "dns.records", 3),
            [false, true, false],
            "[1] must not match [10]"
        );
    }
}
