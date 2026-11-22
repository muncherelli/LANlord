//! draws the app's layout tree (`lanlord_app::view`) with egui, in the nt4 look, and returns
//! the message of whatever was clicked.

use chrono::{Datelike, NaiveDate};
use eframe::egui::{self, Frame, Margin, RichText, ScrollArea, Stroke, TextEdit, Ui, Vec2};
use eframe::egui::{Align2, Pos2, Rect, Sense, Shape};
use lanlord_app::clock_window;
use lanlord_app::view::{Dialog, Node, Size};

use crate::console;
use crate::controls;
use crate::dialogs;
use crate::icons;
use crate::remote::RED;
use crate::theme::{self, BLACK, GRAY, NAVY, SHADOW, WHITE};
use crate::view_wizard;
use crate::widgets::Table;

/// how long a status bar takes to slide in or out.
const SLIDE_SECS: f32 = 0.15;

/// draw `nodes` top to bottom; the message of a click, if there was one. a message changes the
/// window, so another frame is asked for to show it. inside a window, a status bar that ends the
/// nodes is held back for the window to draw below the part that scrolls.
pub fn show<M: Clone>(ui: &mut Ui, nodes: &[Node<M>]) -> Option<M> {
    let pinning = ui
        .ctx()
        .data(|d| d.get_temp::<Option<Option<Pinned>>>(pin_id()))
        .flatten()
        .is_some();
    let (head, pinned) = match nodes {
        [head @ .., Node::Gap(gap), Node::Status { text, error }] if pinning => (
            head,
            Some(Pinned {
                gap: Some((nodes.len() - 2, *gap)),
                index: nodes.len() - 1,
                text: text.clone(),
                error: *error,
            }),
        ),
        [head @ .., Node::Status { text, error }] if pinning => (
            head,
            Some(Pinned {
                gap: None,
                index: nodes.len() - 1,
                text: text.clone(),
                error: *error,
            }),
        ),
        _ => (nodes, None),
    };
    let sent = nodes_ui(ui, head);
    if let Some(pinned) = pinned {
        ui.ctx()
            .data_mut(|d| d.insert_temp(pin_id(), Some(Some(pinned))));
    }
    if sent.is_some() {
        ui.ctx().request_repaint();
    }
    sent
}

/// a status bar that ends a window's contents, held back by `show` so that the window can draw
/// it on its bottom edge, outside what scrolls.
#[derive(Clone)]
pub struct Pinned {
    /// the node index and size of the gap before the bar, if there is one.
    gap: Option<(usize, f32)>,
    /// the node index of the bar, which its slide is kept under.
    index: usize,
    text: String,
    error: bool,
}

fn pin_id() -> egui::Id {
    egui::Id::unique("window-pinned-status")
}

/// run `contents`, which show nodes, with their trailing status bar held back; the window draws
/// it with `draw_pinned`.
pub fn pin_status<R>(ctx: &egui::Context, contents: impl FnOnce() -> R) -> (R, Option<Pinned>) {
    ctx.data_mut(|d| d.insert_temp(pin_id(), Some(None::<Pinned>)));
    let result = contents();
    let back = ctx.data_mut(|d| d.remove_temp::<Option<Option<Pinned>>>(pin_id()));
    (result, back.flatten().flatten())
}

/// how far the held-back bar has slid in: 0 when hidden, 1 when fully shown.
pub fn pinned_amount(ui: &Ui, pinned: &Pinned) -> f32 {
    slide_amount(ui, pinned.index, !pinned.text.is_empty())
}

/// draw the held-back status bar, and the gap before it, sliding in and out.
pub fn draw_pinned(ui: &mut Ui, pinned: &Pinned) {
    if let Some((index, gap)) = pinned.gap {
        slide_space(ui, index, gap, !pinned.text.is_empty());
    }
    slide_status(ui, pinned.index, &pinned.text, pinned.error);
}

/// draw a modal box; the message of a click inside it, if there was one.
pub fn dialog<M: Clone>(ctx: &egui::Context, dialog: &Dialog<M>) -> Option<M> {
    let (sent, closed) = dialogs::modal(
        ctx,
        dialog.id,
        &dialog.title,
        dialog.escape.is_some(),
        |ui| {
            let sent = nodes_ui(ui, &dialog.body);
            let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
            sent.or_else(|| escaped.then(|| dialog.escape.clone()).flatten())
        },
    );
    // the close button does what escape does
    let sent = sent.or_else(|| closed.then(|| dialog.escape.clone()).flatten());
    if sent.is_some() {
        ctx.request_repaint();
    }
    sent
}

fn nodes_ui<M: Clone>(ui: &mut Ui, nodes: &[Node<M>]) -> Option<M> {
    let mut sent = None;
    // a status bar that ends the window sits on its bottom edge: the room the window has
    // left over, which no growable part took, goes above the bar and the gap before it
    let pinned = match nodes {
        [.., Node::Gap(_), Node::Status { .. }] => nodes.len().checked_sub(2),
        [.., Node::Status { .. }] => nodes.len().checked_sub(1),
        _ => None,
    };
    for (i, node) in nodes.iter().enumerate() {
        if pinned == Some(i) {
            ui.add_space(controls::grow(ui, 0.0));
        }
        let msg = match (node, nodes.get(i + 1)) {
            // a gap that leads into a status bar slides with it
            (Node::Gap(gap), Some(Node::Status { text, .. })) => {
                slide_space(ui, i, *gap, !text.is_empty());
                None
            }
            (Node::Status { text, error }, _) => {
                slide_status(ui, i, text, *error);
                None
            }
            _ => node_ui(ui, node),
        };
        if msg.is_some() {
            sent = msg;
        }
    }
    sent
}

