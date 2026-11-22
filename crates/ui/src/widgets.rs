//! small drawing helpers shared by the management windows.

use eframe::egui::{Align2, Pos2, Rect, ScrollArea, Sense, Ui, Vec2};

use crate::controls;
use crate::remote::RED;
use crate::theme::{self, BLACK, NAVY, WHITE};

const ROW: f32 = 17.0;
const HEADING: f32 = 20.0;

/// a list with column headings. `widths` are in points, the last column
/// takes what is left. rows in `bad` are drawn red.
pub struct Table<'a> {
    pub id: &'a str,
    pub headings: &'a [&'a str],
    pub widths: &'a [f32],
    pub rows: &'a [Vec<String>],
    pub selected: Option<usize>,
    pub bad: &'a [bool],
    pub height: f32,
    /// takes none of the window's spare room.
    pub steady: bool,
}

impl Table<'_> {
    /// draw the table; returns the row clicked.
    pub fn show(&self, ui: &mut Ui) -> Option<usize> {
        let mut clicked = None;
        let height = if self.steady {
            self.height
        } else {
            controls::grow(ui, self.height)
        };
        // the columns are at least as wide as their widest text, so no cell is clipped
        let widths = fitted(ui, self.headings, self.rows, self.widths);
        theme::sunken(ui, WHITE, |ui| {
            ui.set_width(ui.available_width().max(widths.iter().sum()));
            heading_row(ui, self.headings, &widths);
            ScrollArea::vertical()
                .id_salt(self.id)
                .max_height(height)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height(height);
                    for (i, row) in self.rows.iter().enumerate() {
                        let flagged = self.bad.get(i).copied().unwrap_or(false);
                        let selected = self.selected == Some(i);
                        if data_row(ui, row, &widths, selected, flagged).clicked() {
                            clicked = Some(i);
                        }
                    }
                });
        });
        clicked
    }
}

/// room either side of a cell's text.
const CELL_PAD: f32 = 5.0;

/// the most a table's last column is widened to fit its text.
const LAST_FIT: f32 = 140.0;

/// `widths` grown to hold the widest heading or cell of each column, measured in the real font;
/// the last column, which takes what is left, is given what its text needs up to `LAST_FIT`: a
/// column of long text is for the window to give room to, not to widen it.
fn fitted(ui: &Ui, headings: &[&str], rows: &[Vec<String>], widths: &[f32]) -> Vec<f32> {
    let text = |s: &str| {
        let (mark, s) = lanlord_model::compat::parse_mark(s).map_or((0.0, s), |(_, rest)| {
            (crate::compat_alert::MARK + 4.0, rest)
        });
        let galley = ui
            .painter()
            .layout_no_wrap(s.to_string(), theme::font(), BLACK);
        mark + galley.size().x + 2.0 * CELL_PAD
    };
    let columns = headings
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    (0..columns)
        .map(|i| {
            let widest = headings
                .get(i)
                .map(|h| text(h))
                .into_iter()
                .chain(rows.iter().filter_map(|r| r.get(i)).map(|c| text(c)))
                .fold(0.0, f32::max);
            let last = i + 1 == columns;
            let set = widths.get(i).copied().filter(|_| !last).unwrap_or(0.0);
            let widest = if last { widest.min(LAST_FIT) } else { widest };
            set.max(widest).ceil()
        })
        .collect()
}

fn heading_row(ui: &mut Ui, headings: &[&str], widths: &[f32]) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), HEADING), Sense::hover());
    let mut x = rect.left();
    for (i, heading) in headings.iter().enumerate() {
        let last = i + 1 == headings.len();
        let width = if last {
            rect.right() - x
        } else {
            widths.get(i).copied().unwrap_or(0.0)
        };
        let cell = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(width, HEADING));
        ui.painter().rect_filled(cell, 0.0, theme::GRAY);
        theme::bevel(ui.painter(), cell, true);
        let _drawn = ui.painter().with_clip_rect(cell.shrink(2.0)).text(
            cell.left_center() + Vec2::new(CELL_PAD, 0.0),
            Align2::LEFT_CENTER,
            *heading,
            theme::font(),
            BLACK,
        );
        #[cfg(test)]
        theme::probe::record_cell(ui, heading, _drawn, cell);
        x += width;
    }
}

fn data_row(
    ui: &mut Ui,
    cells: &[String],
    widths: &[f32],
    selected: bool,
    flagged: bool,
) -> eframe::egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
    if selected {
        ui.painter().rect_filled(rect, 0.0, NAVY);
    }
    let colour = match (selected, flagged) {
        (true, _) => WHITE,
        (false, true) => RED,
        _ => BLACK,
    };
    let mut x = 0.0;
    for (i, cell) in cells.iter().enumerate() {
        let width = widths.get(i).copied().unwrap_or(0.0);
        // a cell never paints past its own column, however long its text is
        let right = if i + 1 == cells.len() {
            rect.right()
        } else {
            rect.left() + x + width
        };
        let column = Rect::from_min_max(
            Pos2::new(rect.left() + x, rect.top()),
            Pos2::new(right, rect.bottom()),
        );
        let mut text_x = x + CELL_PAD;
        let mut shown = cell.as_str();
        if let Some((status, rest)) = lanlord_model::compat::parse_mark(cell) {
            let at = rect.left_center() + Vec2::new(x + 5.0 + crate::compat_alert::MARK / 2.0, 0.0);
            crate::compat_alert::paint_mark(&ui.painter().with_clip_rect(column), at, status);
            text_x += crate::compat_alert::MARK + 4.0;
            shown = rest;
        }
        let _drawn = ui.painter().with_clip_rect(column).text(
            rect.left_center() + Vec2::new(text_x, 0.0),
            Align2::LEFT_CENTER,
            shown,
            theme::font(),
            colour,
        );
        #[cfg(test)]
        theme::probe::record_cell(ui, shown, _drawn, column);
        x += width;
    }
    response
}
