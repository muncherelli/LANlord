//! draws the app's layout tree (`lanlord_app::view`) in character cells with the paint
//! toolkit, the way `lanlord-ui` draws it with egui, and turns clicks and keys back into the
//! window's messages. nothing here knows what a window is for: every window that describes
//! itself as a tree shows up in the terminal through this one file.

use std::collections::{HashMap, HashSet};

use chrono::{Datelike, NaiveDate};
use lanlord_app::view::{Button, Kind, Node, Size};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget;
use zeroize::Zeroizing;

use crate::paint::chrome::{row_button, Bevel};
use crate::paint::controls::{self, Checkbox, TextField};
use crate::paint::desktop::GlyphTile;
use crate::paint::glyphs::chip;
use crate::paint::palette::{Palette, Rgb, BLACK, GRAY, NAVY, SHADOW, WHITE};
use crate::paint::{bold, fill, style, text};

/// layout units per column and per row: one unit is one egui point, and a gui line of text is
/// about 16 points tall and 7 wide per character.
const COL: f32 = 7.0;
const ROW: f32 = 16.0;

/// the red of refused input and of the danger border, like the gui's.
pub const RED: Rgb = (160, 0, 0);

fn cols(units: f32) -> u16 {
    ((units / COL).round() as u16).max(1)
}

fn rows(units: f32) -> u16 {
    (units / ROW).floor() as u16
}

/// what a region of the screen does when it is clicked or focused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    /// button `n` of a button row (0 for a lone button).
    Press(usize),
    Toggle,
    /// row `n` of a list or table.
    Row(usize),
    /// the whole list or table, for the keyboard.
    List,
    /// tab `n` of a tab row.
    Tab(usize),
    /// the whole tab row, for the keyboard.
    TabBar,
    /// a text field.
    Field,
    /// a drop-down list's box.
    Open,
    /// something that scrolls with the wheel.
    Scroll,
    /// a fold's title, which opens and closes it.
    Fold,
    /// the up (`1`) or down (`-1`) arrow of a spin box; the box itself is `Spin(0)`, for the
    /// keyboard and the wheel.
    Spin(i32),
    /// day `n` of a calendar's grid, counted from the top left; the whole grid is `Days`.
    Day(usize),
    /// a calendar's grid, for the keyboard and the wheel.
    Days,
    /// the tick box of row `n` of a check list.
    Mark(usize),
}

/// a part of a drawn tree: where it is, which node it belongs to (its path through the tree)
/// and what it does.
#[derive(Clone, Debug)]
pub struct Region {
    pub rect: Rect,
    pub path: Vec<usize>,
    pub part: Part,
    /// the keyboard can land on it.
    pub focusable: bool,
    /// names it across frames: the path and the part.
    pub key: String,
}

impl Region {
    fn new(rect: Rect, path: &[usize], part: Part, focusable: bool) -> Self {
        let mut key = path
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(".");
        key.push('#');
        key.push_str(&match &part {
            Part::Press(n) => format!("b{n}"),
            Part::Row(n) => format!("r{n}"),
            Part::Tab(n) => format!("t{n}"),
            other => format!("{other:?}"),
        });
        Self {
            rect,
            path: path.to_vec(),
            part,
            focusable,
            key,
        }
    }
}

/// an open drop-down list, drawn over everything else.
#[derive(Clone, Debug)]
pub struct Dropdown {
    pub path: Vec<usize>,
    /// the box it hangs from.
    pub under: Rect,
    pub options: Vec<(usize, String)>,
    pub current: String,
}

/// what a window remembers between frames: where the keyboard is, how far each box is
/// scrolled and which drop-down is open.
#[derive(Default, Debug)]
pub struct ViewState {
    pub focus: Option<String>,
    pub scroll: HashMap<String, usize>,
    pub dropdown: Option<Dropdown>,
    /// the folds that are open, by the key of their title.
    pub open: HashSet<String>,
}

impl ViewState {
    pub fn scroll_by(&mut self, key: &str, lines: i32) {
        let at = self.scroll.entry(key.to_string()).or_insert(0);
        *at = (*at as i32 + lines).max(0) as usize;
    }
}

/// what the user did to a node, ready to become the window's message. a secret's text only
/// ever travels zeroized.
#[derive(Clone)]
pub enum Input {
    Press(usize),
    Toggle,
    Row(usize),
    Tab(usize),
    Pick(usize),
    /// the field's whole new text.
    Text(String),
    /// Enter in a one-line field, with its final text.
    Enter(String),
    /// a spin box stepped up (`1`) or down (`-1`).
    Step(i32),
    /// day `n` of a calendar's grid picked.
    Day(usize),
    /// the tick box of row `n` of a check list clicked.
    Mark(usize),
    /// a secret field's whole new text.
    SecretText(Zeroizing<String>),
    /// Enter in a secret field, with its final text.
    SecretEnter(Zeroizing<String>),
}

/// what a field holds now.
pub enum FieldValue {
    Plain { text: String, multi: bool },
    Secret(Zeroizing<String>),
}

/// how many nodes a column may hold in a path step: a step into columns is
/// `column * COLUMN + node`.
const COLUMN: usize = 1000;

/// the days of the month shown, as a grid of seven columns starting on Sunday.
fn month_days(year: i32, month: u32) -> Vec<NaiveDate> {
    lanlord_app::clock_window::month_days(year, month)
}

/// the node at `path`, going down through groups, tabs, rows and scroll boxes.
fn find<'a, M>(nodes: &'a [Node<M>], path: &[usize]) -> Option<&'a Node<M>> {
    let (first, rest) = path.split_first()?;
    let node = nodes.get(*first)?;
    if rest.is_empty() {
        return Some(node);
    }
    match node {
        Node::Group { body, .. } | Node::Tabs { body, .. } | Node::Scroll { body, .. } => {
            find(body, rest)
        }
        Node::Danger(body) | Node::Row(body) | Node::Disabled(body) => find(body, rest),
        // the next step of a path through columns packs the column and the node in it
        Node::Columns(columns) => {
            let (step, rest) = rest.split_first()?;
            let (_, body) = columns.get(step / COLUMN)?;
            let mut inside = vec![step % COLUMN];
            inside.extend_from_slice(rest);
            find(body, &inside)
        }
        Node::Fold { body, .. } | Node::Labelled { body, .. } => find(body, rest),
        _ => None,
    }
}

/// the message `input` on the node at `path` sends, if it sends one.
pub fn act<M: Clone>(nodes: &[Node<M>], path: &[usize], input: Input) -> Option<M> {
    let pressed = |buttons: &[Button<M>], n: usize| {
        buttons
            .get(n)
            .filter(|b| b.enabled)
            .map(|b| b.press.clone())
    };
    match (find(nodes, path)?, input) {
        (Node::Buttons(buttons) | Node::Toolbar(buttons), Input::Press(n)) => pressed(buttons, n),
        (Node::Button(button), Input::Press(_)) => button.enabled.then(|| button.press.clone()),
        (
            Node::Check {
                enabled, toggle, ..
            },
            Input::Toggle,
        ) => enabled.then(|| toggle.clone()),
        (Node::Table(table), Input::Row(n)) if n < table.rows.len() => Some((table.select)(n)),
        (Node::List { items, select, .. }, Input::Row(n)) if n < items.len() => Some(select(n)),
        (Node::CheckList(list), Input::Row(n)) if n < list.rows.len() => Some((list.select)(n)),
        (Node::CheckList(list), Input::Mark(n)) => list
            .rows
            .get(n)
            .filter(|row| row.enabled)
            .map(|_| (list.toggle)(n)),
        (Node::KeyedInput(input), Input::Text(text)) => {
            (text != input.value).then(|| (input.edit)(input.key, text))
        }
        (
            Node::Tabs {
                titles,
                current,
                pick,
                ..
            },
            Input::Tab(n),
        ) if n < titles.len() && n != *current => Some(pick(n)),
        (Node::Choice { pick, options, .. }, Input::Pick(key))
            if options.iter().any(|(k, _)| *k == key) =>
        {
            Some(pick(key))
        }
        (Node::Input { value, edit, .. }, Input::Text(text)) => {
            (text != *value).then(|| edit(text))
        }
        (Node::TextArea { value, edit, .. }, Input::Text(text)) => {
            (text != *value).then(|| edit(text))
        }
        (
            Node::Input {
                enter: Some(enter), ..
            },
            Input::Enter(text),
        ) => Some(enter(text)),
        (
            Node::Spin {
                enabled: true,
                step,
                ..
            },
            Input::Step(dir),
        ) => Some(step(dir)),
        (
            Node::Calendar {
                year,
                month,
                enabled: true,
                pick,
                ..
            },
            Input::Day(n),
        ) => month_days(*year, *month).get(n).map(|day| pick(*day)),
        (Node::Secret { value, edit, .. }, Input::SecretText(text)) => {
            (*text != **value).then(|| edit(text))
        }
        (
            Node::Secret {
                enter: Some(enter), ..
            },
            Input::SecretEnter(text),
        ) => Some(enter(text)),
        _ => None,
    }
}

/// the text a field holds now.
pub fn field_value<M>(nodes: &[Node<M>], path: &[usize]) -> Option<FieldValue> {
    match find(nodes, path)? {
        Node::Input { value, .. } => Some(FieldValue::Plain {
            text: value.clone(),
            multi: false,
        }),
        Node::TextArea { value, .. } => Some(FieldValue::Plain {
            text: value.clone(),
            multi: true,
        }),
        Node::Secret { value, .. } => Some(FieldValue::Secret(value.clone())),
        Node::KeyedInput(input) => Some(FieldValue::Plain {
            text: input.value.clone(),
            multi: false,
        }),
        _ => None,
    }
}