/// how far a status bar at node `index` has slid in: 0 when hidden, 1 when fully shown.
fn slide_amount(ui: &Ui, index: usize, shown: bool) -> f32 {
    let id = ui.scope_id().with(("status-slide", index));
    slide(ui.ctx(), id, shown, reduce_motion())
}

/// whether moving pictures are off (LANLORD_REDUCE_MOTION).
pub fn reduce_motion() -> bool {
    std::env::var_os("LANLORD_REDUCE_MOTION").is_some()
}

/// the slide at `id`; with `reduce` it jumps straight to fully shown or hidden.
fn slide(ctx: &egui::Context, id: egui::Id, shown: bool, reduce: bool) -> f32 {
    if reduce {
        return f32::from(u8::from(shown));
    }
    ctx.animate_bool_with_time(id, shown, SLIDE_SECS)
}

/// the gap before a status bar, scaled by how far the bar has slid in.
fn slide_space(ui: &mut Ui, index: usize, gap: f32, shown: bool) {
    // the bar is the next node, so it owns the animation under its own index
    let t = slide_amount(ui, index + 1, shown);
    ui.add_space(gap * t);
}

/// a status bar that slides up into the window when it has text and away when it clears.
fn slide_status(ui: &mut Ui, index: usize, text: &str, error: bool) {
    let t = slide_amount(ui, index, !text.is_empty());
    if t <= 0.0 {
        return;
    }
    if t >= 1.0 {
        controls::status_bar(ui, text, error);
        return;
    }
    let full = controls::STATUS_HEIGHT;
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), full * t), Sense::hover());
    let mut inner = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.translate(Vec2::new(0.0, full * (1.0 - t))))
            .layout(*ui.layout()),
    );
    inner.set_clip_rect(rect.intersect(ui.clip_rect()));
    controls::status_bar(&mut inner, text, error);
}

