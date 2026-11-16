//! draws the app's layout tree (`lanlord_app::view`) with egui, in the nt4 look, and returns
//! the message of whatever was clicked.

use eframe::egui::{self, Frame, Margin, RichText, ScrollArea, Stroke, TextEdit, Ui};
use lanlord_app::view::{Dialog, Node};

use crate::console;
use crate::controls;
use crate::dialogs;
use crate::icons;
use crate::remote::RED;
use crate::theme::{self, BLACK, WHITE};
use crate::widgets::Table;

/// draw `nodes` top to bottom; the message of a click, if there was one.
pub fn show<M: Clone>(ui: &mut Ui, nodes: &[Node<M>]) -> Option<M> {
    let mut sent = None;
    for node in nodes {
        if let Some(msg) = node_ui(ui, node) {
            sent = Some(msg);
        }
    }
    sent
}

/// draw a modal box; the message of a click inside it, if there was one.
pub fn dialog<M: Clone>(ctx: &egui::Context, dialog: &Dialog<M>) -> Option<M> {
    dialogs::modal(ctx, dialog.id, &dialog.title, |ui| {
        let sent = show(ui, &dialog.body);
        let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
        sent.or_else(|| escaped.then(|| dialog.escape.clone()).flatten())
    })
}

fn node_ui<M: Clone>(ui: &mut Ui, node: &Node<M>) -> Option<M> {
    match node {
        Node::Text(text) => {
            ui.label(text.as_str());
            None
        }
        Node::Gap(height) => {
            ui.add_space(*height);
            None
        }
        Node::Group { title, body } => controls::group(ui, title, |ui| show(ui, body)),
        Node::Table(table) => Table {
            id: table.id,
            headings: &table.headings,
            widths: &table.widths,
            rows: &table.rows,
            selected: table.selected,
            bad: &[],
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
            let inside = controls::tabs(ui, titles, &mut tab, |ui, _| show(ui, body));
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
                .show(ui, |ui| show(ui, body))
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
        Node::Input { value, width, edit } => {
            let mut text = value.clone();
            controls::edit(ui, &mut text, *width);
            (text != *value).then(|| edit(text))
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
        Node::Log { id, lines } => {
            let height = controls::fill_height(ui);
            theme::sunken(ui, BLACK, |ui| {
                ui.set_width(ui.available_width());
                ScrollArea::vertical()
                    .id_salt(*id)
                    .stick_to_bottom(true)
                    .max_height(height)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.set_min_height(height);
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
                    ui.label(RichText::new(title.as_str()).size(16.0).strong());
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
        Node::Row(body) => ui.horizontal(|ui| show(ui, body)).inner,
        Node::TextArea {
            id,
            value,
            height,
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
        Node::Entry {
            value,
            room,
            masked,
            focus,
            edit,
            enter,
        } => {
            let mut text = value.clone();
            let width = ui.available_width() - room;
            let response = controls::field(ui, |ui| {
                ui.add(
                    TextEdit::singleline(&mut text)
                        .password(*masked)
                        .desired_width(width)
                        .frame(egui::Frame::new())
                        .margin(egui::Margin::ZERO),
                )
            });
            if *focus {
                response.request_focus();
            }
            let pressed = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if pressed && enter.is_some() {
                return enter.clone();
            }
            (text != *value).then(|| edit(text))
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
