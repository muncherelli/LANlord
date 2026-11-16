//! the common controls inside a window: push button, text field, checkbox and list. a control
//! one row tall gets thin edges on its first and last cell; three rows or more get the full
//! bevel like the gui.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::widgets::Widget;

use super::chrome::{row_button, Bevel};
use super::palette::{Palette, BLACK, GRAY, NAVY, SHADOW, WHITE};
use super::{bold, fill, style, text};

/// a push button; `default` draws the label bold, `pressed` sinks it.
pub struct Button<'a> {
    pub label: &'a str,
    pub pressed: bool,
    pub default: bool,
    pub enabled: bool,
    pub palette: Palette,
}

impl Widget for Button<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let palette = self.palette;
        if area.height < 3 {
            // one row: thin edges, the label centred like the gui's push buttons
            row_button(buf, area, None, "", self.pressed, palette);
            let fg = if self.enabled { BLACK } else { SHADOW };
            let mut label = style(palette, fg, GRAY);
            if self.default {
                label = bold(label);
            }
            let width = self.label.chars().count() as u16;
            let x = area.x + area.width.saturating_sub(width) / 2;
            text(
                buf,
                x,
                area.y,
                area.right().saturating_sub(x + 1),
                self.label,
                label,
            );
            return;
        }
        Bevel {
            sunken: self.pressed,
            palette,
        }
        .render(area, buf);
        let face = Bevel::inner(area);
        let fg = if self.enabled { BLACK } else { SHADOW };
        let mut label = style(palette, fg, GRAY);
        if self.default {
            label = bold(label);
        }
        let width = self.label.chars().count() as u16;
        // a pressed button's label moves down and right, like the gui's
        let nudge = u16::from(self.pressed);
        let x = face.x + face.width.saturating_sub(width) / 2 + nudge;
        let y = face.y + face.height / 2;
        text(buf, x, y, face.right().saturating_sub(x), self.label, label);
    }
}

/// a sunken white field; a password shows dots, focus shows the cursor block at the end.
pub struct TextField<'a> {
    pub text: &'a str,
    pub password: bool,
    pub focused: bool,
    pub palette: Palette,
}

impl TextField<'_> {
    /// the white part of a field drawn in `area`.
    pub fn field(area: Rect) -> Rect {
        if area.height >= 3 {
            let inner = Bevel::inner(area);
            Rect::new(inner.x, inner.y + inner.height / 2, inner.width, 1)
        } else {
            Rect::new(
                area.x.saturating_add(1),
                area.y,
                area.width.saturating_sub(2),
                area.height.min(1),
            )
        }
    }
}

/// the sunken edge of a one-row field or a tall one.
fn sunken_edge(buf: &mut Buffer, area: Rect, palette: Palette) {
    if area.height >= 3 {
        Bevel {
            sunken: true,
            palette,
        }
        .render(area, buf);
    } else if area.width >= 2 && area.height >= 1 {
        buf[(area.x, area.y)]
            .set_symbol("▕")
            .set_style(style(palette, SHADOW, GRAY));
        buf[(area.right() - 1, area.y)]
            .set_symbol("▏")
            .set_style(style(palette, WHITE, GRAY));
    }
}

impl Widget for TextField<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let palette = self.palette;
        sunken_edge(buf, area, palette);
        let field = TextField::field(area);
        fill(buf, field, palette, WHITE);
        let shown: String = if self.password {
            "•".repeat(self.text.chars().count())
        } else {
            self.text.to_string()
        };
        // keep the end of a long value in view, like a field scrolled to the cursor
        let room = field.width.saturating_sub(u16::from(self.focused) + 1) as usize;
        let skip = shown.chars().count().saturating_sub(room);
        let visible: String = shown.chars().skip(skip).collect();
        let face = style(palette, BLACK, WHITE);
        text(
            buf,
            field.x + 1,
            field.y,
            field.width.saturating_sub(1),
            &visible,
            face,
        );
        if self.focused {
            let x = field.x + 1 + visible.chars().count() as u16;
            if x < field.right() {
                buf[(x, field.y)].set_symbol("▏").set_style(face);
            }
        }
    }
}

/// a white box with a tick when checked, then the label.
pub struct Checkbox<'a> {
    pub label: &'a str,
    pub checked: bool,
    pub focused: bool,
    pub palette: Palette,
}

impl Widget for Checkbox<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 2 || area.height == 0 {
            return;
        }
        let palette = self.palette;
        buf[(area.x, area.y)]
            .set_symbol("▕")
            .set_style(style(palette, SHADOW, GRAY));
        let mark = if self.checked { "✓" } else { " " };
        buf[(area.x + 1, area.y)]
            .set_symbol(mark)
            .set_style(bold(style(palette, BLACK, WHITE)));
        let mut label = style(palette, BLACK, GRAY);
        if self.focused {
            label = label.add_modifier(Modifier::UNDERLINED);
        }
        text(
            buf,
            area.x + 3,
            area.y,
            area.width.saturating_sub(3),
            self.label,
            label,
        );
    }
}

