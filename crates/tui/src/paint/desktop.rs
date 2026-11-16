//! the teal desktop and its icons, laid out in columns from the top-left like the gui: each
//! icon is 8 by 4 cells (a cell holds two pixels stacked, so 8 by 8 pixels) with its label
//! under it, wrapped to two lines like the gui wraps "Remote Cmds". at that size the
//! hand-drawn 16 by 16 art (`art::small`) halved keeps its outlines far better than the
//! 32 by 32 art quartered, so desktop icons are drawn from the small art.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use super::art::Icon;
use super::palette::{Palette, NAVY, TEAL, WHITE};
use super::{fill, style, text};

/// the space one desktop icon takes: (columns, rows), in the gui's proportions.
pub const ICON_CELL: (u16, u16) = (13, 7);

/// how much the 16 by 16 art is shrunk by on the desktop.
pub const ICON_STEP: usize = 2;

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

/// split `label` into at most two lines of `width`, breaking at a space when it can.
fn wrap(label: &str, width: usize) -> Vec<String> {
    if label.chars().count() <= width {
        return vec![label.to_string()];
    }
    let cut = label
        .char_indices()
        .filter(|(i, c)| *c == ' ' && *i <= width)
        .map(|(i, _)| i)
        .next_back();
    match cut {
        Some(i) => vec![
            label[..i].to_string(),
            label[i + 1..].chars().take(width).collect(),
        ],
        None => vec![label.chars().take(width).collect()],
    }
}

/// one desktop icon, from the 16 by 16 art; a selected icon has its label in navy like the gui.
pub struct DesktopIcon<'a> {
    pub art: &'a [&'a str],
    pub label: &'a str,
    pub selected: bool,
    pub palette: Palette,
}

impl Widget for DesktopIcon<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let palette = self.palette;
        let icon = Icon {
            art: self.art,
            step: ICON_STEP,
            palette,
        };
        let (w, h) = icon.size();
        let x = area.x + area.width.saturating_sub(w) / 2;
        icon.render(Rect::new(x, area.y, w, h).intersection(area), buf);
        let bg = if self.selected { NAVY } else { TEAL };
        for (i, line) in wrap(self.label, area.width as usize - 1).iter().enumerate() {
            let width = line.chars().count() as u16;
            let lx = area.x + area.width.saturating_sub(width) / 2;
            let ly = area.y + h + i as u16;
            if ly < area.bottom() {
                text(buf, lx, ly, width, line, style(palette, WHITE, bg));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint::art::small;
    use crate::paint::palette::Depth;
    use ratatui::style::Color;

    #[test]
    fn icons_fill_columns_from_the_top_left() {
        let area = Rect::new(0, 0, 80, 24);
        assert_eq!(icon_rect(area, 0), Rect::new(1, 1, 13, 7));
        assert_eq!(icon_rect(area, 1), Rect::new(1, 8, 13, 7));
        assert_eq!(icon_rect(area, 2), Rect::new(1, 15, 13, 7));
        assert_eq!(icon_rect(area, 3), Rect::new(14, 1, 13, 7));
    }

    #[test]
    fn long_labels_wrap_at_a_space() {
        assert_eq!(wrap("Remote Cmds", 8), ["Remote", "Cmds"]);
        assert_eq!(wrap("DNS", 8), ["DNS"]);
        assert_eq!(wrap("Networking", 8), ["Networki"]);
    }

    #[test]
    fn an_icon_is_eight_by_four_cells_with_its_label_under_it() {
        let palette = Palette::new(Depth::TrueColor);
        let mut buf = Buffer::empty(Rect::new(0, 0, 13, 7));
        Desktop { palette }.render(buf.area, &mut buf);
        DesktopIcon {
            art: small("my-server"),
            label: "My Server",
            selected: true,
            palette,
        }
        .render(buf.area, &mut buf);
        let row =
            |y: u16| -> String { (0..13).map(|x| buf[(x, y)].symbol().to_string()).collect() };
        assert_eq!(row(4), "  My Server  ");
        assert_eq!(buf[(5, 4)].bg, Color::Rgb(0, 0, 128));
        assert_eq!(buf[(0, 4)].bg, Color::Rgb(0, 128, 128));
        let drawn = (2..10)
            .flat_map(|x| (0..4).map(move |y| (x, y)))
            .filter(|p| buf[*p].symbol() != " ")
            .count();
        assert!(drawn > 12, "the art is drawn ({drawn} cells)");
        assert!((0..2).all(|x| (0..4).all(|y| buf[(x, y)].symbol() == " ")));
    }
}