fn node_ui<M: Clone>(ui: &mut Ui, node: &Node<M>) -> Option<M> {
    match node {
        Node::Banner(banner) => {
            view_wizard::banner(ui, banner);
            None
        }
        Node::CheckList(list) => view_wizard::check_list(ui, list),
        Node::Description(text) => {
            view_wizard::description(ui, text);
            None
        }
        Node::KeyedInput(input) => view_wizard::keyed_input(ui, input),
        Node::Text(text) => {
            ui.label(text.as_str());
            None
        }
        Node::Gap(height) => {
            ui.add_space(*height);
            None
        }
        Node::Group { title, body } => controls::group(ui, title, |ui| nodes_ui(ui, body)),
        Node::SameSize { key, body, others } => {
            controls::same_size(ui, *key, others.len() + 1, |ui, i| match i {
                0 => nodes_ui(ui, body),
                i => nodes_ui(ui, &others[i - 1]),
            })
        }
        Node::Details { title, lines, body } => controls::group(ui, title, |ui| {
            controls::fixed_lines(ui, *lines, |ui| nodes_ui(ui, body))
        }),
        Node::Table(table) => Table {
            id: table.id,
            headings: &table.headings,
            widths: &table.widths,
            rows: &table.rows,
            selected: table.selected,
            bad: &table.bad,
            height: table.height,
            steady: table.steady,
        }
        .show(ui)
        .map(table.select),
        Node::Buttons(buttons) => controls::buttons(ui, |ui| {
            let mut sent = None;
            for button in buttons {
                if theme::action(ui, &button.label, button.enabled).clicked() {
                    sent = Some(button.press.clone());
                }
            }
            sent
        }),
        Node::Field { label, value } => {
            controls::row(ui, label, |ui| ui.label(value.as_str()));
            None
        }
        Node::Status { .. } => None,
        Node::Heading(text) => {
            ui.label(RichText::new(text.as_str()).strong());
            None
        }
        Node::Bullet(text) => {
            // the bullet stands in its own column so wrapped lines hang under the text
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.add_space(8.0);
                ui.label("\u{2022}");
                ui.add_space(8.0);
                ui.add(egui::Label::new(text.as_str()).wrap());
            });
            None
        }
        Node::Button(button) if button.selected => {
            // the chosen one is pressed in, not greyed
            theme::sized_button(ui, &button.label, true, true, button.width);
            None
        }
        Node::Button(button) if button.width.is_some() => {
            theme::sized_button(ui, &button.label, false, button.enabled, button.width)
                .clicked()
                .then(|| button.press.clone())
        }
        Node::Button(button) => theme::action(ui, &button.label, button.enabled)
            .clicked()
            .then(|| button.press.clone()),
        Node::Tabs {
            titles,
            key,
            current,
            pick,
            body,
            others,
        } => {
            let mut tab = *current;
            let labels: Vec<&str> = titles.iter().map(String::as_str).collect();
            let inside = controls::tabs(ui, &labels, *key, &mut tab, |ui, i| {
                if i == *current {
                    nodes_ui(ui, body)
                } else {
                    others
                        .iter()
                        .find(|(n, _)| *n == i)
                        .and_then(|(_, nodes)| nodes_ui(ui, nodes))
                }
            });
            if tab == *current {
                inside
            } else {
                Some(pick(tab))
            }
        }
        Node::Danger(body) => {
            Frame::new()
                .stroke(Stroke::new(1.0, RED))
                .inner_margin(Margin::same(3))
                .show(ui, |ui| nodes_ui(ui, body))
                .inner
        }
        Node::Check {
            label,
            checked,
            enabled,
            toggle,
        } => {
            let mut ticked = *checked;
            ui.add_enabled_ui(*enabled, |ui| controls::check(ui, &mut ticked, label));
            (ticked != *checked).then(|| toggle.clone())
        }
        Node::Input {
            value,
            size,
            focus,
            hint,
            edit,
            enter,
        } => {
            let mut text = value.clone();
            let pressed = line_edit(ui, &mut text, *size, false, hint, *focus);
            match enter {
                // Enter carries the final text, so keys typed with it are kept
                Some(enter) if pressed => Some(enter(text)),
                _ => (text != *value).then(|| edit(text)),
            }
        }
        Node::Secret {
            value,
            size,
            shown,
            focus,
            edit,
            enter,
        } => {
            let mut text = value.clone();
            let pressed = line_edit(ui, &mut text, *size, !shown, "", *focus);
            match enter {
                Some(enter) if pressed => Some(enter(text)),
                _ => (*text != **value).then(|| edit(text)),
            }
        }
        Node::Lines {
            id,
            lines,
            height,
            enabled,
        } => {
            let height = controls::grow(ui, *height);
            theme::sunken(ui, if *enabled { WHITE } else { GRAY }, |ui| {
                ui.set_width(ui.available_width());
                ScrollArea::vertical()
                    .id_salt(*id)
                    .max_height(height)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.set_min_height(height);
                        for line in lines {
                            ui.label(RichText::new(line.as_str()).monospace().size(11.0));
                        }
                    });
            });
            None
        }
        Node::Log { id, lines, height } => {
            let (max, min) = match height {
                Some(height) => (*height, height - 20.0),
                // a log that fills: this tall on its own, and the room a window has to spare
                None => {
                    let fill = controls::grow(ui, LOG_ROWS);
                    (fill, fill)
                }
            };
            theme::sunken(ui, BLACK, |ui| {
                ui.set_width(ui.available_width());
                ScrollArea::vertical()
                    .id_salt(*id)
                    .stick_to_bottom(true)
                    .max_height(max)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.set_min_height(min);
                        for (kind, text) in lines {
                            let line = RichText::new(text.as_str())
                                .monospace()
                                .color(console::colour(*kind));
                            ui.label(line);
                        }
                    });
            });
            None
        }
        Node::List {
            id,
            items,
            selected,
            min_height,
            empty,
            select,
        } => {
            let mut picked = None;
            let min_height = controls::grow(ui, *min_height);
            ui.push_id(*id, |ui| {
                theme::sunken(ui, WHITE, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height(min_height);
                    if items.is_empty() {
                        ui.label(empty.as_str());
                    }
                    for (i, item) in items.iter().enumerate() {
                        if theme::list_row(ui, item, *selected == Some(i)).clicked() {
                            picked = Some(i);
                        }
                    }
                });
            });
            picked.map(select)
        }
        Node::Toolbar(buttons) => {
            ui.horizontal(|ui| {
                let mut sent = None;
                for button in buttons {
                    if theme::action(ui, &button.label, button.enabled).clicked() {
                        sent = Some(button.press.clone());
                    }
                }
                sent
            })
            .inner
        }
        Node::Header { icon, title, lines } => {
            ui.horizontal(|ui| {
                icons::draw(ui.painter(), ui.cursor().min, icons::art(icon), 1.0);
                ui.add_space(40.0);
                ui.vertical(|ui| {
                    if !title.is_empty() {
                        ui.label(RichText::new(title.as_str()).size(16.0).strong());
                    }
                    for line in lines {
                        ui.label(line.as_str());
                    }
                });
            });
            None
        }
        Node::Code(text) => {
            theme::sunken(ui, WHITE, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(text.as_str()).monospace());
            });
            None
        }
        Node::Separator => {
            controls::separator(ui);
            None
        }
        Node::Bar { fraction, text } => {
            theme::sunken(ui, WHITE, |ui| {
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.0), Sense::hover());
                // nt4 style: solid navy blocks with a gap between them
                let full = rect.width() * fraction.clamp(0.0, 1.0);
                let mut x = rect.left();
                while x + 8.0 <= rect.left() + full {
                    let block = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(8.0, 16.0));
                    ui.painter().rect_filled(block, 0.0, NAVY);
                    x += 10.0;
                }
                // the fill ends at the right edge of the last block (blocks are 8 wide, 2 apart)
                let edge = if x > rect.left() {
                    x - 2.0
                } else {
                    rect.left()
                };
                // the label is drawn twice, clipped at the fill edge: white over the blocks,
                // black over the rest, as a classic progress bar does
                let (done, todo) = bar_clips(rect, edge);
                for (clip, colour) in [(done, WHITE), (todo, BLACK)] {
                    ui.painter().with_clip_rect(clip).text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        text,
                        egui::FontId::proportional(12.0),
                        colour,
                    );
                }
            });
            None
        }
        Node::Row(body) => ui.horizontal(|ui| show(ui, body)).inner,
        Node::TextArea {
            id,
            value,
            height,
            hint,
            edit,
        } => {
            let mut text = value.clone();
            let height = controls::grow(ui, *height);
            theme::sunken(ui, WHITE, |ui| {
                ui.set_width(ui.available_width());
                ScrollArea::vertical()
                    .id_salt(*id)
                    .max_height(height)
                    .show(ui, |ui| {
                        ui.add_sized(
                            [ui.available_width(), height],
                            TextEdit::multiline(&mut text)
                                .hint_text(hint.as_str())
                                .font(egui::TextStyle::Monospace)
                                .frame(egui::Frame::NONE)
                                .desired_width(f32::INFINITY),
                        );
                    });
            });
            (text != *value).then(|| edit(text))
        }
        Node::Choice {
            id,
            current,
            width,
            options,
            pick,
        } => {
            let mut picked = None;
            controls::dropdown(ui, id, current, *width, |ui| {
                for (key, label) in options {
                    if ui
                        .selectable_label(*label == *current, label.as_str())
                        .clicked()
                    {
                        picked = Some(*key);
                    }
                }
            });
            picked.map(pick)
        }
        Node::Fold { id, title, body } => egui::CollapsingHeader::new(title.as_str())
            .id_salt(*id)
            .show(ui, |ui| show(ui, body))
            .body_returned
            .flatten(),
        Node::Disabled(body) => ui.add_enabled_ui(false, |ui| show(ui, body)).inner,
        Node::Labelled { label, body } => controls::row(ui, label, |ui| show(ui, body)),
        Node::FieldError(text) => {
            ui.label(RichText::new(text.as_str()).color(RED).small());
            None
        }
        Node::HeldError {
            text,
            partial,
            field,
        } => {
            // hidden, not removed, while a field is being typed in: the room stays so the
            // fields below do not jump when the focus leaves
            let typing = controls::field_has_keyboard(ui.ctx(), *field);
            let colour = if *partial && typing {
                egui::Color32::TRANSPARENT
            } else {
                RED
            };
            ui.label(RichText::new(text.as_str()).color(colour).small());
            None
        }
        Node::Alert(text) => {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(32.0), egui::Sense::hover());
                warning_sign(ui, rect);
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width());
                    ui.label(text.as_str());
                });
            });
            None
        }
        Node::Spin {
            id,
            text,
            width,
            enabled,
            step,
        } => {
            let dir = spin(ui, id, text, *width, *enabled);
            (dir != 0).then(|| step(dir))
        }
        Node::Calendar {
            year,
            month,
            selected,
            today,
            enabled,
            pick,
            ..
        } => calendar(ui, *year, *month, *selected, *today, *enabled).map(pick),
        Node::ClockFace {
            hour,
            minute,
            second,
            size,
        } => {
            analog(ui, *size, (*hour, *minute, *second));
            None
        }
        Node::Columns(columns) => {
            ui.horizontal_top(|ui| {
                let mut sent = None;
                for (width, body) in columns {
                    let got = ui
                        .vertical(|ui| {
                            if let Some(width) = width {
                                ui.set_width(*width);
                            }
                            show(ui, body)
                        })
                        .inner;
                    sent = sent.or(got);
                }
                sent
            })
            .inner
        }
        Node::Icon(id) => {
            let (art, _) = ui.allocate_exact_size(Vec2::splat(16.0), egui::Sense::hover());
            icons::draw(ui.painter(), art.min, icons::small(id), 1.0);
            None
        }
        Node::Warning(text) => {
            ui.colored_label(egui::Color32::from_rgb(160, 0, 0), text.as_str());
            None
        }
        Node::Scroll {
            id,
            max_height,
            body,
        } => {
            let natural = *max_height;
            let max_height = controls::grow(ui, natural);
            theme::sunken(ui, WHITE, |ui| {
                ui.set_width(ui.available_width());
                ScrollArea::vertical()
                    .id_salt(*id)
                    .max_height(max_height)
                    .show(ui, |ui| {
                        if max_height > natural {
                            ui.set_min_height(max_height);
                        }
                        show(ui, body)
                    })
                    .inner
            })
        }
    }
}

