//! draws the app's layout tree (`lanlord_app::view`) with egui, in the nt4 look, and returns
//! the message of whatever was clicked.

use eframe::egui::{self, Frame, Margin, RichText, ScrollArea, Stroke, Ui};
use lanlord_app::view::{Dialog, Node};

use crate::controls;
use crate::dialogs;
use crate::remote::RED;
use crate::theme::{self, WHITE};
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
        nodes_ui(ui, &dialog.body)
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
        Node::Row(nodes) => ui.horizontal(|ui| show(ui, nodes)).inner,
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
    }
}
