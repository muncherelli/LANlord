//! the classic controls egui does not draw like windows 95: edit boxes, check boxes, tabs,
//! group boxes, status bars and drop-downs. all of them are built from the two-shade bevels in
//! `theme`.

use eframe::egui::{
    self, Align2, ComboBox, Frame, Margin, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2,
};

use crate::theme::{self, BLACK, GRAY, SHADOW, WHITE};

/// the height of an edit box, a drop-down and a tab strip's buttons.
pub const FIELD_HEIGHT: f32 = 23.0;
const BOX: f32 = 13.0;

/// a thin one-pixel bevel, as on status panels and table headings.
pub fn thin_bevel(ui: &Ui, rect: Rect, raised: bool) {
    let (top_left, bottom_right) = if raised {
        (WHITE, SHADOW)
    } else {
        (SHADOW, WHITE)
    };
    let r = rect.shrink(0.5);
    let painter = ui.painter();
    let line = |a: Pos2, b: Pos2, colour| painter.line_segment([a, b], Stroke::new(1.0, colour));
    line(r.left_bottom(), r.left_top(), top_left);
    line(r.left_top(), r.right_top(), top_left);
    line(r.right_top(), r.right_bottom(), bottom_right);
    line(r.right_bottom(), r.left_bottom(), bottom_right);
}

/// a sunken white box around whatever `add` draws, like an edit box.
pub fn field<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let framed = Frame::new()
        .fill(WHITE)
        .inner_margin(Margin::symmetric(4, 2))
        .show(ui, add);
    theme::bevel(ui.painter(), framed.response.rect, false);
    framed.inner
}

/// a one-line edit box `width` points wide (the text area; the frame adds a few more).
pub fn edit(ui: &mut Ui, text: &mut String, width: f32) -> Response {
    field(ui, |ui| {
        ui.add(
            eframe::egui::TextEdit::singleline(text)
                .desired_width(width)
                .frame(Frame::new())
                .margin(Margin::ZERO),
        )
    })
}

/// a check box with its label; `changed` is set when it was toggled.
pub fn check(ui: &mut Ui, value: &mut bool, label: &str) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), theme::font(), BLACK);
    let size = Vec2::new(BOX + 6.0 + galley.size().x, 18.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    let square = Rect::from_min_size(
        rect.left_center() - Vec2::new(0.0, BOX / 2.0),
        Vec2::splat(BOX),
    );
    ui.painter().rect_filled(square, 0.0, WHITE);
    theme::bevel(ui.painter(), square, false);
    if *value {
        let c = square.center();
        let stroke = Stroke::new(1.6, BLACK);
        let points = [
            c + Vec2::new(-3.0, 0.0),
            c + Vec2::new(-1.0, 2.5),
            c + Vec2::new(3.5, -3.0),
        ];
        ui.painter().line_segment([points[0], points[1]], stroke);
        ui.painter().line_segment([points[1], points[2]], stroke);
    }
    ui.painter().galley(
        Pos2::new(
            rect.left() + BOX + 6.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        BLACK,
    );
    #[cfg(test)]
    theme::probe::record(ui, label, rect, ui.is_enabled());
    response
}

/// a label in a fixed-width column, then `add` beside it: the rows of a form line up.
pub fn row<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(LABEL_WIDTH, FIELD_HEIGHT), Sense::hover());
        ui.painter().text(
            rect.left_center(),
            Align2::LEFT_CENTER,
            label,
            theme::font(),
            BLACK,
        );
        add(ui)
    })
    .inner
}

/// the width of the label column in `row`.
pub const LABEL_WIDTH: f32 = 110.0;

/// draws page 0 of `count` pages in room as tall as the tallest of them, which are laid out unseen
/// to learn their height, so which one is open never changes the size of what it is in. the
/// height is kept until `key`, which stands for what the pages are made of, the width or the scale
/// change.
pub fn same_size<R>(
    ui: &mut Ui,
    key: u64,
    count: usize,
    mut page: impl FnMut(&mut Ui, usize) -> R,
) -> R {
    let width = ui.available_width();
    let ctx = ui.ctx().clone();
    let id = ui.scope_id().with("same-size");
    let ppp = ctx.pixels_per_point();
    let kept: Option<(u64, f32, f32, usize, f32)> = ctx.data(|d| d.get_temp(id));
    let tallest = match kept {
        Some((k, w, p, n, tallest)) if (k, w, p, n) == (key, width, ppp, count) => tallest,
        _ => {
            let tallest = measure_pages(ui, count, width, &mut page)
                .iter()
                .map(|size| size.y)
                .fold(0.0, f32::max);
            ctx.data_mut(|d| d.insert_temp(id, (key, width, ppp, count, tallest)));
            tallest
        }
    };
    let from = ui.cursor().top();
    let out = page(ui, 0);
    let used = ui.min_rect().bottom() - from;
    if used < tallest {
        ui.add_space(tallest - used);
    }
    out
}