/// a white sunken list; the selected row in navy, scrolled so the selection is visible.
pub struct List<'a> {
    pub rows: &'a [&'a str],
    pub selected: Option<usize>,
    pub palette: Palette,
}

impl List<'_> {
    /// the first row shown so that `selected` fits in `height` rows.
    pub fn offset(selected: Option<usize>, height: u16) -> usize {
        let height = usize::from(height.max(1));
        selected.map_or(0, |s| (s + 1).saturating_sub(height))
    }

    /// the rows area inside the edge.
    pub fn inner(area: Rect) -> Rect {
        Bevel::inner(area)
    }
}

impl Widget for List<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let palette = self.palette;
        Bevel {
            sunken: true,
            palette,
        }
        .render(area, buf);
        let inner = List::inner(area);
        fill(buf, inner, palette, WHITE);
        let offset = List::offset(self.selected, inner.height);
        for (i, row) in self
            .rows
            .iter()
            .enumerate()
            .skip(offset)
            .take(inner.height as usize)
        {
            let y = inner.y + (i - offset) as u16;
            let (fg, bg) = if self.selected == Some(i) {
                (WHITE, NAVY)
            } else {
                (BLACK, WHITE)
            };
            let line = Rect::new(inner.x, y, inner.width, 1);
            fill(buf, line, palette, bg);
            text(
                buf,
                inner.x + 1,
                y,
                inner.width.saturating_sub(1),
                row,
                style(palette, fg, bg),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint::lines;
    use crate::paint::palette::Depth;
    use ratatui::style::Color;

    fn palette() -> Palette {
        Palette::new(Depth::TrueColor)
    }

    #[test]
    fn a_tall_button_is_bevelled_and_centred() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 3));
        Button {
            label: "OK",
            pressed: false,
            default: true,
            enabled: true,
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(lines(&buf), ["▔▔▔▔▔▔▔▕", "▏  OK  ▕", "▏▁▁▁▁▁▁▁"]);
        assert!(buf[(3, 1)].modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn a_pressed_button_sinks_and_nudges_its_label() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 3));
        Button {
            label: "OK",
            pressed: true,
            default: false,
            enabled: true,
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(lines(&buf), [" ▁▁▁▁▁▁ ", "▕   OK ▏", " ▔▔▔▔▔▔ "]);
    }

    #[test]
    fn a_one_row_button_has_thin_edges() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        Button {
            label: "Cancel",
            pressed: false,
            default: false,
            enabled: true,
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(lines(&buf), ["▏ Cancel ▕"]);
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        Button {
            label: "OK",
            pressed: false,
            default: true,
            enabled: true,
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(lines(&buf), ["▏   OK   ▕"]);
        assert_eq!(buf[(0, 0)].fg, Color::Rgb(255, 255, 255));
    }

    #[test]
    fn a_password_field_shows_dots_and_the_cursor() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 9, 1));
        TextField {
            text: "abc",
            password: true,
            focused: true,
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(lines(&buf), ["▕ •••▏  ▏"]);
        assert_eq!(buf[(2, 0)].bg, Color::Rgb(255, 255, 255));
        assert_eq!(buf[(0, 0)].fg, Color::Rgb(128, 128, 128));
    }

    #[test]
    fn a_tall_field_is_sunken_around_its_white_line() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 3));
        TextField {
            text: "abc",
            password: false,
            focused: false,
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(lines(&buf), [" ▁▁▁▁▁▁ ", "▕ abc  ▏", " ▔▔▔▔▔▔ "]);
        assert_eq!(buf[(1, 1)].bg, Color::Rgb(255, 255, 255));
    }

    #[test]
    fn a_long_value_keeps_its_end_in_view() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 1));
        TextField {
            text: "gateway.example",
            password: false,
            focused: false,
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(lines(&buf), ["▕ ample▏"]);
    }

    #[test]
    fn a_checkbox_ticks_its_box() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 1));
        Checkbox {
            label: "Enabled",
            checked: true,
            focused: false,
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(lines(&buf), ["▕✓ Enabled  "]);
        assert_eq!(buf[(1, 0)].bg, Color::Rgb(255, 255, 255));
    }

    #[test]
    fn a_list_scrolls_to_the_selection() {
        let rows = ["alpha", "bravo", "charlie", "delta"];
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 4));
        List {
            rows: &rows,
            selected: Some(3),
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(
            lines(&buf),
            [" ▁▁▁▁▁▁▁▁ ", "▕ charlie▏", "▕ delta  ▏", " ▔▔▔▔▔▔▔▔ "]
        );
        assert_eq!(buf[(2, 2)].bg, Color::Rgb(0, 0, 128));
        assert_eq!(List::offset(None, 2), 0);
        assert_eq!(List::offset(Some(1), 2), 0);
    }
}
