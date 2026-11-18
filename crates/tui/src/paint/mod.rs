//! drawing blocks that make the terminal look like the gui: the teal desktop with its icon
//! tiles, gray windows boxed in single lines with the title on a navy top line, the taskbar, menus and the common
//! controls, plus mouse handling. each widget takes plain data, so a renderer of the shared
//! app layer can feed them later. every widget clips to the buffer, so a window dragged
//! partly off screen draws only what is visible.
//!
//! the glyphs used (box lines `┌ ─ │`, marks `✓ ▸ × □ •` and the icon glyphs in glyphs.rs) are one cell
//! wide in unicode, but some are "ambiguous width": a terminal set to draw ambiguous characters wide (a common cjk setting)
//! shifts everything after them on the row, so layouts look broken there.

pub mod chrome;
pub mod controls;
pub mod desktop;
pub mod glyphs;
pub mod mouse;
pub mod palette;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use palette::{Palette, Rgb};

/// the style for `fg` text on `bg` at this palette's depth.
pub fn style(palette: Palette, fg: Rgb, bg: Rgb) -> Style {
    Style::default().fg(palette.color(fg)).bg(palette.color(bg))
}

/// paint every cell of `area` (clipped to the buffer) blank in `bg`.
pub fn fill(buf: &mut Buffer, area: Rect, palette: Palette, bg: Rgb) {
    let area = area.intersection(buf.area);
    let style = style(palette, palette::BLACK, bg);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buf[(x, y)].set_symbol(" ").set_style(style);
        }
    }
}

/// write `text` at (`x`, `y`), cut to `width` cells and to the buffer.
pub fn text(buf: &mut Buffer, x: u16, y: u16, width: u16, text: &str, style: Style) {
    if !buf.area.contains((x, y).into()) {
        return;
    }
    let room = width.min(buf.area.right().saturating_sub(x));
    #[cfg(test)]
    if text.chars().count() > usize::from(room) {
        cut::note(text);
    }
    buf.set_stringn(x, y, text, room as usize, style);
}

/// set one cell's glyph and style, doing nothing when (`x`, `y`) is off the buffer.
pub fn put(buf: &mut Buffer, (x, y): (u16, u16), symbol: &str, style: Style) {
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_symbol(symbol).set_style(style);
    }
}

/// bold, for titles and default buttons.
pub fn bold(style: Style) -> Style {
    style.add_modifier(Modifier::BOLD)
}

/// the lines of a buffer area as plain text, for tests.
#[cfg(test)]
pub fn lines(buf: &Buffer) -> Vec<String> {
    (buf.area.top()..buf.area.bottom())
        .map(|y| {
            (buf.area.left()..buf.area.right())
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        })
        .collect()
}

/// which texts a test frame had to cut short, so a test can say that none was.
#[cfg(test)]
pub mod cut {
    use std::cell::RefCell;

    thread_local! {
        static CUT: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    pub fn note(text: &str) {
        CUT.with(|cut| cut.borrow_mut().push(text.to_string()));
    }

    /// the texts cut short since the last call.
    pub fn take() -> Vec<String> {
        CUT.with(|cut| std::mem::take(&mut *cut.borrow_mut()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_that_does_not_fit_is_cut_and_noted() {
        cut::take();
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        text(&mut buf, 0, 0, 4, "abcdef", Style::default());
        text(&mut buf, 0, 0, 6, "abcdef", Style::default());
        assert_eq!(cut::take(), ["abcdef"]);
    }
}