/// a calendar's month, selected day and whether it can be picked from.
pub fn calendar_state<M>(nodes: &[Node<M>], path: &[usize]) -> Option<(i32, u32, NaiveDate, bool)> {
    match find(nodes, path)? {
        Node::Calendar {
            year,
            month,
            selected,
            enabled,
            ..
        } => Some((*year, *month, *selected, *enabled)),
        _ => None,
    }
}

/// a one-line field sends something of its own when Enter is pressed in it.
pub fn field_enters<M>(nodes: &[Node<M>], path: &[usize]) -> bool {
    matches!(
        find(nodes, path),
        Some(Node::Input { enter: Some(_), .. } | Node::Secret { enter: Some(_), .. })
    )
}

/// the selected row of a list or table and how many rows it has.
pub fn list_state<M>(nodes: &[Node<M>], path: &[usize]) -> Option<(Option<usize>, usize)> {
    match find(nodes, path)? {
        Node::Table(table) => Some((table.selected, table.rows.len())),
        Node::List {
            items, selected, ..
        } => Some((*selected, items.len())),
        Node::CheckList(list) => Some((list.selected, list.rows.len())),
        _ => None,
    }
}

/// the current tab and how many there are.
pub fn tab_state<M>(nodes: &[Node<M>], path: &[usize]) -> Option<(usize, usize)> {
    match find(nodes, path)? {
        Node::Tabs {
            titles, current, ..
        } => Some((*current, titles.len())),
        _ => None,
    }
}

/// a choice's options and the key of the one showing.
pub fn choice_state<M>(
    nodes: &[Node<M>],
    path: &[usize],
) -> Option<(Vec<(usize, String)>, String)> {
    match find(nodes, path)? {
        Node::Choice {
            options, current, ..
        } => Some((options.clone(), current.clone())),
        _ => None,
    }
}

/// the first field that asks for the cursor (`focus: true`), as a region key.
pub fn wants_focus<M>(nodes: &[Node<M>], regions: &[Region], prefix: &[usize]) -> Option<String> {
    regions
        .iter()
        .filter(|r| r.part == Part::Field)
        .find(|r| {
            let Some(path) = r.path.strip_prefix(prefix) else {
                return false;
            };
            matches!(
                find(nodes, path),
                Some(Node::Input { focus: true, .. } | Node::Secret { focus: true, .. })
            )
        })
        .map(|r| r.key.clone())
}

/// split `text` into lines no wider than `width`, breaking at spaces when it can.
pub fn wrap(text: &str, width: u16) -> Vec<String> {
    let width = usize::from(width.max(1));
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split(' ') {
            let fits = line.chars().count() + usize::from(!line.is_empty()) + word.chars().count();
            if !line.is_empty() && fits > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
            while line.chars().count() > width {
                let rest: String = line.chars().skip(width).collect();
                lines.push(line.chars().take(width).collect());
                line = rest;
            }
        }
        lines.push(line);
    }
    lines
}

/// draws a tree and collects its regions.
pub struct Painter<'a> {
    pub palette: Palette,
    pub state: &'a ViewState,
    /// the window has the keyboard, so its focus shows.
    pub active: bool,
    pub regions: Vec<Region>,
}

impl<'a> Painter<'a> {
    pub fn new(palette: Palette, state: &'a ViewState, active: bool) -> Self {
        Self {
            palette,
            state,
            active,
            regions: Vec::new(),
        }
    }

    fn focused(&self, path: &[usize], part: Part) -> bool {
        let key = Region::new(Rect::default(), path, part, false).key;
        self.active && self.state.focus.as_deref() == Some(key.as_str())
    }

    fn push(&mut self, rect: Rect, path: &[usize], part: Part, focusable: bool) {
        self.regions.push(Region::new(rect, path, part, focusable));
    }

    fn st(&self, fg: Rgb, bg: Rgb) -> Style {
        style(self.palette, fg, bg)
    }

    /// how many rows `nodes` take at `width`, giving a console whatever `room` is left.
    pub fn height<M>(&self, nodes: &[Node<M>], width: u16, room: Option<u16>) -> u16 {
        self.height_at(nodes, width, room, &[])
    }

