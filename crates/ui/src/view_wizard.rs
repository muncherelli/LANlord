//! draws the wizard pieces of the layout tree (`lanlord_app::view_wizard`) with egui: the white
//! band across the top of a page, the list of tick boxes with icons, the sunken description and
//! keyed text fields.

use eframe::egui::{self, Align2, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, Ui, Vec2};
use lanlord_app::view_wizard::{Banner, CheckList, KeyedInput};

use crate::controls;
use crate::icons;
use crate::theme::{self, bevel, BLACK, GRAY, NAVY, SHADOW, WHITE};

const BOX: f32 = 13.0;
const ROW: f32 = 18.0;

/// the white band with a bold title, the line under it and the icon on the right.
pub fn banner(ui: &mut Ui, banner: &Banner) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 48.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, WHITE);
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, SHADOW),
    );
    painter.text(
        rect.left_top() + Vec2::new(12.0, 9.0),
        Align2::LEFT_TOP,
        &banner.title,
        egui::FontId::proportional(theme::TEXT),
        BLACK,
    );
    painter.text(
        rect.left_top() + Vec2::new(24.0, 27.0),
        Align2::LEFT_TOP,
        &banner.subtitle,
        theme::font(),
        BLACK,
    );
    // bold by drawing the title twice, one point apart, as the nt wizards did
    painter.text(
        rect.left_top() + Vec2::new(13.0, 9.0),
        Align2::LEFT_TOP,
        &banner.title,
        egui::FontId::proportional(theme::TEXT),
        BLACK,
    );
    icons::draw(
        painter,
        rect.right_top() + Vec2::new(-40.0, 8.0),
        icons::art(banner.icon),
        1.0,
    );
}

/// the white list of tick boxes with small icons and a right-hand column; the message of a click.
pub fn check_list<M: Clone>(ui: &mut Ui, list: &CheckList<M>) -> Option<M> {
    let mut sent = None;
    theme::sunken(ui, WHITE, |ui| {
        ui.set_width(ui.available_width());
        ScrollArea::vertical()
            .id_salt(list.id)
            .max_height(list.height)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(list.height);
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                for (i, row) in list.rows.iter().enumerate() {
                    let width = ui.available_width();
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::new(width, ROW), Sense::click());
                    let square = Rect::from_min_size(
                        rect.left_center() + Vec2::new(3.0, -BOX / 2.0),
                        Vec2::splat(BOX),
                    );
                    let selected = list.selected == Some(i);
                    let painter = ui.painter();
                    let fill = match (row.partial, row.enabled) {
                        (true, _) | (_, false) => GRAY,
                        _ => WHITE,
                    };
                    painter.rect_filled(square, 0.0, fill);
                    bevel(painter, square, false);
                    if row.checked {
                        tick(painter, square, if row.enabled { BLACK } else { SHADOW });
                    }
                    icons::draw(
                        painter,
                        rect.left_top() + Vec2::new(22.0, 1.0),
                        icons::small(row.icon),
                        1.0,
                    );
                    let label = Rect::from_min_max(
                        rect.left_top() + Vec2::new(41.0, 1.0),
                        rect.right_bottom() - Vec2::new(0.0, 1.0),
                    );
                    let galley = painter.layout_no_wrap(row.label.clone(), theme::font(), BLACK);
                    let text_rect = Rect::from_min_size(
                        label.left_center() - Vec2::new(0.0, galley.size().y / 2.0),
                        galley.size(),
                    )
                    .expand2(Vec2::new(2.0, 0.0));
                    let (colour, back) = if selected {
                        (WHITE, Some(NAVY))
                    } else {
                        (BLACK, None)
                    };
                    if let Some(back) = back {
                        painter.rect_filled(text_rect, 0.0, back);
                    }
                    painter.galley(
                        text_rect.min + Vec2::new(2.0, 0.0),
                        painter.layout_no_wrap(row.label.clone(), theme::font(), colour),
                        colour,
                    );
                    painter.text(
                        rect.right_center() - Vec2::new(6.0, 0.0),
                        Align2::RIGHT_CENTER,
                        &row.detail,
                        theme::font(),
                        BLACK,
                    );
                    if response.clicked() {
                        let on_box = response
                            .interact_pointer_pos()
                            .is_some_and(|p| square.expand(2.0).contains(p));
                        sent = Some(if on_box && row.enabled {
                            (list.toggle)(i)
                        } else {
                            (list.select)(i)
                        });
                    }
                    if response.double_clicked() && row.enabled {
                        sent = Some((list.toggle)(i));
                    }
                }
            });
    });
    sent
}

/// a check mark inside `square`.
fn tick(painter: &egui::Painter, square: Rect, colour: egui::Color32) {
    let at = |x: f32, y: f32| square.left_top() + Vec2::new(x, y);
    let stroke = Stroke::new(2.0, colour);
    painter.line_segment([at(3.0, 6.0), at(5.5, 9.0)], stroke);
    painter.line_segment([at(5.5, 9.0), at(10.0, 3.5)], stroke);
}

/// the sunken box of wrapped text under a setup list.
pub fn description(ui: &mut Ui, text: &str) {
    let frame = egui::Frame::new().inner_margin(egui::Margin::same(5));
    let shown = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(44.0);
        ui.label(RichText::new(text).color(BLACK));
    });
    let rect = shown.response.rect;
    ui.painter()
        .rect_stroke(rect, 0.0, Stroke::new(1.0, SHADOW), StrokeKind::Inside);
}

/// a keyed one-line field; the message of an edit.
pub fn keyed_input<M>(ui: &mut Ui, input: &KeyedInput<M>) -> Option<M> {
    let mut text = input.value.clone();
    ui.push_id(input.key, |ui| controls::edit(ui, &mut text, input.width));
    (text != input.value).then(|| (input.edit)(input.key, text))
}
