//! the common controls inside a window: push button, text field, checkbox and list. a button is
//! `[ label ]`; a field or list is a lighter patch with no frame.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::widgets::Widget;

use super::chrome::{row_button, Frame};
use super::palette::{Palette, BLACK, GRAY, NAVY, SHADOW, WHITE};
use super::{bold, fill, put, style, text};

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
            let (fg, bg) = match (self.enabled, self.pressed) {
                (false, _) => (SHADOW, GRAY),
                (true, true) => (WHITE, NAVY),
                (true, false) => (BLACK, GRAY),
            };
            let mut label = style(palette, fg, bg);
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
        Frame { palette }.render(area, buf);
        let face = Frame::inner(area);
        let (fg, bg) = match (self.enabled, self.pressed) {
            (false, _) => (SHADOW, GRAY),
            (true, true) => (WHITE, NAVY),
            (true, false) => (BLACK, GRAY),
        };
        fill(buf, face, palette, bg);
        let mut label = style(palette, fg, bg);
        if self.default {
            label = bold(label);
        }
        let width = self.label.chars().count() as u16;
        let x = face.x + face.width.saturating_sub(width) / 2;
        let y = face.y + face.height / 2;
        text(buf, x, y, face.right().saturating_sub(x), self.label, label);
    }
}

/// a white field with no frame; a password shows dots, focus shows the cursor block at the end.
pub struct TextField<'a> {
    pub text: &'a str,
    pub password: bool,
    pub focused: bool,
    pub palette: Palette,
}

impl TextField<'_> {
    /// the white part of a field drawn in `area`: its middle row.
    pub fn field(area: Rect) -> Rect {
        Rect::new(
            area.x,
            area.y.saturating_add(area.height.saturating_sub(1) / 2),
            area.width,
            area.height.min(1),
        )
    }
}

impl Widget for TextField<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let palette = self.palette;
        let field = TextField::field(area);
        fill(buf, field, palette, WHITE);
        let shown: String = if self.password {
            "•".repeat(self.text.chars().count())
        } else {
            self.text.to_string()
        };
        // keep the end of a long value in view, like a field scrolled to the cursor
        let room = field.width.saturating_sub(u16::from(self.focused) + 2) as usize;
        let skip = shown.chars().count().saturating_sub(room);
        let visible: String = shown.chars().skip(skip).collect();
        let face = style(palette, BLACK, WHITE);
        text(
            buf,
            field.x + 1,
            field.y,
            field.width.saturating_sub(2),
            &visible,
            face,
        );
        if self.focused {
            let x = field.x + 1 + visible.chars().count() as u16;
            if x < field.right() {
                put(buf, (x, field.y), " ", style(palette, WHITE, BLACK));
            }
        }
    }
}

/// `[✓]` or `[ ]`, then the label.
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
        let mark = if self.checked { "[✓]" } else { "[ ]" };
        text(
            buf,
            area.x,
            area.y,
            3,
            mark,
            bold(style(palette, BLACK, GRAY)),
        );
        let mut label = style(palette, BLACK, GRAY);
        if self.focused {
            label = label.add_modifier(Modifier::UNDERLINED);
        }
        text(
            buf,
            area.x + 4,
            area.y,
            area.width.saturating_sub(4),
            self.label,
            label,
        );
    }
}

/// a white list with no frame; the selected row in navy, scrolled so the selection is visible.
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
}

impl Widget for List<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let palette = self.palette;
        let inner = area;
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
    fn controls_clip_to_the_buffer() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 6, 3));
        let off = Rect::new(4, 1, 20, 5);
        TextField {
            text: "abc",
            password: false,
            focused: true,
            palette: palette(),
        }
        .render(off, &mut buf);
        TextField {
            text: "abc",
            password: true,
            focused: true,
            palette: palette(),
        }
        .render(Rect::new(5, 2, 9, 1), &mut buf);
        Checkbox {
            label: "Enabled",
            checked: true,
            focused: true,
            palette: palette(),
        }
        .render(Rect::new(5, 2, 12, 1), &mut buf);
        List {
            rows: &["a", "b"],
            selected: Some(1),
            palette: palette(),
        }
        .render(off, &mut buf);
        Button {
            label: "OK",
            pressed: true,
            default: true,
            enabled: true,
            palette: palette(),
        }
        .render(off, &mut buf);
    }

    #[test]
    fn a_tall_button_is_boxed_and_centred() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 3));
        Button {
            label: "OK",
            pressed: false,
            default: true,
            enabled: true,
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(lines(&buf), ["┌──────┐", "│  OK  │", "└──────┘"]);
        assert!(buf[(3, 1)].modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn a_pressed_button_is_navy() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 3));
        Button {
            label: "OK",
            pressed: true,
            default: false,
            enabled: true,
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(lines(&buf), ["┌──────┐", "│  OK  │", "└──────┘"]);
        assert_eq!(buf[(3, 1)].bg, Color::Rgb(0, 0, 128));
    }

    #[test]
    fn a_one_row_button_has_brackets() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        Button {
            label: "Cancel",
            pressed: false,
            default: false,
            enabled: true,
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(lines(&buf), ["[ Cancel ]"]);
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        Button {
            label: "OK",
            pressed: false,
            default: true,
            enabled: true,
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(lines(&buf), ["[   OK   ]"]);
        assert_eq!(buf[(0, 0)].fg, Color::Rgb(0, 0, 0));
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
        assert_eq!(lines(&buf), [" •••     "]);
        assert_eq!(buf[(2, 0)].bg, Color::Rgb(255, 255, 255));
        assert_eq!(buf[(4, 0)].bg, Color::Rgb(0, 0, 0), "the cursor is a block");
    }

    #[test]
    fn a_tall_field_is_its_white_middle_line() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 3));
        TextField {
            text: "abc",
            password: false,
            focused: false,
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(lines(&buf), ["        ", " abc    ", "        "]);
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
        assert_eq!(lines(&buf), [" xample "]);
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
        assert_eq!(lines(&buf), ["[✓] Enabled "]);
        assert_eq!(buf[(1, 0)].fg, Color::Rgb(0, 0, 0));
        assert!(buf[(1, 0)].modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn a_list_scrolls_to_the_selection() {
        let rows = ["alpha", "bravo", "charlie", "delta"];
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 2));
        List {
            rows: &rows,
            selected: Some(3),
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(lines(&buf), [" charlie  ", " delta    "]);
        assert_eq!(buf[(2, 1)].bg, Color::Rgb(0, 0, 128));
        assert_eq!(List::offset(None, 2), 0);
        assert_eq!(List::offset(Some(1), 2), 0);
    }
}