    fn height_at<M>(
        &self,
        nodes: &[Node<M>],
        width: u16,
        room: Option<u16>,
        path: &[usize],
    ) -> u16 {
        let fixed: u16 = nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| !fills(n))
            .map(|(i, n)| self.node_height(n, width, &child(path, i)))
            .sum();
        let logs = nodes.iter().filter(|n| fills(n)).count() as u16;
        let log = match room {
            Some(room) => room
                .saturating_sub(fixed)
                .checked_div(logs)
                .map_or(0, |rows| rows.max(4)),
            None => 8,
        };
        fixed + logs * log
    }

    fn node_height<M>(&self, node: &Node<M>, width: u16, path: &[usize]) -> u16 {
        let inner = |body: &[Node<M>], width: u16| self.height_at(body, width, None, path);
        match node {
            Node::Text(text) => wrap(text, width).len() as u16,
            Node::Gap(height) => rows(*height),
            Node::Group { body, .. } => 2 + inner(body, width.saturating_sub(4)),
            Node::Table(table) => rows(table.height).max(3) + 2,
            Node::Tabs { body, .. } => 3 + inner(body, width.saturating_sub(4)),
            Node::Danger(body) => 2 + inner(body, width.saturating_sub(4)),
            Node::Lines { height, .. } => rows(*height).max(2) + 2,
            Node::Log { height, .. } => height.map_or(8, |h| rows(h).max(3) + 2),
            Node::List {
                items, min_height, ..
            } => {
                let least = rows(*min_height).max(2);
                (items.len() as u16).clamp(least, least.max(8)) + 2
            }
            Node::Header { lines, .. } => (1 + lines.len() as u16).max(3) + 1,
            Node::Code(text) => text.lines().count() as u16 + 2,
            Node::Row(body) => self
                .row_lines(body, width)
                .iter()
                .map(|line| {
                    line.iter()
                        .map(|(i, _)| self.node_height(&body[*i], width, &child(path, *i)))
                        .max()
                        .unwrap_or(0)
                })
                .sum(),
            Node::TextArea { height, .. } => rows(*height).max(3) + 2,
            Node::Scroll {
                max_height, body, ..
            } => inner(body, width.saturating_sub(2)).min(rows(*max_height).max(3)) + 2,
            Node::Fold { body, .. } => {
                if self.state.open.contains(&fold_key(path)) {
                    1 + inner(body, width.saturating_sub(2))
                } else {
                    1
                }
            }
            Node::Disabled(body) => inner(body, width),
            Node::Labelled { body, .. } => inner(body, width.saturating_sub(LABEL)).max(1),
            Node::Alert(text) => wrap(text, width.saturating_sub(5)).len() as u16,
            Node::Spin { .. } => 1,
            Node::ClockFace { .. } => 1,
            Node::KeyedInput(_) => 1,
            Node::Banner(_) => 3,
            Node::CheckList(list) => rows(list.height).max(3) + 2,
            Node::Description(text) => wrap(text, width.saturating_sub(2)).len() as u16 + 2,
            Node::Calendar { .. } => 9,
            Node::Columns(columns) => {
                let widths = self.column_widths(columns, width);
                columns
                    .iter()
                    .zip(widths)
                    .enumerate()
                    .map(|(c, ((_, body), w))| self.height_in(body, w, &child(path, c), c))
                    .max()
                    .unwrap_or(0)
            }
            _ => 1,
        }
    }

    /// draw `nodes` top to bottom inside `area`.
    pub fn draw<M>(&mut self, nodes: &[Node<M>], area: Rect, buf: &mut Buffer, path: &[usize]) {
        let logs = nodes.iter().filter(|n| fills(n)).count() as u16;
        let without: u16 = nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| !fills(n))
            .map(|(i, n)| self.node_height(n, area.width, &child(path, i)))
            .sum();
        let log_rows = area
            .height
            .saturating_sub(without)
            .checked_div(logs)
            .map_or(0, |rows| rows.max(4));
        let mut y = area.y;
        for (i, node) in nodes.iter().enumerate() {
            let here = child(path, i);
            let height = if fills(node) {
                log_rows
            } else {
                self.node_height(node, area.width, &here)
            };
            let rect = Rect::new(area.x, y, area.width, height);
            self.node(node, rect, buf, &here);
            y = y.saturating_add(height);
        }
    }

    fn node<M>(&mut self, node: &Node<M>, area: Rect, buf: &mut Buffer, path: &[usize]) {
        let p = self.palette;
        match node {
            Node::Text(t) => {
                for (n, line) in wrap(t, area.width).iter().enumerate() {
                    text(
                        buf,
                        area.x,
                        area.y + n as u16,
                        area.width,
                        line,
                        self.st(BLACK, GRAY),
                    );
                }
            }
            Node::Gap(_) => {}
            Node::Heading(t) => {
                text(
                    buf,
                    area.x,
                    area.y,
                    area.width,
                    t,
                    bold(self.st(BLACK, GRAY)),
                );
            }
            Node::Warning(t) => {
                text(buf, area.x, area.y, area.width, t, self.st(RED, GRAY));
            }
            Node::Field { label, value } => {
                let label_width = LABEL.min(area.width / 2);
                text(
                    buf,
                    area.x,
                    area.y,
                    label_width,
                    label,
                    self.st(BLACK, GRAY),
                );
                text(
                    buf,
                    area.x + label_width,
                    area.y,
                    area.width.saturating_sub(label_width),
                    value,
                    self.st(BLACK, GRAY),
                );
            }
            Node::Status { text: t, error } => {
                let fg = if *error { RED } else { BLACK };
                fill(buf, area, p, GRAY);
                edges(buf, area, p);
                text(
                    buf,
                    area.x + 1,
                    area.y,
                    area.width.saturating_sub(2),
                    t,
                    self.st(fg, GRAY),
                );
            }
            Node::Separator => {
                let line = "─".repeat(usize::from(area.width));
                text(
                    buf,
                    area.x,
                    area.y,
                    area.width,
                    &line,
                    self.st(SHADOW, GRAY),
                );
            }
            Node::Group { title, body } => {
                frame_box(buf, area, p, SHADOW, Some(title));
                let inner = Rect::new(
                    area.x + 2,
                    area.y + 1,
                    area.width.saturating_sub(4),
                    area.height.saturating_sub(2),
                );
                self.draw(body, inner, buf, path);
            }
            Node::Danger(body) => {
                frame_box(buf, area, p, RED, None);
                let inner = Rect::new(
                    area.x + 2,
                    area.y + 1,
                    area.width.saturating_sub(4),
                    area.height.saturating_sub(2),
                );
                self.draw(body, inner, buf, path);
            }
            Node::Buttons(buttons) => {
                let mut right = area.right();
                for (n, button) in buttons.iter().enumerate() {
                    let width = button_width(&button.label).min(right.saturating_sub(area.x));
                    let rect = Rect::new(right.saturating_sub(width), area.y, width, 1);
                    self.button(button, rect, buf, path, n);
                    right = rect.x.saturating_sub(1);
                }
            }
            Node::Toolbar(buttons) => {
                let mut x = area.x;
                for (n, button) in buttons.iter().enumerate() {
                    let width = button_width(&button.label).min(area.right().saturating_sub(x));
                    let rect = Rect::new(x, area.y, width, 1);
                    self.button(button, rect, buf, path, n);
                    x = rect.right().saturating_add(1);
                }
            }
            Node::Button(button) => {
                let width = button_width(&button.label).min(area.width);
                self.button(button, Rect::new(area.x, area.y, width, 1), buf, path, 0);
            }
            Node::Check {
                label,
                checked,
                enabled,
                ..
            } => {
                let rect = Rect::new(
                    area.x,
                    area.y,
                    (label.chars().count() as u16 + 4).min(area.width),
                    1,
                );
                let rect = rect.intersection(buf.area);
                if !rect.is_empty() {
                    Checkbox {
                        label,
                        checked: *checked,
                        focused: self.focused(path, Part::Toggle),
                        palette: p,
                    }
                    .render(rect, buf);
                    if !enabled {
                        recolour(buf, rect, self.st(SHADOW, GRAY));
                    }
                }
                if *enabled {
                    self.push(rect, path, Part::Toggle, true);
                }
            }
            Node::Input {
                value, size, hint, ..
            } => {
                let rect = Rect::new(area.x, area.y, size_cells(size, area.width), 1);
                self.field(value, false, rect, buf, path);
                if value.is_empty() && !self.focused(path, Part::Field) && rect.width > 2 {
                    let style = self.st(SHADOW, WHITE);
                    text(buf, rect.x + 1, rect.y, rect.width - 2, hint, style);
                }
            }
            Node::Secret {
                value, size, shown, ..
            } => {
                let rect = Rect::new(area.x, area.y, size_cells(size, area.width), 1);
                self.field(value, !shown, rect, buf, path);
            }
            Node::Spin {
                text,
                width,
                enabled,
                ..
            } => {
                let _ = width;
                let w = spin_cells(text);
                let field = Rect::new(area.x, area.y, w.min(area.width), 1).intersection(buf.area);
                if field.is_empty() {
                    return;
                }
                fill(buf, field, p, WHITE);
                let ink = if *enabled { BLACK } else { SHADOW };
                let shown = text.chars().count() as u16;
                text_at(
                    buf,
                    field.x + field.width.saturating_sub(shown) / 2,
                    field.y,
                    field.width,
                    text,
                    self.st(ink, WHITE),
                );
                // two arrows beside it, up then down, like the gui's spin buttons
                let mut x = field.right();
                for (dir, glyph) in [(1, "▲"), (-1, "▼")] {
                    let arrow = Rect::new(x, area.y, 2, 1).intersection(buf.area);
                    if arrow.is_empty() {
                        break;
                    }
                    fill(buf, arrow, p, GRAY);
                    let face = if *enabled { BLACK } else { SHADOW };
                    text_at(buf, arrow.x, arrow.y, 2, glyph, self.st(face, GRAY));
                    if *enabled {
                        self.push(arrow, path, Part::Spin(dir), false);
                    }
                    x = arrow.right();
                }
                let whole = Rect::new(field.x, field.y, x - field.x, 1);
                if *enabled {
                    self.push(whole, path, Part::Spin(0), true);
                    if self.focused(path, Part::Spin(0)) {
                        recolour(buf, field, self.st(WHITE, NAVY));
                    }
                }
            }
            Node::ClockFace {
                hour,
                minute,
                second,
                ..
            } => {
                let t = format!("{hour:02}:{minute:02}:{second:02}");
                text_at(
                    buf,
                    area.x,
                    area.y,
                    area.width,
                    &t,
                    bold(self.st(BLACK, GRAY)),
                );
            }
            Node::Calendar {
                year,
                month,
                selected,
                today,
                enabled,
                ..
            } => {
                // a day is as wide as the room allows, from 5 cells down to 3
                let cell_w = (area.width.saturating_sub(2) / 7).clamp(3, CELL_W);
                let grid = Rect::new(
                    area.x,
                    area.y,
                    (7 * cell_w + 2).min(area.width),
                    9.min(area.height),
                );
                self.sunken(grid, buf, WHITE);
                let top = grid.y + 1;
                for (i, name) in lanlord_app::clock_window::WEEKDAYS.iter().enumerate() {
                    let x = grid.x + 1 + i as u16 * cell_w;
                    text_at(buf, x + cell_w / 2, top, 1, name, self.st(SHADOW, WHITE));
                }
                let focused = self.focused(path, Part::Days);
                for (i, day) in month_days(*year, *month).into_iter().enumerate() {
                    let x = grid.x + 1 + (i % 7) as u16 * cell_w;
                    let y = top + 1 + (i / 7) as u16;
                    if y >= grid.bottom().saturating_sub(1) {
                        break;
                    }
                    let cell = Rect::new(x, y, cell_w, 1).intersection(buf.area);
                    let (is_selected, in_month) = (day == *selected, day.month() == *month);
                    let (fg, bg) = match (is_selected, in_month) {
                        (true, _) => (WHITE, if focused { NAVY } else { SHADOW }),
                        (false, true) => (BLACK, WHITE),
                        (false, false) => (SHADOW, WHITE),
                    };
                    fill(buf, cell, p, bg);
                    let label = day.day().to_string();
                    let width = label.chars().count() as u16;
                    let mut mark = self.st(fg, bg);
                    if day == *today && !is_selected {
                        // today is outlined in the gui; here it is bold and underlined
                        mark = bold(mark).add_modifier(Modifier::UNDERLINED);
                    }
                    text_at(
                        buf,
                        cell.x + cell.width.saturating_sub(width) / 2,
                        y,
                        cell.width,
                        &label,
                        mark,
                    );
                    if *enabled {
                        self.push(cell, path, Part::Day(i), false);
                    }
                }
                if *enabled {
                    self.push(grid, path, Part::Days, true);
                }
            }
            Node::Columns(columns) => {
                let widths = self.column_widths(columns, area.width);
                let mut x = area.x;
                for (c, ((_, body), width)) in columns.iter().zip(widths).enumerate() {
                    let rect = Rect::new(x, area.y, width, area.height);
                    self.draw_in(body, rect, buf, &child(path, c), c);
                    x = x.saturating_add(width).saturating_add(1);
                }
            }
            Node::Alert(t) => {
                // the message box's warning sign: a black "!" on yellow
                let sign = Rect::new(area.x, area.y, 3, 1).intersection(buf.area);
                if !sign.is_empty() {
                    fill(buf, sign, p, YELLOW);
                    text(
                        buf,
                        sign.x + 1,
                        sign.y,
                        1,
                        "!",
                        bold(self.st(BLACK, YELLOW)),
                    );
                }
                let room = area.width.saturating_sub(5);
                for (n, line) in wrap(t, room).iter().enumerate() {
                    let y = area.y + n as u16;
                    text(buf, area.x + 5, y, room, line, self.st(BLACK, GRAY));
                }
            }
            Node::Icon(id) => chip(buf, area.x, area.y, id, p),
            Node::Banner(banner) => {
                let band = Rect::new(area.x, area.y, area.width, 2);
                fill(buf, band, p, WHITE);
                let room = area.width.saturating_sub(5);
                let title = bold(self.st(BLACK, WHITE));
                text(buf, area.x + 1, area.y, room, &banner.title, title);
                let sub = self.st(BLACK, WHITE);
                text(buf, area.x + 3, area.y + 1, room, &banner.subtitle, sub);
                chip(buf, area.right().saturating_sub(3), area.y, banner.icon, p);
                let line = "─".repeat(usize::from(area.width));
                let under = self.st(SHADOW, GRAY);
                text(buf, area.x, area.y + 2, area.width, &line, under);
            }
            Node::CheckList(list) => {
                let inner = self.sunken(area, buf, WHITE);
                self.push(inner, path, Part::List, true);
                let focused = self.focused(path, Part::List);
                let offset = controls::List::offset(list.selected, inner.height);
                for (n, row) in list
                    .rows
                    .iter()
                    .enumerate()
                    .skip(offset)
                    .take(usize::from(inner.height))
                {
                    let y = inner.y + (n - offset) as u16;
                    let line = Rect::new(inner.x, y, inner.width, 1);
                    let (fg, bg) = if list.selected == Some(n) {
                        (WHITE, if focused { NAVY } else { SHADOW })
                    } else {
                        (BLACK, WHITE)
                    };
                    fill(buf, line, p, bg);
                    let mark = match (row.partial, row.checked) {
                        (true, _) => "[-]",
                        (_, true) => "[x]",
                        _ => "[ ]",
                    };
                    let mark_fg = if row.enabled { fg } else { SHADOW };
                    text(buf, inner.x, y, inner.width, mark, self.st(mark_fg, bg));
                    chip(buf, inner.x + 4, y, row.icon, p);
                    let detail = row.detail.chars().count() as u16;
                    let label_room = inner.width.saturating_sub(7 + detail + 1);
                    text(buf, inner.x + 6, y, label_room, &row.label, self.st(fg, bg));
                    let at = inner.right().saturating_sub(detail + 1).max(inner.x);
                    match lanlord_app::compat::parse_mark(&row.detail) {
                        Some((status, rest)) => {
                            let mark = lanlord_app::compat::mark(status);
                            text(buf, at, y, 1, mark, bold(self.st(mark_colour(status), bg)));
                            text(
                                buf,
                                at + 3,
                                y,
                                detail.saturating_sub(2),
                                rest,
                                self.st(fg, bg),
                            );
                        }
                        None => text(buf, at, y, detail + 1, &row.detail, self.st(fg, bg)),
                    }
                    self.push(line, path, Part::Row(n), false);
                    if row.enabled {
                        let box_rect = Rect::new(inner.x, y, 3.min(inner.width), 1);
                        self.push(box_rect, path, Part::Mark(n), false);
                    }
                }
            }
            Node::Description(description) => {
                let inner = self.sunken(area, buf, WHITE);
                for (n, line) in wrap(description, inner.width).iter().enumerate() {
                    if n as u16 >= inner.height {
                        break;
                    }
                    let style = self.st(BLACK, WHITE);
                    text(buf, inner.x, inner.y + n as u16, inner.width, line, style);
                }
            }
            Node::KeyedInput(input) => {
                let rect = Rect::new(area.x, area.y, cols(input.width).min(area.width), 1);
                self.field(&input.value, false, rect, buf, path);
            }
            Node::FieldError(t) => {
                text(buf, area.x, area.y, area.width, t, self.st(RED, GRAY));
            }
            Node::Labelled { label, body } => {
                let width = LABEL.min(area.width / 2);
                text(buf, area.x, area.y, width, label, self.st(BLACK, GRAY));
                let rest = Rect::new(area.x + width, area.y, area.width - width, area.height);
                self.draw(body, rest, buf, path);
            }
            Node::Disabled(body) => {
                let before = self.regions.len();
                self.draw(body, area, buf, path);
                // greyed out and not clickable, like the gui
                self.regions.truncate(before);
                let area = area.intersection(buf.area);
                for y in area.top()..area.bottom() {
                    for x in area.left()..area.right() {
                        let cell = &mut buf[(x, y)];
                        if cell.symbol().trim().is_empty() {
                            continue;
                        }
                        cell.set_fg(self.palette.color(SHADOW));
                    }
                }
            }
            Node::Fold { title, body, .. } => {
                let open = self.state.open.contains(&fold_key(path));
                let mark = if open { "[-] " } else { "[+] " };
                let line = format!("{mark}{title}");
                let width = (line.chars().count() as u16).min(area.width);
                let head = Rect::new(area.x, area.y, width, 1).intersection(buf.area);
                let s = if self.focused(path, Part::Fold) {
                    self.st(WHITE, NAVY)
                } else {
                    self.st(BLACK, GRAY)
                };
                text(buf, area.x, area.y, area.width, &line, s);
                self.push(head, path, Part::Fold, true);
                if open {
                    let inner = Rect::new(
                        area.x + 2,
                        area.y + 1,
                        area.width.saturating_sub(2),
                        area.height.saturating_sub(1),
                    );
                    self.draw(body, inner, buf, path);
                }
            }
            Node::Choice { current, width, .. } => {
                let w = cols(*width)
                    .max(current.chars().count() as u16 + 4)
                    .min(area.width);
                let rect = Rect::new(area.x, area.y, w, 1).intersection(buf.area);
                if rect.width >= 3 {
                    let focused = self.focused(path, Part::Open);
                    fill(buf, rect, p, WHITE);
                    let (fg, bg) = if focused {
                        (WHITE, NAVY)
                    } else {
                        (BLACK, WHITE)
                    };
                    text(
                        buf,
                        rect.x + 1,
                        rect.y,
                        rect.width.saturating_sub(3),
                        current,
                        self.st(fg, bg),
                    );
                    text(buf, rect.right() - 2, rect.y, 2, "▼", self.st(BLACK, GRAY));
                }
                self.push(rect, path, Part::Open, true);
            }
            Node::Tabs {
                titles,
                current,
                body,
                ..
            } => {
                let mut x = area.x;
                let bar = Rect::new(area.x, area.y, area.width, 1);
                self.push(bar.intersection(buf.area), path, Part::TabBar, true);
                let bar_focus = self.focused(path, Part::TabBar);
                for (n, title) in titles.iter().enumerate() {
                    let width =
                        (title.chars().count() as u16 + 4).min(area.right().saturating_sub(x));
                    let rect = Rect::new(x, area.y, width, 1).intersection(buf.area);
                    if !rect.is_empty() {
                        row_button(buf, rect, None, title, n != *current, p);
                        if n == *current {
                            recolour_text(buf, rect, if bar_focus { NAVY } else { BLACK }, p, true);
                        }
                        self.push(rect, path, Part::Tab(n), false);
                    }
                    x = x.saturating_add(width);
                }
                let page = Rect::new(
                    area.x,
                    area.y + 1,
                    area.width,
                    area.height.saturating_sub(1),
                );
                let page = page.intersection(buf.area);
                if page.height >= 2 {
                    Bevel {
                        sunken: false,
                        palette: p,
                    }
                    .render(page, buf);
                }
                let inner = Rect::new(
                    area.x + 2,
                    area.y + 2,
                    area.width.saturating_sub(4),
                    area.height.saturating_sub(3),
                );
                self.draw(body, inner, buf, path);
            }
            Node::Table(table) => {
                let inner = self.sunken(area, buf, WHITE);
                if inner.height == 0 {
                    return;
                }
                let widths: Vec<u16> = table.widths.iter().map(|w| cols(*w)).collect();
                let header = self.st(BLACK, GRAY);
                fill(buf, Rect::new(inner.x, inner.y, inner.width, 1), p, GRAY);
                let mut x = inner.x;
                for (n, heading) in table.headings.iter().enumerate() {
                    let w = widths
                        .get(n)
                        .copied()
                        .unwrap_or(inner.right().saturating_sub(x));
                    text(
                        buf,
                        x,
                        inner.y,
                        w.min(inner.right().saturating_sub(x)),
                        heading,
                        bold(header),
                    );
                    x = x.saturating_add(w);
                }
                let shown = inner.height.saturating_sub(1);
                let offset = controls::List::offset(table.selected, shown);
                self.push(inner, path, Part::List, true);
                let focused = self.focused(path, Part::List);
                for (n, row) in table
                    .rows
                    .iter()
                    .enumerate()
                    .skip(offset)
                    .take(usize::from(shown))
                {
                    let y = inner.y + 1 + (n - offset) as u16;
                    let selected = table.selected == Some(n);
                    let bad = table.bad.get(n).copied().unwrap_or(false);
                    let (fg, bg) = match (selected, bad) {
                        (true, _) => (WHITE, if focused { NAVY } else { SHADOW }),
                        (false, true) => (RED, WHITE),
                        (false, false) => (BLACK, WHITE),
                    };
                    let line = Rect::new(inner.x, y, inner.width, 1);
                    fill(buf, line, p, bg);
                    let mut x = inner.x;
                    for (c, cell) in row.iter().enumerate() {
                        let w = widths
                            .get(c)
                            .copied()
                            .unwrap_or(inner.right().saturating_sub(x));
                        let room = w.min(inner.right().saturating_sub(x));
                        match lanlord_app::compat::parse_mark(cell) {
                            Some((status, rest)) => {
                                let mark = lanlord_app::compat::mark(status);
                                text(
                                    buf,
                                    x,
                                    y,
                                    room,
                                    mark,
                                    bold(self.st(mark_colour(status), bg)),
                                );
                                if !rest.is_empty() {
                                    text(
                                        buf,
                                        x + 2,
                                        y,
                                        room.saturating_sub(2),
                                        rest,
                                        self.st(fg, bg),
                                    );
                                }
                            }
                            None => text(buf, x, y, room, cell, self.st(fg, bg)),
                        }
                        x = x.saturating_add(w);
                    }
                    self.push(line, path, Part::Row(n), false);
                }
            }
            Node::List {
                items,
                selected,
                empty,
                ..
            } => {
                let inner = self.sunken(area, buf, WHITE);
                self.push(inner, path, Part::List, true);
                if items.is_empty() {
                    text(
                        buf,
                        inner.x,
                        inner.y,
                        inner.width,
                        empty,
                        self.st(SHADOW, WHITE),
                    );
                    return;
                }
                let focused = self.focused(path, Part::List);
                let offset = controls::List::offset(*selected, inner.height);
                for (n, item) in items
                    .iter()
                    .enumerate()
                    .skip(offset)
                    .take(usize::from(inner.height))
                {
                    let y = inner.y + (n - offset) as u16;
                    let line = Rect::new(inner.x, y, inner.width, 1);
                    let (fg, bg) = if *selected == Some(n) {
                        (WHITE, if focused { NAVY } else { SHADOW })
                    } else {
                        (BLACK, WHITE)
                    };
                    fill(buf, line, p, bg);
                    text(buf, inner.x, y, inner.width, item, self.st(fg, bg));
                    self.push(line, path, Part::Row(n), false);
                }
            }
            Node::Lines { lines, .. } => {
                let inner = self.sunken(area, buf, WHITE);
                let key = scroll_key(path);
                self.scrolled_lines(
                    lines.iter().map(|l| (l.as_str(), BLACK)),
                    inner,
                    buf,
                    &key,
                    false,
                    WHITE,
                );
                self.push(inner, path, Part::Scroll, false);
            }
            Node::Log { lines, .. } => {
                let inner = self.sunken(area, buf, BLACK);
                let key = scroll_key(path);
                self.scrolled_lines(
                    lines.iter().map(|(k, l)| (l.as_str(), log_colour(*k))),
                    inner,
                    buf,
                    &key,
                    true,
                    BLACK,
                );
                self.push(inner, path, Part::Scroll, false);
            }
            Node::Code(code) => {
                let inner = self.sunken(area, buf, WHITE);
                for (n, line) in code.lines().enumerate().take(usize::from(inner.height)) {
                    text(
                        buf,
                        inner.x,
                        inner.y + n as u16,
                        inner.width,
                        line,
                        self.st(BLACK, WHITE),
                    );
                }
            }
            Node::TextArea { value, hint, .. } => {
                let inner = self.sunken(area, buf, WHITE);
                let focused = self.focused(path, Part::Field);
                if value.is_empty() && !focused {
                    text(
                        buf,
                        inner.x,
                        inner.y,
                        inner.width,
                        hint,
                        self.st(SHADOW, WHITE),
                    );
                }
                let lines: Vec<&str> = value.split('\n').collect();
                let first = lines.len().saturating_sub(usize::from(inner.height.max(1)));
                for (n, line) in lines.iter().enumerate().skip(first) {
                    text(
                        buf,
                        inner.x,
                        inner.y + (n - first) as u16,
                        inner.width,
                        line,
                        self.st(BLACK, WHITE),
                    );
                }
                if focused {
                    let last = lines.last().map_or(0, |l| l.chars().count()) as u16;
                    let y = inner.y + (lines.len() - 1 - first) as u16;
                    text(
                        buf,
                        inner.x + last.min(inner.width.saturating_sub(1)),
                        y,
                        1,
                        "▏",
                        self.st(BLACK, WHITE),
                    );
                }
                self.push(inner, path, Part::Field, true);
            }
            Node::Header { icon, title, lines } => {
                // the window's own icon as a desktop tile, its label left out
                let tile = Rect::new(area.x, area.y, 6, 3).intersection(buf.area);
                if !tile.is_empty() {
                    GlyphTile {
                        id: icon,
                        label: "",
                        selected: false,
                        palette: p,
                    }
                    .render(tile, buf);
                }
                let x = area.x + 8;
                let room = area.right().saturating_sub(x);
                text(buf, x, area.y, room, title, bold(self.st(BLACK, GRAY)));
                for (n, line) in lines.iter().enumerate() {
                    text(
                        buf,
                        x,
                        area.y + 1 + n as u16,
                        room,
                        line,
                        self.st(BLACK, GRAY),
                    );
                }
            }
            Node::Row(body) => {
                // a row that does not fit goes on to a second line, like text
                let mut y = area.y;
                for line in self.row_lines(body, area.width) {
                    let mut x = area.x;
                    let mut tallest = 1;
                    for (n, width) in line {
                        let here = child(path, n);
                        let height = self.node_height(&body[n], width, &here);
                        let rect = Rect::new(x, y, width, height);
                        self.node(&body[n], rect, buf, &here);
                        tallest = tallest.max(height);
                        x = x.saturating_add(width).saturating_add(1);
                    }
                    y = y.saturating_add(tallest);
                }
            }
            Node::Scroll { body, .. } => {
                let inner = self.sunken(area, buf, WHITE);
                self.push(inner, path, Part::Scroll, false);
                let total = self.height_at(body, inner.width, None, path);
                let offset = self
                    .state
                    .scroll
                    .get(&scroll_key(path))
                    .copied()
                    .unwrap_or(0)
                    .min(usize::from(total.saturating_sub(inner.height)))
                    as u16;
                if total <= inner.height {
                    self.draw(body, inner, buf, path);
                    return;
                }
                let mut tall = Buffer::empty(Rect::new(0, 0, inner.width, total));
                let whole = tall.area;
                fill(&mut tall, whole, p, WHITE);
                let mut inside = Painter::new(p, self.state, self.active);
                inside.draw(body, whole, &mut tall, path);
                blit(&tall, offset, inner, buf);
                for mut region in inside.regions {
                    if region.rect.y < offset || region.rect.bottom() > offset + inner.height {
                        continue;
                    }
                    region.rect.x += inner.x;
                    region.rect.y = region.rect.y - offset + inner.y;
                    self.regions.push(region);
                }
            }
        }
    }

    /// a row's nodes broken into lines that fit `width`: each is its index and its width. a
    /// node that cannot fit starts a new line; the flexible ones share what a line has left.
    fn row_lines<M>(&self, body: &[Node<M>], width: u16) -> Vec<Vec<(usize, u16)>> {
        let natural = self.row_widths(body, u16::MAX);
        let flexible = |n: &Node<M>| {
            !matches!(
                n,
                Node::Text(_)
                    | Node::Heading(_)
                    | Node::Warning(_)
                    | Node::Gap(_)
                    | Node::Button(_)
                    | Node::Buttons(_)
                    | Node::Toolbar(_)
                    | Node::Check { .. }
                    | Node::Choice { .. }
                    | Node::Field { .. }
                    | Node::Icon(_)
                    | Node::Spin { .. }
                    | Node::ClockFace { .. }
                    | Node::KeyedInput(_)
                    | Node::Input {
                        size: Size::Points(_),
                        ..
                    }
                    | Node::Secret {
                        size: Size::Points(_),
                        ..
                    }
            )
        };
        let mut lines: Vec<Vec<(usize, u16)>> = vec![Vec::new()];
        let mut used = 0u16;
        for (i, node) in body.iter().enumerate() {
            let w = if flexible(node) { 0 } else { natural[i] };
            let gap = u16::from(used > 0);
            if used > 0 && used + gap + w > width && !flexible(node) {
                lines.push(Vec::new());
                used = 0;
            }
            let gap = u16::from(used > 0);
            lines.last_mut().expect("a line").push((i, w));
            used += gap + w;
        }
        // a flexible node takes what its line has left
        for line in &mut lines {
            let fixed: u16 =
                line.iter().map(|(_, w)| *w).sum::<u16>() + line.len().saturating_sub(1) as u16;
            let open = line.iter().filter(|(i, _)| flexible(&body[*i])).count() as u16;
            let share = width.saturating_sub(fixed).checked_div(open).unwrap_or(0);
            for (i, w) in line.iter_mut() {
                if flexible(&body[*i]) {
                    *w = share;
                }
            }
        }
        lines
    }

    /// how wide each column is: one that asks for a width gets it, the rest share what is left.
    fn column_widths<M>(&self, columns: &[(Option<f32>, Vec<Node<M>>)], width: u16) -> Vec<u16> {
        let asked: Vec<Option<u16>> = columns.iter().map(|(w, _)| w.map(cols)).collect();
        let used: u16 =
            asked.iter().flatten().sum::<u16>() + columns.len().saturating_sub(1) as u16;
        let open = asked.iter().filter(|w| w.is_none()).count() as u16;
        let share = width.saturating_sub(used).checked_div(open).unwrap_or(0);
        asked.into_iter().map(|w| w.unwrap_or(share)).collect()
    }

    /// `nodes` of column `c`, one under another, with `c` packed into every node's path step.
    fn height_in<M>(&self, nodes: &[Node<M>], width: u16, path: &[usize], c: usize) -> u16 {
        // path already ends in the column's index (`child(path, c)`); the nodes under it step
        // as column * COLUMN + node, so replace that last element
        let mut base = path.to_vec();
        base.pop();
        nodes
            .iter()
            .enumerate()
            .map(|(n, node)| self.node_height(node, width, &child(&base, packed(c, n))))
            .sum()
    }

    /// draw the nodes of column `c` one under another.
    fn draw_in<M>(
        &mut self,
        nodes: &[Node<M>],
        area: Rect,
        buf: &mut Buffer,
        path: &[usize],
        c: usize,
    ) {
        let mut base = path.to_vec();
        base.pop();
        let mut y = area.y;
        for (n, node) in nodes.iter().enumerate() {
            let here = child(&base, packed(c, n));
            let height = self.node_height(node, area.width, &here);
            let rect = Rect::new(area.x, y, area.width, height);
            self.node(node, rect, buf, &here);
            y = y.saturating_add(height);
        }
    }

    /// how wide each node of a row is: fixed things their own width, fields what is left.
    fn row_widths<M>(&self, body: &[Node<M>], width: u16) -> Vec<u16> {
        let natural: Vec<Option<u16>> = body
            .iter()
            .map(|n| match n {
                Node::Text(t) | Node::Heading(t) | Node::Warning(t) => {
                    Some(t.chars().count() as u16)
                }
                Node::Gap(units) => Some(cols(*units)),
                Node::Button(b) => Some(button_width(&b.label)),
                Node::Buttons(bs) | Node::Toolbar(bs) => {
                    Some(bs.iter().map(|b| button_width(&b.label) + 1).sum())
                }
                Node::Check { label, .. } => Some(label.chars().count() as u16 + 4),
                Node::Input {
                    size: Size::Points(w),
                    ..
                }
                | Node::Secret {
                    size: Size::Points(w),
                    ..
                } => Some(cols(*w)),
                Node::Icon(_) => Some(1),
                Node::Spin { text, .. } => Some(spin_cells(text) + 4),
                Node::ClockFace { .. } => Some(8),
                Node::KeyedInput(input) => Some(cols(input.width)),
                Node::Choice { current, width, .. } => {
                    Some(cols(*width).max(current.chars().count() as u16 + 4))
                }
                Node::Field { label, value } => Some((label.len() + value.len()) as u16 + 2),
                _ => None,
            })
            .collect();
        let used: u16 = natural.iter().flatten().sum::<u16>() + body.len().saturating_sub(1) as u16;
        let flexible = natural.iter().filter(|n| n.is_none()).count() as u16;
        let share = width
            .saturating_sub(used)
            .checked_div(flexible)
            .unwrap_or(0);
        natural.into_iter().map(|n| n.unwrap_or(share)).collect()
    }

    fn button<M>(
        &mut self,
        button: &Button<M>,
        rect: Rect,
        buf: &mut Buffer,
        path: &[usize],
        n: usize,
    ) {
        let rect = rect.intersection(buf.area);
        if rect.is_empty() {
            return;
        }
        controls::Button {
            label: &button.label,
            pressed: false,
            default: self.focused(path, Part::Press(n)),
            enabled: button.enabled,
            palette: self.palette,
        }
        .render(rect, buf);
        if button.enabled {
            self.push(rect, path, Part::Press(n), true);
        }
    }

    fn field(&mut self, value: &str, masked: bool, rect: Rect, buf: &mut Buffer, path: &[usize]) {
        let rect = Rect::new(rect.x, rect.y, rect.width, 1).intersection(buf.area);
        if rect.width < 2 {
            return;
        }
        TextField {
            text: value,
            password: masked,
            focused: self.focused(path, Part::Field),
            palette: self.palette,
        }
        .render(rect, buf);
        self.push(rect, path, Part::Field, true);
    }

    /// a sunken box filled with `bg`; the area inside its edge.
    fn sunken(&self, area: Rect, buf: &mut Buffer, bg: Rgb) -> Rect {
        let area = area.intersection(buf.area);
        if area.width < 3 || area.height < 3 {
            return Rect::default();
        }
        Bevel {
            sunken: true,
            palette: self.palette,
        }
        .render(area, buf);
        let inner = Bevel::inner(area);
        fill(buf, inner, self.palette, bg);
        inner
    }

    /// lines in a box, scrolled by the wheel; `follow` keeps the newest line in view unless the
    /// user scrolled up.
    fn scrolled_lines<'l>(
        &self,
        lines: impl ExactSizeIterator<Item = (&'l str, Rgb)>,
        inner: Rect,
        buf: &mut Buffer,
        id: &str,
        follow: bool,
        bg: Rgb,
    ) {
        let total = lines.len();
        let shown = usize::from(inner.height);
        let last_start = total.saturating_sub(shown);
        let first = match self.state.scroll.get(id) {
            Some(at) => (*at).min(last_start),
            None if follow => last_start,
            None => 0,
        };
        for (n, (line, fg)) in lines.enumerate().skip(first).take(shown) {
            text(
                buf,
                inner.x,
                inner.y + (n - first) as u16,
                inner.width,
                line,
                self.st(fg, bg),
            );
        }
    }
}