/// the height of a log that fills its window, before the window gives it room.
const LOG_ROWS: f32 = 160.0;

/// the yellow warning triangle of a message box.
fn warning_sign(ui: &egui::Ui, rect: egui::Rect) {
    let c = rect.center();
    let points = vec![
        c + Vec2::new(0.0, -14.0),
        c + Vec2::new(15.0, 13.0),
        c + Vec2::new(-15.0, 13.0),
    ];
    let yellow = egui::Color32::from_rgb(255, 255, 0);
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        yellow,
        egui::Stroke::new(1.5, BLACK),
    ));
    ui.painter().text(
        c + Vec2::new(0.0, 4.0),
        egui::Align2::CENTER_CENTER,
        "!",
        egui::FontId::proportional(16.0),
        BLACK,
    );
}

/// a one-line edit box; true when Enter was pressed in it.
fn line_edit(
    ui: &mut Ui,
    text: &mut String,
    size: Size,
    masked: bool,
    hint: &str,
    focus: bool,
) -> bool {
    let width = match size {
        Size::Points(width) => width,
        // measured in the real font, with a little over for the cursor
        Size::Text(text) => {
            ui.painter()
                .layout_no_wrap(text.to_string(), theme::font(), BLACK)
                .size()
                .x
                + 2.0
        }
        Size::FillLess(room) => ui.available_width() - room,
    };
    let response = controls::field(ui, |ui| {
        ui.add(
            TextEdit::singleline(text)
                .password(masked)
                .hint_text(hint)
                .desired_width(width)
                .frame(egui::Frame::new())
                .margin(egui::Margin::ZERO),
        )
    });
    controls::note_edit(ui.ctx(), response.id);
    #[cfg(test)]
    dialogs::note_box(ui.ctx(), response.rect, width);
    if focus {
        response.request_focus();
    }
    response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

/// a number in a sunken box with an up and a down button; returns which button was pressed.
fn spin(ui: &mut Ui, id: &str, text: &str, width: f32, enabled: bool) -> i32 {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(width + 15.0, controls::FIELD_HEIGHT),
        Sense::hover(),
    );
    let field = Rect::from_min_size(rect.min, Vec2::new(width, rect.height()));
    ui.painter().rect_filled(field, 0.0, WHITE);
    theme::bevel(ui.painter(), field, false);
    let colour = if enabled { BLACK } else { SHADOW };
    ui.painter().text(
        field.center(),
        Align2::CENTER_CENTER,
        text,
        theme::font(),
        colour,
    );
    let half = (rect.height() - 4.0) / 2.0;
    let mut pressed = 0;
    for (dir, name) in [(1, "up"), (-1, "down")] {
        let top = if dir == 1 {
            rect.top() + 2.0
        } else {
            rect.top() + 2.0 + half
        };
        let button = Rect::from_min_size(Pos2::new(field.right(), top), Vec2::new(14.0, half));
        let sense = if enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let response = ui.interact(button, ui.scope_id().with(("clock-spin", id, name)), sense);
        let down = response.is_pointer_button_down_on();
        ui.painter().rect_filled(button, 0.0, GRAY);
        theme::bevel(ui.painter(), button, !down);
        arrow(ui, button, dir == 1, enabled);
        if response.clicked() {
            pressed = dir;
        }
        #[cfg(test)]
        theme::probe::record(ui, &format!("{id} {name}"), button, enabled);
    }
    pressed
}

