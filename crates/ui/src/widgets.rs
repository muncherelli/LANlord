//! small drawing helpers shared by the management windows.

use eframe::egui::{Align2, Pos2, Rect, RichText, ScrollArea, Sense, Ui, Vec2};
use lanlord_model::settings::ValidationError;

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
}

impl Table<'_> {
    /// draw the table; returns the row clicked.
    pub fn show(&self, ui: &mut Ui) -> Option<usize> {
        let mut clicked = None;
        theme::sunken(ui, WHITE, |ui| {
            ui.set_width(ui.available_width());
            heading_row(ui, self.headings, self.widths);
            ScrollArea::vertical()
                .id_salt(self.id)
                .max_height(self.height)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height(self.height);
                    for (i, row) in self.rows.iter().enumerate() {
                        let flagged = self.bad.get(i).copied().unwrap_or(false);
                        let selected = self.selected == Some(i);
                        if data_row(ui, row, self.widths, selected, flagged).clicked() {
                            clicked = Some(i);
                        }
                    }
                });
        });
        clicked
    }
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
        ui.painter().with_clip_rect(cell.shrink(2.0)).text(
            cell.left_center() + Vec2::new(5.0, 0.0),
            Align2::LEFT_CENTER,
            *heading,
            theme::font(),
            BLACK,
        );
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
        ui.painter().with_clip_rect(column).text(
            rect.left_center() + Vec2::new(x + 5.0, 0.0),
            Align2::LEFT_CENTER,
            cell,
            theme::font(),
            colour,
        );
        x += width;
    }
    response
}

/// the messages of the errors at `path` (or below it, like `a.b[2]` for `a.b[2].c`).
pub fn messages(errors: &[ValidationError], path: &str) -> Vec<String> {
    errors
        .iter()
        .filter(|e| matches_path(&e.path, path))
        .map(|e| e.message.clone())
        .collect()
}

fn matches_path(error: &str, path: &str) -> bool {
    error == path
        || error
            .strip_prefix(path)
            .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('['))
}

/// the problem with the field at `path`, drawn small and red under it.
pub fn field_error(ui: &mut Ui, errors: &[ValidationError], path: &str) {
    for message in messages(errors, path) {
        ui.label(RichText::new(message).color(RED).small());
    }
}

/// for each of `count` rows, does any error sit under `{prefix}[i]`?
pub fn bad_rows(errors: &[ValidationError], prefix: &str, count: usize) -> Vec<bool> {
    (0..count)
        .map(|i| !messages(errors, &format!("{prefix}[{i}]")).is_empty())
        .collect()
}

/// a label in the form's label column and a line edit beside it.
pub fn labelled_edit(ui: &mut Ui, label: &str, text: &mut String, width: f32) {
    controls::row(ui, label, |ui| {
        controls::edit(ui, text, width);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(path: &str) -> ValidationError {
        ValidationError {
            path: path.into(),
            message: format!("bad {path}"),
        }
    }

    #[test]
    fn messages_match_a_path_and_what_is_below_it() {
        let errors = [
            error("dns.records[1].name"),
            error("dns.records[10].name"),
            error("dns.zone"),
            error("dns.dhcp.range[0]"),
        ];
        assert_eq!(messages(&errors, "dns.zone"), ["bad dns.zone"]);
        assert_eq!(messages(&errors, "dns.records[1]").len(), 1);
        assert_eq!(messages(&errors, "dns.dhcp.range").len(), 1);
        assert!(messages(&errors, "dns.zon").is_empty());
        assert_eq!(
            bad_rows(&errors, "dns.records", 3),
            [false, true, false],
            "[1] must not match [10]"
        );
    }
}