/// how wide a field of `size` is in a row `room` cells wide.
fn size_cells(size: &Size, room: u16) -> u16 {
    match size {
        Size::Points(width) => cols(*width).min(room),
        Size::FillLess(less) => room.saturating_sub(cols(*less)).max(4).min(room),
    }
}

/// the colour of a status mark: green, red, amber or grey.
fn mark_colour(status: lanlord_app::compat::Status) -> Rgb {
    use lanlord_app::compat::Status;
    match status {
        Status::Ok => (0, 140, 0),
        Status::Fail => (200, 0, 0),
        Status::Warn => (176, 120, 0),
        Status::Unknown => (110, 110, 110),
    }
}

/// the yellow of the warning sign.
const YELLOW: Rgb = (255, 255, 0);

/// the step of a path into node `n` of column `c`.
fn packed(c: usize, n: usize) -> usize {
    c * COLUMN + n
}

/// how wide a spin box's field is: its text with a cell of room each side. the gui's widths
/// are in points for its font; in cells the text is all that matters.
fn spin_cells(text: &str) -> u16 {
    text.chars().count() as u16 + 2
}

/// the width of a calendar day, in cells.
const CELL_W: u16 = 5;

/// write `s` at (`x`, `y`) cut to `width`.
fn text_at(buf: &mut Buffer, x: u16, y: u16, width: u16, s: &str, st: Style) {
    text(buf, x, y, width, s, st);
}

