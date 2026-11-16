//! drawing blocks that make the terminal look like the gui: the teal desktop with its pixel
//! icons, bevelled gray windows with navy title bars, the taskbar, menus and the common
//! controls, plus mouse handling. each widget takes plain data, so a renderer of the shared
//! app layer can feed them later.

pub mod art;
pub mod chrome;
pub mod controls;
pub mod desktop;
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
    buf.set_stringn(x, y, text, room as usize, style);
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