fn arrow(ui: &Ui, rect: Rect, up: bool, enabled: bool) {
    let c = rect.center();
    let tip = if up { -2.0 } else { 2.0 };
    let colour = if enabled { BLACK } else { SHADOW };
    ui.painter().add(Shape::convex_polygon(
        vec![
            Pos2::new(c.x - 3.0, c.y - tip / 2.0),
            Pos2::new(c.x + 3.0, c.y - tip / 2.0),
            Pos2::new(c.x, c.y + tip / 2.0),
        ],
        colour,
        Stroke::NONE,
    ));
}

/// the clock face: ticks, an hour and a minute hand and a red second hand.
fn analog(ui: &mut Ui, size: f32, (h, m, s): (u32, u32, u32)) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let (centre, radius) = (rect.center(), size / 2.0 - 2.0);
    let painter = ui.painter();
    painter.circle_filled(centre, radius, WHITE);
    painter.circle_stroke(centre, radius, Stroke::new(1.0, SHADOW));
    let direction = |degrees: f32| {
        let a = degrees.to_radians();
        Vec2::new(a.sin(), -a.cos())
    };
    for i in 0..12 {
        let long = i % 3 == 0;
        let (from, to) = (radius - if long { 9.0 } else { 5.0 }, radius - 3.0);
        let d = direction(i as f32 * 30.0);
        let width = if long { 2.0 } else { 1.0 };
        painter.line_segment(
            [centre + d * from, centre + d * to],
            Stroke::new(width, BLACK),
        );
    }
    let hands = [
        (((h % 12) as f32 + m as f32 / 60.0) * 30.0, 0.5, 3.0, BLACK),
        ((m as f32 + s as f32 / 60.0) * 6.0, 0.78, 2.0, BLACK),
        (s as f32 * 6.0, 0.82, 1.0, RED),
    ];
    for (degrees, length, width, colour) in hands {
        painter.line_segment(
            [centre, centre + direction(degrees) * radius * length],
            Stroke::new(width, colour),
        );
    }
    painter.circle_filled(centre, 2.5, BLACK);
}