/// the width of the label column in a form row, like the gui's.
const LABEL: u16 = 18;

/// a console without a height of its own takes the rest of the window.
fn fills<M>(node: &Node<M>) -> bool {
    matches!(node, Node::Log { height: None, .. })
}

/// `path` with `i` added.
fn child(path: &[usize], i: usize) -> Vec<usize> {
    let mut here = path.to_vec();
    here.push(i);
    here
}

/// the key a fold's open state is kept under: the key of its title.
pub fn fold_key(path: &[usize]) -> String {
    Region::new(Rect::default(), path, Part::Fold, true).key
}

/// the key a scrolling box keeps its offset under: the key of its scroll region.
pub fn scroll_key(path: &[usize]) -> String {
    Region::new(Rect::default(), path, Part::Scroll, false).key
}

/// a button's width in cells: its label with room for the edges, never tiny.
fn button_width(label: &str) -> u16 {
    (label.chars().count() as u16 + 4).max(8)
}

/// the console colours, like the gui's.
fn log_colour(kind: Kind) -> Rgb {
    match kind {
        Kind::Out => (192, 192, 192),
        Kind::Err => (255, 85, 85),
        Kind::Info => (85, 255, 255),
        Kind::Debug => SHADOW,
    }
}

/// the thin sunken edges of a one-row box.
fn edges(buf: &mut Buffer, area: Rect, palette: Palette) {
    let area = area.intersection(buf.area);
    if area.width < 2 {
        return;
    }
    buf[(area.x, area.y)]
        .set_symbol("▕")
        .set_style(style(palette, SHADOW, GRAY));
    buf[(area.right() - 1, area.y)]
        .set_symbol("▏")
        .set_style(style(palette, WHITE, GRAY));
}

