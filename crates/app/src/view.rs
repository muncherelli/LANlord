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
    /// a page of nodes that is as tall as the tallest of `others`, which are the other pages it
    /// can be, so the window does not change size from one page to the next.
    SameSize {
        /// stands for what the pages are made of: the height is measured again when it changes
        key: u64,
        body: Vec<Node<M>>,
        others: Vec<Vec<Node<M>>>,
    },
    /// a captioned frame of a fixed height, `lines` rows of text, whose body scrolls when it is
    /// longer: what is shown in it never changes the size of the page it is on.
    Details {
        title: String,
        lines: u8,
        body: Vec<Node<M>>,
    },
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
        /// stands for what the pages are made of (see `fingerprint`): the sizes of the pages are
        /// measured again when it changes
        key: u64,
        current: usize,
        pick: fn(usize) -> M,
        body: Vec<Node<M>>,
        /// the other pages by tab number, so a window can be as big as the biggest of them
        /// whichever is open; may leave out any it cannot build without side effects
        others: Vec<(usize, Vec<Node<M>>)>,
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
        /// false draws it with a gray face, like an edit box that is not in use
        enabled: bool,
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
    /// the same red line for a field whose text may still become valid: while `partial` is set
    /// and the keyboard is in a field, it is not shown (its room stays), so typing an address
    /// is never met with an error. it appears once the focus leaves the field. `field` says
    /// which field it is about: the keyboard in any other field does not hide it.
    HeldError {
        text: String,
        partial: bool,
        field: HeldFor,
    },
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
    /// as wide as this text, the longest the box has to show.
    Text(&'static str),
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
    /// keeps its height when the window has room to spare, which goes to what comes after.
    pub steady: bool,
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

/// the field a `Node::HeldError` is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeldFor {
    /// the text field drawn `n` fields before the line: 0 is the one just above it.
    Above(usize),
    /// any of the `n` text fields drawn just above the line.
    AnyAbove(usize),
    /// the wizard field (`KeyedInput`) with this key.
    Keyed((usize, usize)),
}

/// for each of `count` rows, does any error sit under `{prefix}[i]`?
pub fn bad_rows(errors: &[ValidationError], prefix: &str, count: usize) -> Vec<bool> {
    (0..count)
        .map(|i| !messages(errors, &format!("{prefix}[{i}]")).is_empty())
        .collect()
}

/// like `field_errors`, for an address field: the lines stay hidden while the field is being
/// typed in and `text` could still become an address (see `Node::HeldError`).
pub fn address_errors<M>(errors: &[ValidationError], path: &str, text: &str) -> Vec<Node<M>> {
    let partial = lanlord_model::validate::ipv4_partial(text.trim());
    messages(errors, path)
        .into_iter()
        .map(|text| Node::HeldError {
            text,
            partial,
            field: HeldFor::Above(0),
        })
        .collect()
}

/// the problems with the field at `path`, one small red line each.
pub fn field_errors<M>(errors: &[ValidationError], path: &str) -> Vec<Node<M>> {
    messages(errors, path)
        .into_iter()
        .map(Node::FieldError)
        .collect()
}

/// a number that changes when what `nodes` show changes in a way that can change their size: the
/// text, the rows and the parts, not what is typed, ticked or picked. a window that sizes itself
/// to its pages measures them again when it changes.
pub fn fingerprint<M>(nodes: &[Node<M>]) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hash_nodes(nodes, &mut hasher);
    hasher.finish()
}

/// the `fingerprint` of the pages of a tab strip: the open one, `body` at tab `current`, and
/// `others` by tab number, the same whichever is open.
pub fn pages_fingerprint<M>(
    current: usize,
    body: &[Node<M>],
    others: &[(usize, Vec<Node<M>>)],
) -> u64 {
    let mut pages: Vec<(usize, &[Node<M>])> =
        others.iter().map(|(t, n)| (*t, n.as_slice())).collect();
    pages.push((current, body));
    pages_fingerprint_of(pages)
}