/// room for `lines` rows of text, which `add` fills and which scrolls when it needs more, so
/// what is in it does not change the size of what it is in.
pub fn fixed_lines<R>(ui: &mut Ui, lines: u8, add: impl FnOnce(&mut Ui) -> R) -> R {
    let row = ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y;
    let height = row * f32::from(lines);
    ui.set_min_width(ui.available_width());
    egui::ScrollArea::vertical()
        .id_salt("fixed-lines")
        .max_height(height)
        .min_scrolled_height(height)
        .auto_shrink([false, false])
        .show(ui, add)
        .inner
}

/// an etched frame with a caption, around whatever `add` draws.
pub fn group<R>(ui: &mut Ui, caption: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let width = ui.available_width();
    let inner_width = width - 20.0;
    let framed = Frame::new()
        .inner_margin(Margin {
            left: 9,
            right: 9,
            top: 14,
            bottom: 8,
        })
        .show(ui, |ui| {
            ui.set_min_width(inner_width.max(0.0));
            add(ui)
        });
    let full = framed.response.rect;
    let etched = Rect::from_min_max(
        full.min + Vec2::new(1.0, 7.0),
        full.max - Vec2::new(1.0, 1.0),
    );
    let painter = ui.painter();
    painter.rect_stroke(
        etched.translate(Vec2::new(1.0, 1.0)),
        0.0,
        Stroke::new(1.0, WHITE),
        egui::StrokeKind::Inside,
    );
    painter.rect_stroke(
        etched,
        0.0,
        Stroke::new(1.0, SHADOW),
        egui::StrokeKind::Inside,
    );
    let galley = painter.layout_no_wrap(caption.to_string(), theme::font(), BLACK);
    let at = Pos2::new(full.left() + 9.0, full.top() + 7.0 - galley.size().y / 2.0);
    painter.rect_filled(
        Rect::from_min_size(at, galley.size()).expand2(Vec2::new(3.0, 0.0)),
        0.0,
        GRAY,
    );
    painter.galley(at, galley, BLACK);
    framed.inner
}

/// the least a part of a window that takes extra room (a list, a log, a text) shrinks to.
pub const FILL_MIN: f32 = 80.0;

/// what a window tells the growable parts of its contents: the room it has to spare, and what the
/// parts that took it were without it.
#[derive(Clone, Copy, Default)]
pub struct Spare {
    /// the room left over, until a part takes it
    pub extra: f32,
    /// the heights the parts that grow have on their own, added up
    pub claimed: f32,
    /// a part took room that is whole: it is never to be sized below its natural height
    pub whole: bool,
}

fn spare_id() -> egui::Id {
    egui::Id::unique("window-spare-room")
}

/// the window gives its contents `extra` points of room to spare, and takes back what they used.
pub fn lend_room<R>(ctx: &egui::Context, extra: f32, contents: impl FnOnce() -> R) -> (R, Spare) {
    ctx.data_mut(|d| {
        d.insert_temp(
            spare_id(),
            Some(Spare {
                extra,
                claimed: 0.0,
                whole: false,
            }),
        )
    });
    let result = contents();
    let back = ctx.data_mut(|d| d.remove_temp::<Option<Spare>>(spare_id()));
    (result, back.flatten().unwrap_or_default())
}

/// like `lend_room` for a part inside the window's contents: the room the window has lent is kept
/// for what comes after.
fn lend_nested<R>(ctx: &egui::Context, extra: f32, contents: impl FnOnce() -> R) -> (R, Spare) {
    let outer = ctx.data_mut(|d| d.remove_temp::<Option<Spare>>(spare_id()));
    let result = lend_room(ctx, extra, contents);
    if let Some(outer) = outer {
        ctx.data_mut(|d| d.insert_temp(spare_id(), outer));
    }
    result
}

/// the room the window has to spare, taken by a part that is as big as it is whatever the window
/// does: it can never shrink below its natural height, unlike a list. none outside a window.
fn take_whole(ctx: &egui::Context) -> f32 {
    ctx.data_mut(
        |d| match d.get_temp_mut_or_default::<Option<Spare>>(spare_id()) {
            Some(spare) => {
                spare.whole = true;
                std::mem::take(&mut spare.extra)
            }
            None => 0.0,
        },
    )
}