/// an etched frame (a group box) or a coloured one, with an optional caption on its top edge.
fn frame_box(buf: &mut Buffer, area: Rect, palette: Palette, ink: Rgb, title: Option<&str>) {
    let area = area.intersection(buf.area);
    if area.width < 2 || area.height < 2 {
        return;
    }
    let s = style(palette, ink, GRAY);
    let (l, r, t, b) = (area.left(), area.right() - 1, area.top(), area.bottom() - 1);
    for x in l + 1..r {
        buf[(x, t)].set_symbol("─").set_style(s);
        buf[(x, b)].set_symbol("─").set_style(s);
    }
    for y in t + 1..b {
        buf[(l, y)].set_symbol("│").set_style(s);
        buf[(r, y)].set_symbol("│").set_style(s);
    }
    buf[(l, t)].set_symbol("┌").set_style(s);
    buf[(r, t)].set_symbol("┐").set_style(s);
    buf[(l, b)].set_symbol("└").set_style(s);
    buf[(r, b)].set_symbol("┘").set_style(s);
    if let Some(title) = title {
        let caption = format!(" {title} ");
        text(
            buf,
            l + 1,
            t,
            r.saturating_sub(l + 1),
            &caption,
            style(palette, BLACK, GRAY),
        );
    }
}

/// paint every cell of `rect` with `s`, keeping the symbols (greys out a disabled control).
fn recolour(buf: &mut Buffer, rect: Rect, s: Style) {
    for y in rect.top()..rect.bottom() {
        for x in rect.left()..rect.right() {
            buf[(x, y)].set_style(s);
        }
    }
}

/// make the label of a tab stand out: the current tab's text bold in `fg`.
fn recolour_text(buf: &mut Buffer, rect: Rect, fg: Rgb, palette: Palette, strong: bool) {
    let mut s = style(palette, fg, GRAY);
    if strong {
        s = bold(s);
    }
    for x in rect.left() + 1..rect.right().saturating_sub(1) {
        buf[(x, rect.y)].set_style(s);
    }
}

