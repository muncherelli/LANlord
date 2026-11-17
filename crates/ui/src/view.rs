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

/// draw `nodes` top to bottom; the message of a click, if there was one. a message changes the
/// window, so another frame is asked for to show it.
pub fn show<M: Clone>(ui: &mut Ui, nodes: &[Node<M>]) -> Option<M> {
    let sent = nodes_ui(ui, nodes);
    if sent.is_some() {
        ui.ctx().request_repaint();
    }
    sent
}

/// draw a modal box; the message of a click inside it, if there was one.
pub fn dialog<M: Clone>(ctx: &egui::Context, dialog: &Dialog<M>) -> Option<M> {
    let sent = dialogs::modal(ctx, dialog.id, &dialog.title, |ui| {
        let sent = nodes_ui(ui, &dialog.body);
        let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
        sent.or_else(|| escaped.then(|| dialog.escape.clone()).flatten())
    });
    if sent.is_some() {
        ctx.request_repaint();
    }
    sent
}

fn nodes_ui<M: Clone>(ui: &mut Ui, nodes: &[Node<M>]) -> Option<M> {
    let mut sent = None;
    for node in nodes {
        if let Some(msg) = node_ui(ui, node) {
            sent = Some(msg);
        }
    }
    sent
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
        Node::Table(table) => Table {
            id: table.id,
            headings: &table.headings,
            widths: &table.widths,
            rows: &table.rows,
            selected: table.selected,
            bad: &table.bad,
            height: table.height,
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
        Node::Status { text, error } => {
            controls::status_bar(ui, text, *error);
            None
        }
        Node::Heading(text) => {
            ui.label(RichText::new(text.as_str()).strong());
            None
        }
        Node::Button(button) => theme::action(ui, &button.label, button.enabled)
            .clicked()
            .then(|| button.press.clone()),
        Node::Tabs {
            titles,
            current,
            pick,
            body,
        } => {
            let mut tab = *current;
            let inside = controls::tabs(ui, titles, &mut tab, |ui, _| nodes_ui(ui, body));
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
        Node::Lines { id, lines, height } => {
            theme::sunken(ui, WHITE, |ui| {
                ui.set_width(ui.available_width());
                ScrollArea::vertical()
                    .id_salt(*id)
                    .max_height(*height)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.set_min_height(*height);
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
                None => {
                    let fill = controls::fill_height(ui);
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
            ui.push_id(*id, |ui| {
                theme::sunken(ui, WHITE, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height(*min_height);
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
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    text,
                    egui::FontId::proportional(12.0),
                    BLACK,
                );
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
            theme::sunken(ui, WHITE, |ui| {
                ui.set_width(ui.available_width());
                ScrollArea::vertical()
                    .id_salt(*id)
                    .max_height(*height)
                    .show(ui, |ui| {
                        ui.add_sized(
                            [ui.available_width(), *height],
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
        } => theme::sunken(ui, WHITE, |ui| {
            ui.set_width(ui.available_width());
            ScrollArea::vertical()
                .id_salt(*id)
                .max_height(*max_height)
                .show(ui, |ui| show(ui, body))
                .inner
        }),
    }
}

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
