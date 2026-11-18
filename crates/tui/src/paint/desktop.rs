//! the teal desktop and its icons, laid out in columns from the top-left like the gui. an
//! icon is one glyph in the accent colour on a small gray chip with the label under it,
//! wrapped to two lines like the gui wraps "Remote Cmds"; a selected icon has its label on
//! navy.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use super::glyphs::glyph_for;
use super::palette::{Palette, GRAY, NAVY, TEAL, WHITE};
use super::{bold, fill, style, text};

/// the size of the chip with the glyph: (columns, rows).
pub const TILE: (u16, u16) = (3, 1);

/// the space one desktop icon takes: (columns, rows): the chip, two label lines and a gap.
pub const ICON_CELL: (u16, u16) = (12, 4);

/// the plain teal background.
pub struct Desktop {
    pub palette: Palette,
}

impl Widget for Desktop {
    fn render(self, area: Rect, buf: &mut Buffer) {
        fill(buf, area, self.palette, TEAL);
    }
}

/// where icon `index` goes on a desktop of `area`, filling columns top to bottom.
pub fn icon_rect(area: Rect, index: usize) -> Rect {
    let per_column = (area.height.saturating_sub(1) / ICON_CELL.1).max(1) as usize;
    let column = (index / per_column) as u16;
    let row = (index % per_column) as u16;
    Rect::new(
        area.x + 1 + column * ICON_CELL.0,
        area.y + 1 + row * ICON_CELL.1,
        ICON_CELL.0,
        ICON_CELL.1,
    )
    .intersection(area)
}

/// split `label` into at most two lines of `width` characters, breaking at a space when it
/// can.
fn wrap(label: &str, width: usize) -> Vec<String> {
    let chars: Vec<char> = label.chars().collect();
    if chars.len() <= width {
        return vec![label.to_string()];
    }
    let cut = chars
        .iter()
        .enumerate()
        .take(width + 1)
        .filter(|(_, c)| **c == ' ')
        .map(|(i, _)| i)
        .next_back();
    match cut {
        Some(i) => vec![
            chars[..i].iter().collect(),
            chars[i + 1..].iter().take(width).collect(),
        ],
        None => vec![chars.iter().take(width).collect()],
    }
}

/// one desktop icon: a chip with the id's glyph, the label centred under it.
pub struct GlyphTile<'a> {
    pub id: &'a str,
    pub label: &'a str,
    pub selected: bool,
    pub palette: Palette,
}

impl GlyphTile<'_> {
    /// where the chip itself sits inside the icon's `area`: centred at the top.
    pub fn tile(area: Rect) -> Rect {
        Rect::new(
            area.x + area.width.saturating_sub(TILE.0) / 2,
            area.y,
            TILE.0.min(area.width),
            TILE.1.min(area.height),
        )
    }
}

impl Widget for GlyphTile<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let palette = self.palette;
        let tile = GlyphTile::tile(area);
        let glyph = glyph_for(self.id);
        fill(buf, tile, palette, GRAY);
        text(
            buf,
            tile.x + 1,
            tile.y,
            tile.width.saturating_sub(1),
            glyph.cell,
            bold(style(palette, glyph.accent, GRAY)),
        );
        let bg = if self.selected { NAVY } else { TEAL };
        let lines = wrap(self.label, (area.width as usize).saturating_sub(1));
        for (i, line) in lines.iter().enumerate() {
            let width = line.chars().count() as u16;
            let x = area.x + area.width.saturating_sub(width) / 2;
            let y = area.y + TILE.1 + i as u16;
            if y < area.bottom() {
                text(buf, x, y, width, line, style(palette, WHITE, bg));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint::lines;
    use crate::paint::palette::Depth;
    use ratatui::style::{Color, Modifier};

    fn palette() -> Palette {
        Palette::new(Depth::TrueColor)
    }

    #[test]
    fn icons_fill_columns_from_the_top_left() {
        let area = Rect::new(0, 0, 80, 24);
        assert_eq!(icon_rect(area, 0), Rect::new(1, 1, 12, 4));
        assert_eq!(icon_rect(area, 1), Rect::new(1, 5, 12, 4));
        assert_eq!(icon_rect(area, 4), Rect::new(1, 17, 12, 4));
        // five fit in 24 rows; the sixth starts the next column
        assert_eq!(icon_rect(area, 5), Rect::new(13, 1, 12, 4));
    }

    #[test]
    fn long_labels_wrap_at_a_space() {
        assert_eq!(wrap("Remote Cmds", 8), ["Remote", "Cmds"]);
        assert_eq!(wrap("DNS", 8), ["DNS"]);
        assert_eq!(wrap("Networking", 8), ["Networki"]);
        // characters, not bytes: a multi-byte label breaks at the same place
        assert_eq!(wrap("Résumé Été", 7), ["Résumé", "Été"]);
        assert_eq!(wrap("ÉÉÉÉÉÉÉÉÉÉ", 4), ["ÉÉÉÉ"]);
    }

    #[test]
    fn a_tile_shows_the_glyph_on_a_chip_with_the_label_under_it() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 4));
        Desktop { palette: palette() }.render(buf.area, &mut buf);
        GlyphTile {
            id: "remote-commands",
            label: "Remote Cmds",
            selected: true,
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(
            lines(&buf),
            [
                "     ▶      ",
                "Remote Cmds ",
                "            ",
                "            "
            ]
        );
        // the glyph in its accent colour on the gray chip, bold
        assert_eq!(buf[(5, 0)].fg, Color::Rgb(0, 128, 0));
        assert_eq!(buf[(5, 0)].bg, Color::Rgb(192, 192, 192));
        assert!(buf[(5, 0)].modifier.contains(Modifier::BOLD));
        // selected: the label sits on navy, the desktop stays teal
        assert_eq!(buf[(4, 1)].bg, Color::Rgb(0, 0, 128));
        assert_eq!(buf[(11, 1)].bg, Color::Rgb(0, 128, 128));
    }

    #[test]
    fn a_zero_width_icon_draws_nothing_and_does_not_panic() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 4));
        GlyphTile {
            id: "dns",
            label: "DNS",
            selected: false,
            palette: palette(),
        }
        .render(Rect::new(0, 0, 0, 4), &mut buf);
        GlyphTile {
            id: "dns",
            label: "DNS",
            selected: false,
            palette: palette(),
        }
        .render(Rect::new(2, 2, 12, 6), &mut buf);
    }
}