/// copy rows `from..` of `tall` into `area` of `buf`.
pub fn blit(tall: &Buffer, from: u16, area: Rect, buf: &mut Buffer) {
    for dy in 0..area.height {
        let sy = from + dy;
        if sy >= tall.area.bottom() {
            break;
        }
        for dx in 0..area.width.min(tall.area.width) {
            let (tx, ty) = (area.x + dx, area.y + dy);
            if buf.area.contains((tx, ty).into()) {
                buf[(tx, ty)] = tall[(dx, sy)].clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint::lines;
    use lanlord_app::view::Table;

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Msg {
        Ok,
        Cancel,
        Pick(usize),
        Tick,
        Typed(String),
        Entered(String),
        Secret(String),
        SecretDone(String),
        Tab(usize),
    }

    fn sample() -> Vec<Node<Msg>> {
        vec![
            Node::Heading("Accounts".into()),
            Node::Tabs {
                titles: vec!["General", "Advanced"],
                current: 0,
                pick: Msg::Tab,
                body: vec![
                    Node::List {
                        id: "people",
                        items: vec!["alice".into(), "bob".into()],
                        selected: Some(1),
                        min_height: 48.0,
                        empty: "nobody".into(),
                        select: Msg::Pick,
                    },
                    Node::Check {
                        label: "Enabled".into(),
                        checked: true,
                        enabled: true,
                        toggle: Msg::Tick,
                    },
                    Node::Input {
                        value: "demo".into(),
                        size: Size::FillLess(0.0),
                        focus: true,
                        hint: String::new(),
                        edit: Box::new(Msg::Typed),
                        enter: Some(Box::new(Msg::Entered)),
                    },
                ],
            },
            Node::Buttons(vec![
                Button::new("Cancel", true, Msg::Cancel),
                Button::new("OK", true, Msg::Ok),
            ]),
        ]
    }

    fn paint(nodes: &[Node<Msg>], state: &ViewState) -> (Buffer, Vec<Region>) {
        let area = Rect::new(0, 0, 40, 16);
        let mut buf = Buffer::empty(area);
        fill(&mut buf, area, Palette::default(), GRAY);
        let mut painter = Painter::new(Palette::default(), state, true);
        painter.draw(nodes, area, &mut buf, &[]);
        (buf, painter.regions)
    }

    #[test]
    fn a_tree_draws_its_tabs_list_and_buttons() {
        let (buf, _) = paint(&sample(), &ViewState::default());
        let text = lines(&buf).join("\n");
        assert!(text.contains("Accounts"), "{text}");
        assert!(
            text.contains("General") && text.contains("Advanced"),
            "{text}"
        );
        assert!(text.contains("alice") && text.contains("bob"), "{text}");
        assert!(text.contains("Enabled"), "{text}");
        assert!(text.contains("OK") && text.contains("Cancel"), "{text}");
    }

    #[test]
    fn clicks_on_regions_become_the_windows_messages() {
        let nodes = sample();
        let (_, regions) = paint(&nodes, &ViewState::default());
        let message = |part: Part| {
            let region = regions.iter().find(|r| r.part == part).expect("a region");
            let input = match part {
                Part::Press(n) => Input::Press(n),
                Part::Row(n) => Input::Row(n),
                Part::Tab(n) => Input::Tab(n),
                _ => Input::Toggle,
            };
            act(&nodes, &region.path, input)
        };
        assert_eq!(message(Part::Press(1)), Some(Msg::Ok));
        assert_eq!(message(Part::Press(0)), Some(Msg::Cancel));
        assert_eq!(message(Part::Row(0)), Some(Msg::Pick(0)));
        assert_eq!(message(Part::Tab(1)), Some(Msg::Tab(1)));
        assert_eq!(message(Part::Tab(0)), None, "the current tab sends nothing");
        assert_eq!(message(Part::Toggle), Some(Msg::Tick));
    }

    #[test]
    fn the_first_button_sits_rightmost() {
        let (_, regions) = paint(&sample(), &ViewState::default());
        let x = |n| {
            regions
                .iter()
                .find(|r| r.part == Part::Press(n))
                .unwrap()
                .rect
                .x
        };
        assert!(x(0) > x(1));
    }

    #[test]
    fn typing_into_a_field_sends_the_new_text() {
        let nodes = sample();
        let (_, regions) = paint(&nodes, &ViewState::default());
        let field = wants_focus(&nodes, &regions, &[]).expect("the entry asks for the cursor");
        let region = regions.iter().find(|r| r.key == field).unwrap();
        let Some(FieldValue::Plain { text: value, multi }) = field_value(&nodes, &region.path)
        else {
            panic!("a plain field");
        };
        assert!(!multi);
        assert_eq!(
            act(&nodes, &region.path, Input::Text(format!("{value}!"))),
            Some(Msg::Typed("demo!".into()))
        );
    }

    #[test]
    fn disabled_buttons_take_no_clicks() {
        let nodes: Vec<Node<Msg>> = vec![Node::Buttons(vec![Button::new("OK", false, Msg::Ok)])];
        let (_, regions) = paint(&nodes, &ViewState::default());
        assert!(regions.iter().all(|r| r.part != Part::Press(0)));
        assert_eq!(act(&nodes, &[0], Input::Press(0)), None);
    }

    #[test]
    fn status_marks_in_a_table_and_a_check_list_are_drawn_in_their_colours() {
        use lanlord_app::compat::{mark, Status};
        let all = [Status::Ok, Status::Fail, Status::Warn, Status::Unknown];
        let nodes: Vec<Node<Msg>> = vec![Node::Table(Table {
            id: "t",
            headings: vec!["", "Check"],
            widths: vec![20.0],
            rows: all
                .iter()
                .map(|s| vec![mark(*s).to_string(), "a check".into()])
                .collect(),
            selected: None,
            bad: Vec::new(),
            height: 80.0,
            select: Msg::Pick,
        })];
        let (buf, _) = paint(&nodes, &ViewState::default());
        for status in all {
            let want = style(Palette::default(), mark_colour(status), WHITE).fg;
            let cell = buf
                .content()
                .iter()
                .find(|c| c.symbol() == mark(status))
                .unwrap_or_else(|| panic!("no mark for {status:?}"));
            assert_eq!(cell.style().fg, want, "{status:?}");
        }
        assert_eq!(mark(Status::Ok), "\u{2713}");
        assert_eq!(mark(Status::Fail), "\u{2717}");
    }

    #[test]
    fn a_table_keeps_the_selection_in_view_and_rows_are_clickable() {
        let nodes: Vec<Node<Msg>> = vec![Node::Table(Table {
            id: "t",
            headings: vec!["Name", "Address"],
            widths: vec![70.0],
            rows: (0..20)
                .map(|n| vec![format!("pc{n}"), format!("192.0.2.{n}")])
                .collect(),
            selected: Some(15),
            bad: Vec::new(),
            height: 64.0,
            select: Msg::Pick,
        })];
        let (buf, regions) = paint(&nodes, &ViewState::default());
        let text = lines(&buf).join("\n");
        assert!(text.contains("Name") && text.contains("pc15"), "{text}");
        assert!(regions.iter().any(|r| r.part == Part::Row(15)));
        assert_eq!(act(&nodes, &[0], Input::Row(3)), Some(Msg::Pick(3)));
    }

    #[test]
    fn a_fold_opens_greyed_nodes_take_no_clicks_and_enter_sends_its_message() {
        let nodes: Vec<Node<Msg>> = vec![
            Node::Fold {
                id: "more",
                title: "More".into(),
                body: vec![Node::Text("hidden detail".into())],
            },
            Node::Disabled(vec![Node::Buttons(vec![Button::new("OK", true, Msg::Ok)])]),
            Node::Labelled {
                label: "Name:".into(),
                body: vec![Node::Text("value".into())],
            },
            Node::FieldError("too short".into()),
        ];
        let (buf, regions) = paint(&nodes, &ViewState::default());
        let text = lines(&buf).join("\n");
        assert!(
            text.contains("[+] More") && !text.contains("hidden detail"),
            "{text}"
        );
        assert!(text.contains("Name:") && text.contains("value") && text.contains("too short"));
        assert!(
            regions.iter().all(|r| r.part != Part::Press(0)),
            "greyed buttons are dead"
        );
        let mut state = ViewState::default();
        state.open.insert(fold_key(&[0]));
        let (buf, _) = paint(&nodes, &state);
        assert!(lines(&buf).join("\n").contains("hidden detail"));
        assert_eq!(
            act(&sample(), &[1, 2], Input::Enter("demo!".into())),
            Some(Msg::Entered("demo!".into())),
            "Enter carries the final text, typed in the same frame"
        );
    }

    fn secret(shown: bool) -> Vec<Node<Msg>> {
        vec![Node::Secret {
            value: Zeroizing::new("hunter2".into()),
            size: Size::Points(140.0),
            shown,
            focus: true,
            edit: Box::new(|text| Msg::Secret(text.to_string())),
            enter: Some(Box::new(|text| Msg::SecretDone(text.to_string()))),
        }]
    }

    #[test]
    fn a_secret_is_masked_until_shown_and_its_text_stays_zeroized() {
        let (buf, regions) = paint(&secret(false), &ViewState::default());
        let text = lines(&buf).join("\n");
        assert!(!text.contains("hunter2"), "{text}");
        assert!(
            text.contains('•') || text.contains('*') || text.contains('●'),
            "{text}"
        );
        let (buf, _) = paint(&secret(true), &ViewState::default());
        assert!(lines(&buf).join("\n").contains("hunter2"));
        // the field asks for the cursor and hands back a zeroized copy of what it holds
        let nodes = secret(false);
        assert!(wants_focus(&nodes, &regions, &[]).is_some());
        let Some(FieldValue::Secret(mut value)) = field_value(&nodes, &[0]) else {
            panic!("a secret field");
        };
        value.push('!');
        assert_eq!(
            act(&nodes, &[0], Input::SecretText(value.clone())),
            Some(Msg::Secret("hunter2!".into()))
        );
        assert_eq!(
            act(&nodes, &[0], Input::SecretEnter(value)),
            Some(Msg::SecretDone("hunter2!".into())),
            "Enter carries the text typed in the same frame"
        );
    }

    #[test]
    fn an_alert_shows_its_sign_and_a_field_error_its_line() {
        let nodes: Vec<Node<Msg>> = vec![
            Node::Alert("The gateway will restart.".into()),
            Node::FieldError("not an address".into()),
            Node::Icon("console"),
        ];
        let (buf, _) = paint(&nodes, &ViewState::default());
        let text = lines(&buf).join("\n");
        assert!(text.contains(" ! ") || text.contains("!"), "{text}");
        assert!(text.contains("The gateway will restart."), "{text}");
        assert!(text.contains("not an address"), "{text}");
        // the error line is red
        let y = lines(&buf)
            .iter()
            .position(|l| l.contains("not an address"))
            .unwrap() as u16;
        let x = lines(&buf)[y as usize].find("not").unwrap() as u16;
        assert_eq!(buf[(x, y)].fg, Palette::default().color(RED));
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Pick {
        Step(i32),
        Day(NaiveDate),
        Press,
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn calendar(enabled: bool) -> Vec<Node<Pick>> {
        vec![Node::Calendar {
            id: "cal",
            year: 2026,
            month: 11,
            selected: day(2026, 11, 16),
            today: day(2026, 11, 20),
            enabled,
            pick: Pick::Day,
        }]
    }

    fn spin(enabled: bool) -> Vec<Node<Pick>> {
        vec![Node::Spin {
            id: "hour",
            text: "11".into(),
            width: 56.0,
            enabled,
            step: Pick::Step,
        }]
    }

    fn painted<M>(nodes: &[Node<M>]) -> (String, Vec<Region>) {
        let area = Rect::new(0, 0, 60, 14);
        let mut buf = Buffer::empty(area);
        fill(&mut buf, area, Palette::default(), GRAY);
        let state = ViewState::default();
        let mut painter = Painter::new(Palette::default(), &state, true);
        painter.draw(nodes, area, &mut buf, &[]);
        (lines(&buf).join("\n"), painter.regions)
    }

    #[test]
    fn a_spin_box_shows_its_value_and_steps_with_its_arrows() {
        let nodes = spin(true);
        let (text, regions) = painted(&nodes);
        assert!(
            text.contains("11") && text.contains('▲') && text.contains('▼'),
            "{text}"
        );
        let up = regions
            .iter()
            .find(|r| r.part == Part::Spin(1))
            .expect("up arrow");
        let down = regions
            .iter()
            .find(|r| r.part == Part::Spin(-1))
            .expect("down arrow");
        assert!(up.rect.x < down.rect.x, "the up arrow comes first");
        assert_eq!(act(&nodes, &up.path, Input::Step(1)), Some(Pick::Step(1)));
        assert_eq!(
            act(&nodes, &down.path, Input::Step(-1)),
            Some(Pick::Step(-1))
        );
        // the box itself takes the keyboard and the wheel
        assert!(regions
            .iter()
            .any(|r| r.part == Part::Spin(0) && r.focusable));
        // a greyed-out spin box sends nothing and takes no clicks
        let off = spin(false);
        let (_, regions) = painted(&off);
        assert!(regions.iter().all(|r| !matches!(r.part, Part::Spin(_))));
        assert_eq!(act(&off, &[0], Input::Step(1)), None);
    }

    #[test]
    fn a_calendar_draws_the_month_marks_today_and_picks_a_day() {
        let nodes = calendar(true);
        let (text, regions) = painted(&nodes);
        // today (the 20th) is underlined, the selected day (the 16th) is highlighted
        let area = Rect::new(0, 0, 60, 14);
        let mut buf = Buffer::empty(area);
        let state = ViewState::default();
        let mut painter = Painter::new(Palette::default(), &state, true);
        painter.draw(&nodes, area, &mut buf, &[]);
        let today = painter
            .regions
            .iter()
            .find(|r| r.part == Part::Day(19))
            .unwrap()
            .rect;
        assert!(buf[(today.x + 1, today.y)]
            .modifier
            .contains(Modifier::UNDERLINED));
        let chosen = painter
            .regions
            .iter()
            .find(|r| r.part == Part::Day(15))
            .unwrap()
            .rect;
        assert_ne!(buf[(chosen.x, chosen.y)].bg, buf[(today.x, today.y)].bg);
        for needle in ["S", "M", "T", "W", "F", "16", "30", "20"] {
            assert!(text.contains(needle), "{needle} missing:\n{text}");
        }
        // the grid starts on the Sunday before the 1st: Nov 1 2026 is a Sunday, so cell 0
        let first = regions.iter().find(|r| r.part == Part::Day(0)).unwrap();
        assert_eq!(
            act(&nodes, &first.path, Input::Day(0)),
            Some(Pick::Day(day(2026, 11, 1)))
        );
        assert_eq!(
            act(&nodes, &first.path, Input::Day(15)),
            Some(Pick::Day(day(2026, 11, 16)))
        );
        assert_eq!(
            regions
                .iter()
                .filter(|r| matches!(r.part, Part::Day(_)))
                .count(),
            42
        );
        // read-only: no day takes a click
        let (_, regions) = painted(&calendar(false));
        assert!(regions
            .iter()
            .all(|r| !matches!(r.part, Part::Day(_) | Part::Days)));
        assert_eq!(act(&calendar(false), &[0], Input::Day(3)), None);
    }

    #[test]
    fn a_clock_face_prints_the_time_as_digits() {
        let nodes: Vec<Node<Pick>> = vec![Node::ClockFace {
            hour: 9,
            minute: 5,
            second: 7,
            size: 100.0,
        }];
        let (text, _) = painted(&nodes);
        assert!(text.contains("09:05:07"), "{text}");
    }

    #[test]
    fn columns_sit_side_by_side_and_their_buttons_still_send_their_messages() {
        let nodes: Vec<Node<Pick>> = vec![Node::Columns(vec![
            (
                Some(210.0),
                vec![
                    Node::Text("left side".into()),
                    Node::Calendar {
                        id: "c",
                        year: 2026,
                        month: 11,
                        selected: day(2026, 11, 16),
                        today: day(2026, 11, 20),
                        enabled: true,
                        pick: Pick::Day,
                    },
                ],
            ),
            (
                None,
                vec![
                    Node::Text("right side".into()),
                    Node::Button(Button::new("Go", true, Pick::Press)),
                ],
            ),
        ])];
        let (text, regions) = painted(&nodes);
        let rows: Vec<&str> = text.lines().collect();
        let both = rows.iter().find(|l| l.contains("left side")).unwrap();
        assert!(both.contains("right side"), "tops aligned:\n{text}");
        assert!(both.find("left side") < both.find("right side"));
        // a node inside the second column is found again through its packed path
        let go = regions
            .iter()
            .find(|r| r.part == Part::Press(0))
            .expect("the button");
        assert_eq!(act(&nodes, &go.path, Input::Press(0)), Some(Pick::Press));
        // and so is a day inside the first
        let day0 = regions
            .iter()
            .find(|r| r.part == Part::Day(15))
            .expect("a day");
        assert_eq!(
            act(&nodes, &day0.path, Input::Day(15)),
            Some(Pick::Day(day(2026, 11, 16)))
        );
    }

    #[test]
    fn long_text_wraps_at_spaces() {
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn a_console_follows_the_newest_line() {
        let nodes: Vec<Node<Msg>> = vec![Node::Log {
            id: "console",
            lines: (0..50).map(|n| (Kind::Info, format!("line {n}"))).collect(),
            height: None,
        }];
        let (buf, _) = paint(&nodes, &ViewState::default());
        let text = lines(&buf).join("\n");
        assert!(
            text.contains("line 49") && !text.contains("line 0 "),
            "{text}"
        );
    }

    #[test]
    fn wizard_nodes_draw_and_their_parts_send_messages() {
        use lanlord_app::view_wizard::{Banner, CheckList, CheckRow, KeyedInput};
        let row = |label: &str, checked, partial, enabled| CheckRow {
            icon: "dns",
            label: label.into(),
            detail: "1 MB".into(),
            checked,
            partial,
            enabled,
        };
        let nodes: Vec<Node<Msg>> = vec![
            Node::Banner(Banner {
                title: "Setup".into(),
                subtitle: "Pick the parts.".into(),
                icon: "settings",
            }),
            Node::CheckList(CheckList {
                id: "parts",
                rows: vec![
                    row("Gateway", true, false, false),
                    row("Proxy", false, false, true),
                    row("Domain", true, true, true),
                ],
                selected: Some(1),
                height: 64.0,
                select: Msg::Pick,
                toggle: |n| Msg::Typed(format!("toggle {n}")),
            }),
            Node::Description("A sunken box of text.".into()),
            Node::KeyedInput(KeyedInput {
                key: (2, 3),
                value: "192.0.2.5".into(),
                width: 120.0,
                edit: |key, text| Msg::Typed(format!("{key:?} {text}")),
            }),
        ];
        let (text, regions) = painted(&nodes);
        for needle in [
            "Setup",
            "Pick the parts.",
            "[x] ",
            "[ ] ",
            "[-] ",
            "Proxy",
            "1 MB",
            "A sunken box of text.",
            "192.0.2.5",
        ] {
            assert!(text.contains(needle), "{needle}:\n{text}");
        }
        assert_eq!(act(&nodes, &[1], Input::Row(2)), Some(Msg::Pick(2)));
        assert_eq!(
            act(&nodes, &[1], Input::Mark(1)),
            Some(Msg::Typed("toggle 1".into()))
        );
        assert_eq!(act(&nodes, &[1], Input::Mark(0)), None, "greyed rows stay");
        assert!(regions.iter().any(|r| r.part == Part::Mark(2)));
        assert!(regions.iter().all(|r| r.part != Part::Mark(0)));
        assert_eq!(list_state(&nodes, &[1]), Some((Some(1), 3)));
        assert_eq!(
            act(&nodes, &[3], Input::Text("192.0.2.6".into())),
            Some(Msg::Typed("(2, 3) 192.0.2.6".into()))
        );
        assert!(matches!(
            field_value(&nodes, &[3]),
            Some(FieldValue::Plain { multi: false, .. })
        ));
    }
}