/// the height of a part that can grow, whose height on its own is `natural`: that plus the room
/// the window has to spare, which the first such part takes. outside a window it is `natural`.
pub fn grow(ui: &Ui, natural: f32) -> f32 {
    ui.ctx().data_mut(
        |d| match d.get_temp_mut_or_default::<Option<Spare>>(spare_id()) {
            Some(spare) => {
                spare.claimed += natural;
                natural + std::mem::take(&mut spare.extra)
            }
            None => natural,
        },
    )
}

/// a row of buttons pushed to the right edge; the row is one button high, so what comes
/// after it (a status bar, say) stays under it.
pub fn buttons<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let size = Vec2::new(ui.available_width(), 25.0);
    ui.allocate_ui_with_layout(size, egui::Layout::right_to_left(egui::Align::Center), add)
        .inner
}

/// the height of a status bar.
pub const STATUS_HEIGHT: f32 = 20.0;

/// a sunken panel for status text at the bottom of a window.
pub fn status_bar(ui: &mut Ui, text: &str, error: bool) {
    let colour = if error { crate::remote::RED } else { BLACK };
    // a long message wraps and grows the bar, so none of it is cut off
    let width = ui.available_width();
    let galley = ui.painter().layout(
        text.to_string(),
        theme::font(),
        colour,
        (width - 12.0).max(1.0),
    );
    let height = STATUS_HEIGHT.max(galley.size().y + 6.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    thin_bevel(ui, rect, false);
    let at = rect.left_center() + Vec2::new(6.0, -galley.size().y / 2.0);
    ui.painter()
        .with_clip_rect(rect.shrink(2.0))
        .galley(at, galley, colour);
}

/// a horizontal line cut into the surface, as between groups of controls.
pub fn separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 2.0), Sense::hover());
    let painter = ui.painter();
    painter.line_segment(
        [rect.left_top(), rect.right_top()],
        Stroke::new(1.0, SHADOW),
    );
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, WHITE),
    );
}

/// where the tab at `index` sits in a strip that starts at `left` and ends at `bottom`.
fn tab_rect(left: f32, widths: &[f32], index: usize, bottom: f32, selected: bool) -> Rect {
    let x = left + widths[..index].iter().sum::<f32>();
    let top = bottom - if selected { 24.0 } else { 21.0 };
    let grow = if selected { 2.0 } else { 0.0 };
    Rect::from_min_max(
        Pos2::new(x - grow, top),
        Pos2::new(x + widths[index] + grow, bottom + 1.0),
    )
}

/// how big each page of a tab strip is on its own, at the width and scale they were laid out at.
#[derive(Clone, PartialEq)]
struct PageSizes {
    width: f32,
    ppp: f32,
    heights: Vec<f32>,
    /// how wide each is, which is more than `width` for a page that will not narrow
    widths: Vec<f32>,
}

fn tabbed_id() -> egui::Id {
    egui::Id::unique("window-has-tabs")
}

/// whether a tab strip was drawn since the last call: the window it is in is a tabbed one.
pub(crate) fn take_tabbed(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(tabbed_id()))
        .unwrap_or(false)
}

/// where the page frame of the last tab strip drawn was.
#[cfg(test)]
pub(crate) fn tab_frame(ctx: &egui::Context) -> Option<Rect> {
    ctx.data(|d| d.get_temp::<Rect>(egui::Id::unique("tabs-frame")))
}

/// how many times the pages of a tab strip were laid out to be measured, to show it is not done
/// every frame.
#[cfg(test)]
pub(crate) fn measure_passes(ctx: &egui::Context) -> u32 {
    ctx.data(|d| d.get_temp::<u32>(egui::Id::unique("tab-measure-passes")))
        .unwrap_or(0)
}

/// lay every page out once, unseen, to learn how big each is. the room a window lends is set
/// aside meanwhile, so measuring takes none of it.
fn measure_pages<R>(
    ui: &mut Ui,
    count: usize,
    width: f32,
    page: &mut impl FnMut(&mut Ui, usize) -> R,
) -> Vec<Vec2> {
    let ctx = ui.ctx().clone();
    let held = ctx.data_mut(|d| d.remove_temp::<Option<Spare>>(spare_id()));
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("tabs-measuring"), true));
    let sizes: Vec<Vec2> = (0..count)
        .map(|i| {
            let rect = Rect::from_min_size(
                ui.cursor().min,
                Vec2::new(width, ui.available_height().max(1.0)),
            );
            let mut probe = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt(("tab-measure", i))
                    .max_rect(rect)
                    .invisible(),
            );
            page(&mut probe, i);
            let size = probe.min_rect().size();
            Vec2::new(size.x.ceil(), size.y.ceil())
        })
        .collect();
    ctx.data_mut(|d| {
        d.remove_temp::<bool>(egui::Id::unique("tabs-measuring"));
        d.remove_temp::<Option<Spare>>(spare_id());
        if let Some(held) = held {
            d.insert_temp(spare_id(), held);
        }
    });
    #[cfg(test)]
    ctx.data_mut(|d| {
        *d.get_temp_mut_or_default::<u32>(egui::Id::unique("tab-measure-passes")) += 1;
    });
    sizes
}