/// the days of a month as a grid under the weekday letters; the day clicked, if any.
fn calendar(
    ui: &mut Ui,
    year: i32,
    month: u32,
    selected: NaiveDate,
    today: NaiveDate,
    editable: bool,
) -> Option<NaiveDate> {
    let cell = Vec2::new(26.0, 19.0);
    let (grid, _) = ui.allocate_exact_size(Vec2::new(cell.x * 7.0, cell.y * 7.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(grid, 0.0, WHITE);
    theme::bevel(painter, grid, false);
    for (i, name) in clock_window::WEEKDAYS.iter().enumerate() {
        let at = Rect::from_min_size(grid.min + Vec2::new(cell.x * i as f32, 0.0), cell);
        painter.text(
            at.center(),
            Align2::CENTER_CENTER,
            *name,
            theme::font(),
            SHADOW,
        );
    }
    let mut picked = None;
    for (i, date) in clock_window::month_days(year, month)
        .into_iter()
        .enumerate()
    {
        let at = Rect::from_min_size(
            grid.min + Vec2::new(cell.x * (i % 7) as f32, cell.y * (1 + i / 7) as f32),
            cell,
        );
        let (selected, in_month) = (date == selected, date.month() == month);
        let sense = if editable {
            Sense::click()
        } else {
            Sense::hover()
        };
        let response = ui.interact(at, ui.scope_id().with(("clock-day", date)), sense);
        let painter = ui.painter();
        if selected {
            painter.rect_filled(at.shrink(1.0), 0.0, NAVY);
        }
        let colour = match (selected, in_month) {
            (true, _) => WHITE,
            (false, true) => BLACK,
            (false, false) => SHADOW,
        };
        if date == today && !selected {
            painter.rect_stroke(
                at.shrink(2.0),
                0.0,
                Stroke::new(1.0, BLACK),
                egui::StrokeKind::Inside,
            );
        }
        painter.text(
            at.center(),
            Align2::CENTER_CENTER,
            date.day().to_string(),
            theme::font(),
            colour,
        );
        if response.clicked() {
            picked = Some(date);
        }
        #[cfg(test)]
        theme::probe::record(
            ui,
            &format!("day {}", date.format("%Y-%m-%d")),
            at,
            editable,
        );
    }
    picked
}

/// the parts of a progress bar left and right of its fill edge: the label is white on the
/// first and black on the second.
fn bar_clips(rect: Rect, edge: f32) -> (Rect, Rect) {
    let edge = edge.clamp(rect.left(), rect.right());
    (
        Rect::from_min_max(rect.min, Pos2::new(edge, rect.bottom())),
        Rect::from_min_max(Pos2::new(edge, rect.top()), rect.max),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit;
    use lanlord_app::view::Button;

    #[test]
    fn a_disabled_fixed_width_button_is_grey_and_reports_no_click() {
        let ctx = egui::Context::default();
        let nodes = vec![
            Node::Button(Button::new("Open", false, 7).width(100.0)),
            Node::Button(Button::new("Go", true, 8).width(100.0)),
        ];
        let mut sent = Vec::new();
        testkit::run(&ctx, vec![], |ui| {
            sent.extend(show(ui, &nodes));
        });
        let (_, off) = crate::theme::probe::find(&ctx, "Open").expect("drawn");
        assert!(!off, "reported as disabled");
        let at = crate::theme::probe::find(&ctx, "Open").unwrap().0.center();
        for _ in 0..2 {
            sent.clear();
            testkit::run(&ctx, testkit::click(at), |ui| {
                sent.extend(show(ui, &nodes));
            });
            assert!(sent.is_empty(), "{sent:?}");
        }
        let go = crate::theme::probe::find(&ctx, "Go").unwrap().0.center();
        testkit::run(&ctx, testkit::click(go), |ui| {
            sent.extend(show(ui, &nodes));
        });
        testkit::run(&ctx, vec![], |ui| {
            sent.extend(show(ui, &nodes));
        });
        assert_eq!(sent, [8]);
    }

    /// the height a window of `nodes` takes after one frame on `ctx`.
    fn height(ctx: &egui::Context, nodes: &[Node<u8>]) -> f32 {
        let mut used = 0.0;
        testkit::run(ctx, vec![], |ui| {
            let top = ui.cursor().top();
            show(ui, nodes);
            used = ui.cursor().top() - top;
        });
        used
    }

    fn with_status(text: &str) -> Vec<Node<u8>> {
        vec![
            Node::Text("body".into()),
            Node::Gap(6.0),
            Node::Status {
                text: text.into(),
                error: false,
            },
        ]
    }

    #[test]
    fn an_empty_status_bar_and_its_gap_take_no_room() {
        let ctx = egui::Context::default();
        let bare = height(&ctx, &[Node::Text("body".into())]);
        let empty = height(&ctx, &with_status(""));
        assert!((empty - bare).abs() < 0.01, "{empty} vs {bare}");
    }

    #[test]
    fn a_status_bar_takes_the_room_a_window_has_left_over_to_sit_at_the_bottom() {
        let ctx = egui::Context::default();
        let nodes = with_status("ready");
        let tight = height(&ctx, &nodes);
        let mut roomy = 0.0;
        for _ in 0..30 {
            controls::lend_room(&ctx, 100.0, || roomy = height(&ctx, &nodes));
        }
        let mut tight = tight;
        for _ in 0..30 {
            tight = height(&ctx, &nodes);
        }
        assert!((roomy - tight - 100.0).abs() < 0.01, "{roomy} vs {tight}");
    }

    #[test]
    fn a_status_bar_slides_in_with_its_text_and_out_when_it_clears() {
        let ctx = egui::Context::default();
        let empty = height(&ctx, &with_status(""));
        let shown = with_status("ready");
        let first = height(&ctx, &shown);
        assert!(first > empty, "starts to grow");
        let mut full = first;
        for _ in 0..30 {
            full = height(&ctx, &shown);
        }
        assert!(full > first, "keeps growing: {first} then {full}");
        assert!(full >= empty + controls::STATUS_HEIGHT + 6.0 - 0.01);
        let first_out = height(&ctx, &with_status(""));
        assert!(first_out < full, "starts to shrink");
        let mut gone = first_out;
        for _ in 0..30 {
            gone = height(&ctx, &with_status(""));
        }
        assert!((gone - empty).abs() < 0.01, "{gone} vs {empty}");
    }

    #[test]
    fn reduced_motion_shows_and_hides_the_bar_at_once() {
        let ctx = egui::Context::default();
        let id = egui::Id::NULL.with("slide");
        assert_eq!(slide(&ctx, id, true, true), 1.0);
        assert_eq!(slide(&ctx, id, false, true), 0.0);
        slide(&ctx, id, false, false);
        assert!(slide(&ctx, id, true, false) < 1.0, "animates otherwise");
    }

    #[test]
    fn a_status_message_never_moves_or_covers_the_content_above_it() {
        let ctx = egui::Context::default();
        let page = |text: &str| {
            vec![
                Node::Button(Button::new("Open", true, 1).width(100.0)),
                Node::Gap(6.0),
                Node::Status {
                    text: text.into(),
                    error: false,
                },
            ]
        };
        let button = |ctx: &egui::Context, nodes: &[Node<u8>]| {
            testkit::run(ctx, vec![], |ui| {
                show(ui, nodes);
            });
            crate::theme::probe::find(ctx, "Open").expect("drawn").0
        };
        let before = button(&ctx, &page(""));
        // mid-slide and fully slid in, the button stays where it was, with the bar below it
        for _ in 0..30 {
            let now = button(&ctx, &page("saved"));
            assert_eq!(now, before);
        }
        let mut bar = 0.0;
        testkit::run(&ctx, vec![], |ui| {
            show(ui, &page("saved"));
            bar = ui.min_rect().bottom();
        });
        assert!(bar >= before.bottom() + 6.0 + controls::STATUS_HEIGHT);
    }

    /// whether the text of `message` is drawn in a visible colour on a frame of `nodes`.
    fn drawn(
        ctx: &egui::Context,
        nodes: &[Node<String>],
        events: Vec<egui::Event>,
    ) -> (bool, Option<String>) {
        drawn_as(ctx, nodes, events, "not an address")
    }

    fn drawn_as(
        ctx: &egui::Context,
        nodes: &[Node<String>],
        events: Vec<egui::Event>,
        message: &str,
    ) -> (bool, Option<String>) {
        fn seen(shape: &Shape, message: &str) -> Option<bool> {
            match shape {
                Shape::Text(t) if t.galley.text() == message => {
                    Some(t.galley.job.sections[0].format.color != egui::Color32::TRANSPARENT)
                }
                Shape::Vec(shapes) => shapes.iter().find_map(|s| seen(s, message)),
                _ => None,
            }
        }
        let mut sent = None;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, testkit::SCREEN)),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            sent = show(ui, nodes).or(sent.take());
        });
        output.textures_delta.clear();
        let visible = output
            .shapes
            .iter()
            .find_map(|s| seen(&s.shape, message))
            .unwrap_or(false);
        (visible, sent)
    }

    #[test]
    fn an_address_is_not_called_wrong_while_it_is_typed_only_after_the_focus_leaves() {
        use lanlord_model::settings::ValidationError;
        let ctx = egui::Context::default();
        let errors = [ValidationError {
            path: "range".into(),
            message: "not an address".into(),
        }];
        let page = |text: &str, focus: bool| -> Vec<Node<String>> {
            let mut nodes = vec![Node::Input {
                value: text.to_string(),
                size: Size::Points(120.0),
                hint: String::new(),
                focus,
                edit: Box::new(|t| t),
                enter: None,
            }];
            // like a real panel, the error exists only once there is something to complain of
            let errors = if text.len() >= 3 { &errors[..] } else { &[] };
            nodes.extend(lanlord_app::view::address_errors(errors, "range", text));
            nodes
        };
        let mut text = String::new();
        for _ in 0..3 {
            drawn(&ctx, &page(&text, true), vec![]);
        }
        for ch in "192.0.2.10".chars() {
            let (shown, sent) = drawn(
                &ctx,
                &page(&text, false),
                vec![egui::Event::Text(ch.to_string())],
            );
            assert!(!shown, "error at {text:?} + {ch:?}");
            assert!(
                ctx.memory(|m| m.focused()).is_some(),
                "the field keeps the keyboard at {text:?} + {ch:?}"
            );
            text = sent.expect("the keystroke is an edit");
            // the text is never rewritten: it is exactly what was typed
            assert!(text.ends_with(ch));
        }
        assert_eq!(text, "192.0.2.10");
        // leaving a half-typed address with a click elsewhere shows the error
        let mut text = String::from("192.0.");
        for _ in 0..3 {
            drawn(&ctx, &page(&text, true), vec![]);
        }
        let away = testkit::click(Pos2::new(900.0, 600.0));
        let (shown, _) = drawn(&ctx, &page(&text, false), away);
        let (shown_later, _) = drawn(&ctx, &page(&text, false), vec![]);
        assert!(shown || shown_later, "error after the focus left");
        // a text that can never be valid is called wrong at once
        text.push('x');
        let (shown, _) = drawn(&ctx, &page(&text, true), vec![]);
        assert!(shown);
    }

    #[test]
    fn leaving_a_field_for_the_next_one_shows_the_error_of_the_one_left() {
        use lanlord_app::view::HeldFor;
        let ctx = egui::Context::default();
        let page = |first_has_it: bool| -> Vec<Node<String>> {
            let field = |focus: bool| Node::Input {
                value: "10.".into(),
                size: Size::Points(120.0),
                hint: String::new(),
                focus,
                edit: Box::new(|t| t),
                enter: None,
            };
            let held = |text: &str| Node::HeldError {
                text: text.into(),
                partial: true,
                field: HeldFor::Above(0),
            };
            vec![
                field(first_has_it),
                held("first is not an address"),
                field(!first_has_it),
                held("second is not an address"),
            ]
        };
        let seen = |first_has_it: bool, message: &str| {
            drawn_as(&ctx, &page(first_has_it), vec![], message).0
        };
        for _ in 0..4 {
            seen(true, "first is not an address");
        }
        assert!(
            !seen(true, "first is not an address"),
            "hidden in the first field"
        );
        assert!(
            seen(true, "second is not an address"),
            "the other one is not hidden"
        );
        // tab to the next field: the one left is called wrong, the one entered is held
        for _ in 0..4 {
            seen(false, "first is not an address");
        }
        assert!(
            seen(false, "first is not an address"),
            "shown after leaving"
        );
        assert!(
            !seen(false, "second is not an address"),
            "hidden in the second field"
        );
    }

    #[test]
    fn a_field_keeps_the_keyboard_when_its_page_is_measured_again() {
        let ctx = egui::Context::default();
        let page = |text: &str, focus: bool, err: bool| -> Vec<Node<String>> {
            let mut body = vec![Node::Input {
                value: text.to_string(),
                size: Size::Points(120.0),
                hint: String::new(),
                focus,
                edit: Box::new(|t| t),
                enter: None,
            }];
            if err {
                body.push(Node::FieldError("bad".into()));
            }
            let key = lanlord_app::view::fingerprint(&body);
            vec![Node::SameSize {
                key,
                body,
                others: vec![],
            }]
        };
        for _ in 0..2 {
            drawn(&ctx, &page("1", false, false), vec![]);
        }
        for event in testkit::click(Pos2::new(40.0, 14.0)) {
            drawn(&ctx, &page("1", false, false), vec![event]);
        }
        assert!(ctx.memory(|m| m.focused()).is_some(), "clicked");
        drawn(&ctx, &page("1", false, false), vec![]);
        assert!(ctx.memory(|m| m.focused()).is_some(), "before");
        drawn(&ctx, &page("1", false, true), vec![]);
        // the page is measured again (its key changed) and the field is still the one typed in
        assert!(ctx.memory(|m| m.focused()).is_some(), "after error appears");
    }

    /// the colours and clips the label of a progress bar was painted with, then the navy blocks.
    fn bar_label(fraction: f32) -> Vec<(egui::Color32, Rect)> {
        fn walk(shape: &Shape, clip: Rect, out: &mut Vec<(egui::Color32, Rect)>) {
            match shape {
                Shape::Vec(inner) => inner.iter().for_each(|s| walk(s, clip, out)),
                Shape::Text(t) if t.galley.text() == "50%" => {
                    out.push((t.override_text_color.unwrap_or(t.fallback_color), clip));
                }
                Shape::Rect(r) if r.fill == NAVY => out.push((NAVY, r.rect)),
                _ => {}
            }
        }
        let ctx = egui::Context::default();
        let nodes: Vec<Node<()>> = vec![Node::Bar {
            fraction,
            text: "50%".into(),
        }];
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, testkit::SCREEN)),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            show(ui, &nodes);
        });
        out.textures_delta.clear();
        let mut found = Vec::new();
        for clipped in &out.shapes {
            walk(&clipped.shape, clipped.clip_rect, &mut found);
        }
        found
    }

    #[test]
    fn the_bar_label_is_white_over_the_blocks_and_black_over_the_rest() {
        let found = bar_label(0.5);
        let white = found.iter().find(|(c, _)| *c == WHITE).expect("white half");
        // the white label never reaches past the last navy block, or it would be white on white
        let last = found
            .iter()
            .rev()
            .find(|(c, _)| *c == NAVY)
            .expect("blocks");
        assert!(white.1.right() <= last.1.right() + 0.01, "{found:?}");
        let black = found.iter().find(|(c, _)| *c == BLACK).expect("black half");
        // the two halves meet at the fill edge
        assert!((white.1.right() - black.1.left()).abs() < 0.5, "{found:?}");
        assert!(white.1.width() > 1.0 && black.1.width() > 1.0, "{found:?}");
        // an empty bar has no filled part to be white in
        assert!(bar_label(0.0)
            .iter()
            .all(|(c, r)| *c != WHITE || r.width() < 0.5));
    }

    #[test]
    fn a_fold_used_by_the_logon_and_account_windows_opens_and_closes_on_a_title_click() {
        let ctx = egui::Context::default();
        let nodes: Vec<Node<u8>> = vec![Node::Fold {
            id: "details",
            title: "Details".into(),
            body: vec![Node::Text("one".into()), Node::Text("two".into())],
        }];
        let height = std::cell::Cell::new(0.0);
        let drawn = |events| {
            testkit::run(&ctx, events, |ui| {
                show(ui, &nodes);
                height.set(ui.min_rect().height());
            })
        };
        for _ in 0..3 {
            drawn(vec![]);
        }
        let closed = height.get();
        let at = Pos2::new(40.0, 14.0);
        for open in [true, false] {
            for event in testkit::click(at) {
                drawn(vec![event]);
            }
            for _ in 0..3 {
                drawn(vec![]);
            }
            assert_eq!(
                height.get() > closed + 10.0,
                open,
                "{closed} {}",
                height.get()
            );
        }
    }
}