/// the same, for pages given as tab numbers and nodes.
pub fn pages_fingerprint_of<'a, M: 'a>(
    pages: impl IntoIterator<Item = (usize, &'a [Node<M>])>,
) -> u64 {
    use std::hash::Hasher;
    let mut pages: Vec<(usize, u64)> = pages
        .into_iter()
        .map(|(tab, nodes)| (tab, fingerprint(nodes)))
        .collect();
    pages.sort_unstable();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&pages, &mut hasher);
    hasher.finish()
}

fn hash_nodes<M>(nodes: &[Node<M>], h: &mut impl std::hash::Hasher) {
    use std::hash::Hash;
    nodes.len().hash(h);
    for node in nodes {
        std::mem::discriminant(node).hash(h);
        match node {
            Node::Text(t)
            | Node::Heading(t)
            | Node::Bullet(t)
            | Node::Code(t)
            | Node::FieldError(t)
            | Node::Alert(t)
            | Node::Warning(t)
            | Node::Description(t) => t.hash(h),
            Node::Gap(g) => g.to_bits().hash(h),
            Node::Group { title, body } | Node::Fold { title, body, .. } => {
                title.hash(h);
                hash_nodes(body, h);
            }
            Node::SameSize { key, body, others } => {
                key.hash(h);
                hash_nodes(body, h);
                others.iter().for_each(|o| hash_nodes(o, h));
            }
            Node::Details { title, lines, body } => {
                (title, lines).hash(h);
                hash_nodes(body, h);
            }
            Node::Table(t) => {
                t.headings.hash(h);
                t.rows.hash(h);
                t.height.to_bits().hash(h);
                t.steady.hash(h);
            }
            Node::Buttons(b) | Node::Toolbar(b) => b.iter().for_each(|b| b.label.hash(h)),
            Node::Button(b) => (&b.label, b.width.map(f32::to_bits)).hash(h),
            Node::Field { label, value } => (label, value).hash(h),
            Node::Status { text, error } => (text, error).hash(h),
            Node::HeldError { text, .. } => text.hash(h),
            Node::Tabs {
                titles,
                key,
                body,
                others,
                ..
            } => {
                (titles, key).hash(h);
                hash_nodes(body, h);
                others.iter().for_each(|(_, o)| hash_nodes(o, h));
            }
            Node::Danger(body) | Node::Disabled(body) => hash_nodes(body, h),
            Node::Check { label, .. } => label.hash(h),
            Node::Input { hint, .. } | Node::TextArea { hint, .. } => hint.hash(h),
            Node::Secret { .. }
            | Node::Separator
            | Node::Bar { .. }
            | Node::Calendar { .. }
            | Node::ClockFace { .. }
            | Node::Spin { .. }
            | Node::KeyedInput(_) => {}
            Node::Lines { lines, height, .. } => (lines, height.to_bits()).hash(h),
            Node::Log { lines, .. } => lines.len().hash(h),
            Node::List { items, empty, .. } => (items, empty).hash(h),
            Node::Header { icon, title, lines } => (icon, title, lines).hash(h),
            Node::Row(body) => hash_nodes(body, h),
            Node::Choice {
                current, options, ..
            } => (current, options).hash(h),
            Node::Labelled { label, body } => {
                label.hash(h);
                hash_nodes(body, h);
            }
            Node::Icon(i) => i.hash(h),
            Node::Columns(cols) => cols.iter().for_each(|(w, body)| {
                w.map(f32::to_bits).hash(h);
                hash_nodes(body, h);
            }),
            Node::Scroll {
                max_height, body, ..
            } => {
                max_height.to_bits().hash(h);
                hash_nodes(body, h);
            }
            Node::Banner(b) => (&b.title, &b.subtitle, b.icon).hash(h),
            Node::CheckList(c) => c
                .rows
                .iter()
                .for_each(|r| (r.icon, &r.label, &r.detail).hash(h)),
        }
    }
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
