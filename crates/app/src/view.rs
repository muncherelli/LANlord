//! the layout language every front end draws. a window describes its contents as a tree of
//! `Node`s built from its state; egui (on the desktop and in the browser) and the tui each
//! interpret the tree in their own way and hand back the message of whatever was clicked or
//! typed, which goes to the window's `update`.
//!
//! sizes are layout units: one unit is one egui point at 100%. the tui divides them by the
//! size of a character cell.
//!
//! windows move to this tree one by one; the variants grow with them.

pub use lanlord_core::log::Kind;

/// a piece of a window, carrying the message type `M` of the window it belongs to.
pub enum Node<M> {
    /// a line of plain text.
    Text(String),
    /// empty space below the previous node.
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
    /// one button on its own line, left aligned.
    Button(Button<M>),
    /// a row of tabs over the page of the `current` one; clicking a tab sends `pick(tab)`.
    Tabs {
        titles: Vec<&'static str>,
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
    /// a one-line text field; each change sends `edit(new text)`.
    Input {
        value: String,
        width: f32,
        edit: fn(String) -> M,
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
    /// nodes side by side, left to right; a `Gap` is horizontal space here.
    Row(Vec<Node<M>>),
    /// a box of editable fixed-width text, `height` tall; each change sends `edit(new text)`.
    TextArea {
        id: &'static str,
        value: String,
        height: f32,
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
    /// a one-line text field as wide as the row less `room`; `masked` hides what is typed,
    /// `focus` puts the cursor in it. each change sends `edit(new text)`.
    Entry {
        value: String,
        room: f32,
        masked: bool,
        focus: bool,
        /// grey text shown while the field is empty.
        hint: String,
        edit: Box<dyn Fn(String) -> M>,
        /// what pressing Enter in the field sends, if anything.
        enter: Option<M>,
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
    /// a line of red text, for input that was refused.
    Warning(String),
    /// a white box that scrolls once its contents are taller than `max_height`.
    Scroll {
        id: &'static str,
        max_height: f32,
        body: Vec<Node<M>>,
    },
}

impl<M> Node<M> {
    pub fn field(label: &str, value: impl Into<String>) -> Self {
        Self::Field {
            label: label.into(),
            value: value.into(),
        }
    }
}

pub struct Table<M> {
    /// keeps the list's scroll position between frames.
    pub id: &'static str,
    pub headings: Vec<&'static str>,
    /// the widths of all but the last column, which takes the rest.
    pub widths: Vec<f32>,
    pub rows: Vec<Vec<String>>,
    pub selected: Option<usize>,
    pub height: f32,
    pub select: fn(usize) -> M,
}

pub struct Button<M> {
    pub label: String,
    pub enabled: bool,
    pub press: M,
}

impl<M> Button<M> {
    pub fn new(label: &str, enabled: bool, press: M) -> Self {
        Self {
            label: label.into(),
            enabled,
            press,
        }
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

/// the status bar for a window following a `Remote`.
pub fn status<T, M>(remote: &crate::remote::Remote<T>) -> Node<M> {
    let (error, text) = remote.note.clone().unwrap_or_default();
    Node::Status { text, error }
}
