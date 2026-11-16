//! draws the app's layout tree (`lanlord_app::view`) in character cells with the paint
//! toolkit, the way `lanlord-ui` draws it with egui, and turns clicks and keys back into the
//! window's messages. nothing here knows what a window is for: every window that describes
//! itself as a tree shows up in the terminal through this one file.

use std::collections::HashMap;

use lanlord_app::view::{Button, Kind, Node};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

use crate::paint::art::Icon;
use crate::paint::chrome::{row_button, Bevel};
use crate::paint::controls::{self, Checkbox, TextField};
use crate::paint::palette::{Palette, Rgb, BLACK, GRAY, NAVY, SHADOW, WHITE};
use crate::paint::{art, bold, fill, style, text};

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
}

impl ViewState {
    pub fn scroll_by(&mut self, key: &str, lines: i32) {
        let at = self.scroll.entry(key.to_string()).or_insert(0);
        *at = (*at as i32 + lines).max(0) as usize;
    }
}

/// what the user did to a node, ready to become the window's message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    Press(usize),
    Toggle,
    Row(usize),
    Tab(usize),
    Pick(usize),
    /// the field's whole new text.
    Text(String),
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
        Node::Danger(body) | Node::Row(body) => find(body, rest),
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
        (Node::Input { value, edit, .. }, Input::Text(text))
        | (Node::TextArea { value, edit, .. }, Input::Text(text)) => {
            (text != *value).then(|| edit(text))
        }
        (Node::Entry { value, edit, .. }, Input::Text(text)) => {
            (text != *value).then(|| edit(text))
        }
        _ => None,
    }
}

/// the text a field holds now, and whether it takes several lines.
pub fn field_value<M>(nodes: &[Node<M>], path: &[usize]) -> Option<(String, bool)> {
    match find(nodes, path)? {
        Node::Input { value, .. } | Node::Entry { value, .. } => Some((value.clone(), false)),
        Node::TextArea { value, .. } => Some((value.clone(), true)),
        _ => None,
    }
}