/// a strip of tabs over a raised page. the chosen tab is taller, joined to the page and the
/// others sit behind it. `page` draws the page at an index, and is called for every page once to
/// measure it: the frame is as big as the biggest, so the window keeps its size whichever tab is
/// open. the sizes are kept until the width or scale changes, or the open page is not the size it
/// was measured at.
pub fn tabs<R>(
    ui: &mut Ui,
    labels: &[&str],
    selected: &mut usize,
    mut page: impl FnMut(&mut Ui, usize) -> R,
) -> R {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let widths: Vec<f32> = labels
            .iter()
            .map(|l| {
                ui.painter()
                    .layout_no_wrap((*l).to_string(), theme::font(), BLACK)
                    .size()
                    .x
                    + 20.0
            })
            .collect();
        let (strip, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 25.0), Sense::hover());
        let bottom = strip.bottom() - 1.0;
        let left = strip.left() + 2.0;
        for (i, label) in labels.iter().enumerate() {
            if i == *selected {
                continue;
            }
            let rect = tab_rect(left, &widths, i, bottom, false);
            let response = ui.interact(rect, ui.scope_id().with(("tab", i)), Sense::click());
            if response.clicked() {
                *selected = i;
            }
            #[cfg(test)]
            theme::probe::record(ui, label, rect, true);
            paint_tab(ui, rect, label, false);
        }
        let ctx = ui.ctx().clone();
        ctx.data_mut(|d| d.insert_temp(tabbed_id(), true));
        // the page sits in a frame with a 9 point margin
        let inner = (ui.available_width() - 18.0).max(1.0);
        let sizes_id = ui.scope_id().with("page-sizes");
        let ppp = ctx.pixels_per_point();
        let kept: Option<PageSizes> = ctx.data(|d| d.get_temp(sizes_id));
        let mut sizes = match kept {
            Some(k) if k.width == inner && k.ppp == ppp && k.heights.len() == labels.len() => k,
            _ => {
                let measured = measure_pages(ui, labels.len(), inner, &mut page);
                PageSizes {
                    width: inner,
                    ppp,
                    heights: measured.iter().map(|m| m.y).collect(),
                    widths: measured.iter().map(|m| m.x).collect(),
                }
            }
        };
        ctx.data_mut(|d| d.insert_temp(sizes_id, sizes.clone()));
        let open = (*selected).min(labels.len().saturating_sub(1));
        let natural = sizes.heights.get(open).copied().unwrap_or(0.0);
        let tallest = sizes.heights.iter().copied().fold(0.0, f32::max);
        // a window with room to spare gives it to the frame, which gives it to the page's list
        let frame_height = tallest + take_whole(&ctx);
        let widest = sizes.widths.iter().copied().fold(0.0, f32::max);
        let mut drawn = (0.0, 0.0);
        let framed = Frame::new()
            .fill(GRAY)
            .inner_margin(Margin::same(9))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width().max(widest));
                ui.set_min_height(frame_height);
                let from = ui.cursor().top();
                let (out, back) = lend_nested(&ctx, frame_height - natural, || page(ui, *selected));
                drawn = (
                    ui.cursor().top() - from,
                    frame_height - natural - back.extra,
                );
                out
            });
        // the open page is not what it was measured as: measure it again, alone
        let (height, used) = drawn;
        if (height - used - natural).abs() > 1.0 {
            sizes.heights[open] = (height - used).ceil();
            ctx.data_mut(|d| d.insert_temp(sizes_id, sizes));
            ctx.request_repaint();
        }
        #[cfg(test)]
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("tabs-frame"), framed.response.rect));
        theme::bevel(ui.painter(), framed.response.rect, true);
        // the chosen tab is drawn last so it covers the page's top edge and joins it
        let rect = tab_rect(left, &widths, *selected, bottom, true);
        paint_tab(ui, rect, labels[*selected], true);
        let join = Rect::from_min_max(
            Pos2::new(rect.left() + 2.0, framed.response.rect.top()),
            Pos2::new(rect.right() - 2.0, framed.response.rect.top() + 2.0),
        );
        ui.painter().rect_filled(join, 0.0, GRAY);
        framed.inner
    })
    .inner
}

