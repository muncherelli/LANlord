//! the pieces of the layout tree that setup wizards use: the white band across the top, a list
//! of tick boxes with icons, a sunken description box and keyed text fields. they sit in
//! `view::Node`; this file holds their data so wizards can grow without touching the rest.

use crate::view::Node;

/// one row of a `CheckList`.
pub struct CheckRow {
    /// the id of the small icon before the name.
    pub icon: &'static str,
    pub label: String,
    /// the right-hand column, like the size column in the nt setup lists.
    pub detail: String,
    pub checked: bool,
    /// a shaded box: only some of its parts are on.
    pub partial: bool,
    /// a box that cannot be cleared (or ticked) is greyed out.
    pub enabled: bool,
}

/// a white list of tick boxes with small icons, one row highlighted; clicking a row sends
/// `select(row)` and clicking its box sends `toggle(row)`.
pub struct CheckList<M> {
    pub id: &'static str,
    pub rows: Vec<CheckRow>,
    pub selected: Option<usize>,
    pub height: f32,
    pub select: fn(usize) -> M,
    pub toggle: fn(usize) -> M,
}

/// a one-line text field that tells its owner which one it is: each change sends
/// `edit(key, new text)`.
pub struct KeyedInput<M> {
    pub key: usize,
    pub value: String,
    pub width: f32,
    pub edit: fn(usize, String) -> M,
}

/// the white band at the top of a wizard page: a bold title, a line under it and an icon on
/// the right.
pub struct Banner {
    pub title: String,
    pub subtitle: String,
    pub icon: &'static str,
}

impl<M> Node<M> {
    pub fn banner(title: &str, subtitle: &str, icon: &'static str) -> Self {
        Self::Banner(Banner {
            title: title.into(),
            subtitle: subtitle.into(),
            icon,
        })
    }
}