/// the selected row of a list or table and how many rows it has.
pub fn list_state<M>(nodes: &[Node<M>], path: &[usize]) -> Option<(Option<usize>, usize)> {
    match find(nodes, path)? {
        Node::Table(table) => Some((table.selected, table.rows.len())),
        Node::List {
            items, selected, ..
        } => Some((*selected, items.len())),
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

/// the first field that asks for the cursor (`Entry { focus: true }`), as a region key.
pub fn wants_focus<M>(nodes: &[Node<M>], regions: &[Region]) -> Option<String> {
    regions
        .iter()
        .filter(|r| r.part == Part::Field)
        .find(|r| matches!(find(nodes, &r.path), Some(Node::Entry { focus: true, .. })))
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
        let fixed: u16 = nodes
            .iter()
            .filter(|n| !matches!(n, Node::Log { .. }))
            .map(|n| self.node_height(n, width))
            .sum();
        let logs = nodes
            .iter()
            .filter(|n| matches!(n, Node::Log { .. }))
            .count() as u16;
        let log = match room {
            Some(room) if logs > 0 => (room.saturating_sub(fixed) / logs).max(4),
            _ => 8,
        };
        fixed + logs * log
    }

    fn node_height<M>(&self, node: &Node<M>, width: u16) -> u16 {
        match node {
            Node::Text(text) => wrap(text, width).len() as u16,
            Node::Gap(height) => rows(*height),
            Node::Group { body, .. } => 2 + self.height(body, width.saturating_sub(4), None),
            Node::Table(table) => rows(table.height).max(3) + 2,
            Node::Tabs { body, .. } => 3 + self.height(body, width.saturating_sub(4), None),
            Node::Danger(body) => 2 + self.height(body, width.saturating_sub(4), None),
            Node::Lines { height, .. } => rows(*height).max(2) + 2,
            Node::Log { .. } => 8,
            Node::List {
                items, min_height, ..
            } => {
                let least = rows(*min_height).max(2);
                (items.len() as u16).clamp(least, least.max(8)) + 2
            }
            Node::Header { lines, .. } => (1 + lines.len() as u16).max(4) + 1,
            Node::Code(text) => text.lines().count() as u16 + 2,
            Node::Row(body) => body
                .iter()
                .map(|n| self.node_height(n, width))
                .max()
                .unwrap_or(0),
            Node::TextArea { height, .. } => rows(*height).max(3) + 2,
            Node::Scroll {
                max_height, body, ..
            } => {
                let inner = self.height(body, width.saturating_sub(2), None);
                inner.min(rows(*max_height).max(3)) + 2
            }
            _ => 1,
        }
    }

    /// draw `nodes` top to bottom inside `area`.
    pub fn draw<M>(&mut self, nodes: &[Node<M>], area: Rect, buf: &mut Buffer, path: &[usize]) {
        let logs = nodes
            .iter()
            .filter(|n| matches!(n, Node::Log { .. }))
            .count() as u16;
        let without: u16 = nodes
            .iter()
            .filter(|n| !matches!(n, Node::Log { .. }))
            .map(|n| self.node_height(n, area.width))
            .sum();
        let log_rows = area
            .height
            .saturating_sub(without)
            .checked_div(logs)
            .map_or(0, |rows| rows.max(4));
        let mut y = area.y;
        for (i, node) in nodes.iter().enumerate() {
            let height = if matches!(node, Node::Log { .. }) {
                log_rows
            } else {
                self.node_height(node, area.width)
            };
            let rect = Rect::new(area.x, y, area.width, height);
            let mut here = path.to_vec();
            here.push(i);
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
                let label_width = 18.min(area.width / 2);
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
            Node::Input { value, width, .. } => {
                let rect = Rect::new(area.x, area.y, cols(*width).min(area.width), 1);
                self.field(value, false, rect, buf, path);
            }
            Node::Entry { value, masked, .. } => {
                self.field(value, *masked, area, buf, path);
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
                    let (fg, bg) = if selected {
                        (WHITE, if focused { NAVY } else { SHADOW })
                    } else {
                        (BLACK, WHITE)
                    };
                    let line = Rect::new(inner.x, y, inner.width, 1);
                    fill(buf, line, p, bg);
                    let mut x = inner.x;
                    for (c, cell) in row.iter().enumerate() {
                        let w = widths
                            .get(c)
                            .copied()
                            .unwrap_or(inner.right().saturating_sub(x));
                        text(
                            buf,
                            x,
                            y,
                            w.min(inner.right().saturating_sub(x)),
                            cell,
                            self.st(fg, bg),
                        );
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
            Node::TextArea { value, .. } => {
                let inner = self.sunken(area, buf, WHITE);
                let focused = self.focused(path, Part::Field);
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
                let icon_area = Rect::new(area.x, area.y, 8, 4).intersection(buf.area);
                if !icon_area.is_empty() {
                    Icon {
                        art: art::small(icon),
                        step: 2,
                        palette: p,
                    }
                    .render(icon_area, buf);
                }
                let x = area.x + 10;
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
                let widths = self.row_widths(body, area.width);
                let mut x = area.x;
                for (n, (child, width)) in body.iter().zip(widths).enumerate() {
                    let mut here = path.to_vec();
                    here.push(n);
                    let rect = Rect::new(x, area.y, width, area.height);
                    self.node(child, rect, buf, &here);
                    x = x.saturating_add(width).saturating_add(1);
                }
            }
            Node::Scroll { body, .. } => {
                let inner = self.sunken(area, buf, WHITE);
                self.push(inner, path, Part::Scroll, false);
                let total = self.height(body, inner.width, None);
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
                Node::Input { width, .. } => Some(cols(*width)),
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
                    Node::Entry {
                        value: "demo".into(),
                        room: 0.0,
                        masked: false,
                        focus: true,
                        edit: Box::new(Msg::Typed),
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
        let field = wants_focus(&nodes, &regions).expect("the entry asks for the cursor");
        let region = regions.iter().find(|r| r.key == field).unwrap();
        let (value, multi) = field_value(&nodes, &region.path).unwrap();
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
    fn a_table_keeps_the_selection_in_view_and_rows_are_clickable() {
        let nodes: Vec<Node<Msg>> = vec![Node::Table(Table {
            id: "t",
            headings: vec!["Name", "Address"],
            widths: vec![70.0],
            rows: (0..20)
                .map(|n| vec![format!("pc{n}"), format!("192.0.2.{n}")])
                .collect(),
            selected: Some(15),
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
    fn long_text_wraps_at_spaces() {
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn a_console_follows_the_newest_line() {
        let nodes: Vec<Node<Msg>> = vec![Node::Log {
            id: "console",
            lines: (0..50).map(|n| (Kind::Info, format!("line {n}"))).collect(),
        }];
        let (buf, _) = paint(&nodes, &ViewState::default());
        let text = lines(&buf).join("\n");
        assert!(
            text.contains("line 49") && !text.contains("line 0 "),
            "{text}"
        );
    }
}