fn paint_tab(ui: &Ui, rect: Rect, label: &str, selected: bool) {
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, GRAY);
    // white on the top and left, dark on the right, no line along the bottom
    let r = rect.shrink2(Vec2::new(0.5, 0.5));
    let stroke = |c| Stroke::new(1.0, c);
    painter.line_segment([r.left_bottom(), r.left_top()], stroke(WHITE));
    painter.line_segment([r.left_top(), r.right_top()], stroke(WHITE));
    painter.line_segment([r.right_top(), r.right_bottom()], stroke(BLACK));
    painter.line_segment(
        [
            r.right_top() - Vec2::new(1.0, -1.0),
            r.right_bottom() - Vec2::new(1.0, 0.0),
        ],
        stroke(SHADOW),
    );
    painter.text(
        rect.center() - Vec2::new(0.0, if selected { 1.0 } else { -1.0 }),
        Align2::CENTER_CENTER,
        label,
        theme::font(),
        BLACK,
    );
}

/// a drop-down list: a sunken box with the current choice and a raised arrow button.
pub fn dropdown<R>(
    ui: &mut Ui,
    id: &str,
    current: &str,
    width: f32,
    list: impl FnOnce(&mut Ui) -> R,
) {
    ui.scope(|ui| {
        let visuals = ui.visuals_mut();
        for widget in [
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            widget.weak_bg_fill = WHITE;
            widget.bg_fill = WHITE;
            widget.bg_stroke = Stroke::NONE;
            widget.expansion = 0.0;
        }
        ui.spacing_mut().interact_size.y = FIELD_HEIGHT;
        let response = ComboBox::from_id_salt(id)
            .selected_text(current)
            .width(width)
            .icon(|ui, rect, _, _| arrow_button(ui, rect))
            .show_ui(ui, list);
        theme::bevel(ui.painter(), response.response.rect, false);
    });
}

fn arrow_button(ui: &Ui, rect: Rect) {
    let button = Rect::from_min_max(
        Pos2::new(rect.right() - 17.0, rect.top() + 2.0),
        Pos2::new(rect.right() - 2.0, rect.bottom() - 2.0),
    );
    ui.painter().rect_filled(button, 0.0, GRAY);
    theme::bevel(ui.painter(), button, true);
    let c = button.center();
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            c + Vec2::new(-4.0, -2.0),
            c + Vec2::new(4.0, -2.0),
            c + Vec2::new(0.0, 2.0),
        ],
        BLACK,
        Stroke::NONE,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit;

    #[test]
    fn the_chosen_tab_is_taller_and_wider_than_the_others() {
        let widths = [60.0, 50.0, 80.0];
        let plain = tab_rect(10.0, &widths, 1, 100.0, false);
        let chosen = tab_rect(10.0, &widths, 1, 100.0, true);
        assert!(chosen.height() > plain.height() && chosen.width() > plain.width());
        assert_eq!(plain.left(), 70.0, "after the first tab");
        assert!(chosen.bottom() > 100.0, "overlaps the page edge");
    }

    #[test]
    fn a_check_box_toggles_and_tabs_switch() {
        let ctx = egui::Context::default();
        let mut on = false;
        let mut tab = 0usize;
        let mut frame = |events: Vec<egui::Event>| {
            testkit::run(&ctx, events, |ui| {
                theme::apply(ui.ctx());
                check(ui, &mut on, "Tick me");
                tabs(ui, &["One", "Two"], &mut tab, |ui, _| {
                    let mut text = String::from("x");
                    edit(ui, &mut text, 80.0);
                    group(ui, "Caption", |ui| {
                        row(ui, "Label", |ui| ui.label("value"));
                        separator(ui);
                    });
                    status_bar(ui, "ready", false);
                    status_bar(ui, "broken", true);
                    dropdown(ui, "pick", "one", 80.0, |ui| ui.label("a"));
                });
            });
        };
        for _ in 0..3 {
            frame(Vec::new());
        }
        for name in ["Tick me", "Two"] {
            let (rect, _) = theme::probe::find(&ctx, name).expect(name);
            for event in testkit::click(rect.center()) {
                frame(vec![event]);
            }
        }
        assert!(on);
        assert_eq!(tab, 1);
    }
}
