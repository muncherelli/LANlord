//! the LANlord menu: the windows, the servers, power actions and quitting.

use eframe::egui::{self, Align2, Area, Id, Order, Painter, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use lanlord_core::log::{emit, Kind};
use lanlord_core::modules::MODULES;
use lanlord_core::power::Power;

use super::{Desktop, Target, MENU_BUTTON, TASKBAR};
use crate::dialogs::TextDialog;
use crate::form::{Form, FormAnswer};
use crate::icons;
use crate::theme::{self, bevel, BLACK, GRAY, NAVY, SHADOW, WHITE};

const MENU_WIDTH: f32 = 200.0;
const ROW: f32 = 26.0;
const SEPARATOR: f32 = 8.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Open(Target),
    /// open or close the list of servers.
    Servers,
    SwitchTo(usize),
    AddServer,
    Power(Power),
    About,
    Quit,
}

pub enum Row {
    Item(Item),
    Separator,
}

pub struct Item {
    label: String,
    art: Option<&'static str>,
    action: Action,
    ticked: bool,
    arrow: bool,
    enabled: bool,
}

impl Item {
    fn new(label: &str, action: Action) -> Self {
        Self {
            label: label.into(),
            art: None,
            action,
            ticked: false,
            arrow: false,
            enabled: true,
        }
    }
}

/// what a confirmation box is about.
pub enum Confirm {
    /// reboot or power off this host; the host is fixed when asked.
    Power(Power, String),
    About,
}

/// the main menu's rows and box, and the server list's when it is open.
type Boxes = (Vec<Row>, Placed, Option<(Vec<Row>, Placed)>);
/// a menu's box and the rectangle of each of its rows.
type Placed = (Rect, Vec<Rect>);

fn row_height(row: &Row) -> f32 {
    match row {
        Row::Item(_) => ROW,
        Row::Separator => SEPARATOR,
    }
}

/// the box and the rectangle of each row, with the box's bottom edge at `bottom`.
fn geometry(rows: &[Row], left: f32, bottom: f32, strip: bool) -> (Rect, Vec<Rect>) {
    let height = rows.iter().map(row_height).sum::<f32>() + 14.0;
    let rect = Rect::from_min_size(
        Pos2::new(left, bottom - height),
        Vec2::new(MENU_WIDTH, height),
    );
    let x = rect.left() + if strip { 28.0 } else { 4.0 };
    let width = rect.right() - 4.0 - x;
    let mut y = rect.top() + 8.0;
    let rects = rows
        .iter()
        .map(|row| {
            let rect =
                Rect::from_min_size(Pos2::new(x, y), Vec2::new(width, row_height(row) - 2.0));
            y += row_height(row);
            rect
        })
        .collect();
    (rect, rects)
}

fn tick(painter: &Painter, at: Pos2, colour: egui::Color32) {
    let stroke = Stroke::new(1.5, colour);
    painter.line_segment([at + Vec2::new(0.0, 2.0), at + Vec2::new(3.0, 5.0)], stroke);
    painter.line_segment(
        [at + Vec2::new(3.0, 5.0), at + Vec2::new(8.0, -3.0)],
        stroke,
    );
}

/// paint one menu and return the enabled item that was clicked.
fn paint(ui: &mut Ui, rows: &[Row], rect: Rect, rects: &[Rect], strip: bool) -> Option<Action> {
    ui.painter().rect_filled(rect, 0.0, GRAY);
    bevel(ui.painter(), rect, true);
    if strip {
        let bar = Rect::from_min_size(
            rect.min + Vec2::splat(3.0),
            Vec2::new(22.0, rect.height() - 6.0),
        );
        ui.painter().rect_filled(bar, 0.0, NAVY);
    }
    let mut chosen = None;
    for (n, (row, at)) in rows.iter().zip(rects).enumerate() {
        let Row::Item(item) = row else {
            let y = at.center().y;
            let (left, right) = (at.left(), at.right());
            ui.painter().line_segment(
                [Pos2::new(left, y), Pos2::new(right, y)],
                Stroke::new(1.0, SHADOW),
            );
            ui.painter().line_segment(
                [Pos2::new(left, y + 1.0), Pos2::new(right, y + 1.0)],
                Stroke::new(1.0, WHITE),
            );
            continue;
        };
        let response = ui.interact(*at, Id::unique(("menu-item", n, strip)), Sense::click());
        let hot = response.hovered() && item.enabled;
        if hot {
            ui.painter().rect_filled(*at, 0.0, NAVY);
        }
        let colour = match (hot, item.enabled) {
            (true, _) => WHITE,
            (false, true) => BLACK,
            (false, false) => SHADOW,
        };
        if let Some(art) = item.art {
            icons::draw(
                ui.painter(),
                at.left_top() + Vec2::new(4.0, 4.0),
                icons::art(art),
                1.0,
            );
        }
        if item.ticked {
            tick(ui.painter(), at.left_center() + Vec2::new(6.0, 0.0), colour);
        }
        let text_at = if item.art.is_some() || !strip {
            28.0
        } else {
            6.0
        };
        ui.painter().text(
            at.left_center() + Vec2::new(text_at, 0.0),
            Align2::LEFT_CENTER,
            &item.label,
            theme::font(),
            colour,
        );
        if item.arrow {
            ui.painter().text(
                at.right_center() - Vec2::new(8.0, 0.0),
                Align2::CENTER_CENTER,
                ">",
                theme::font(),
                colour,
            );
        }
        if response.clicked() && item.enabled {
            chosen = Some(item.action.clone());
        }
    }
    chosen
}

impl Desktop {
    /// the main menu.
    pub(super) fn menu_rows(&self) -> Vec<Row> {
        let mut rows: Vec<Row> = MODULES
            .iter()
            .enumerate()
            .map(|(i, m)| {
                Row::Item(Item {
                    art: Some(m.id),
                    ..Item::new(m.name, Action::Open(Target::Module(i)))
                })
            })
            .collect();
        for target in [Target::Settings, Target::Console] {
            let item = Item::new(target.title(), Action::Open(target));
            rows.push(Row::Item(Item {
                art: Some(target.id()),
                ..item
            }));
        }
        rows.push(Row::Separator);
        if !self.services.is_local() {
            rows.push(Row::Item(Item {
                arrow: true,
                ..Item::new("Switch server", Action::Servers)
            }));
        }
        let ready = self.host().is_some_and(|h| !self.services.busy(&h));
        for (label, power) in [
            ("Reboot server...", Power::Reboot),
            ("Shut down server...", Power::PowerOff),
        ] {
            rows.push(Row::Item(Item {
                enabled: ready,
                ..Item::new(label, Action::Power(power))
            }));
        }
        rows.push(Row::Separator);
        rows.push(Row::Item(Item::new("About LANlord", Action::About)));
        rows.push(Row::Item(Item::new("Quit LANlord", Action::Quit)));
        rows
    }

    /// the saved servers, the current one ticked, and a way to add one.
    pub(super) fn submenu_rows(&self) -> Vec<Row> {
        let mut rows: Vec<Row> = self
            .hosts
            .hosts()
            .iter()
            .enumerate()
            .map(|(i, host)| {
                let ticked = self.hosts.selected_index() == i && self.hosts.selected().is_some();
                Row::Item(Item {
                    ticked,
                    ..Item::new(host, Action::SwitchTo(i))
                })
            })
            .collect();
        if !rows.is_empty() {
            rows.push(Row::Separator);
        }
        rows.push(Row::Item(Item::new("Add server...", Action::AddServer)));
        rows
    }

    /// where the menu and, when open, the server list sit on `screen`.
    fn menu_boxes(&self, screen: Rect) -> Boxes {
        let rows = self.menu_rows();
        let main = geometry(&rows, screen.left() + 2.0, screen.bottom() - TASKBAR, true);
        let sub = (self.submenu && !self.services.is_local()).then(|| {
            let sub_rows = self.submenu_rows();
            let anchor = rows
                .iter()
                .position(|r| matches!(r, Row::Item(i) if i.action == Action::Servers))
                .map_or(main.0.top(), |n| main.1[n].top());
            let height = sub_rows.iter().map(row_height).sum::<f32>() + 14.0;
            let bottom = (anchor + height).min(screen.bottom() - TASKBAR);
            let sub = geometry(&sub_rows, main.0.right() - 2.0, bottom, false);
            (sub_rows, sub)
        });
        (rows, main, sub)
    }

    /// the centre of the menu item that does `action`, for tests.
    #[cfg(test)]
    pub(super) fn menu_item_at(&self, screen: Rect, action: &Action) -> Option<Pos2> {
        let (rows, main, sub) = self.menu_boxes(screen);
        let find = |rows: &[Row], rects: &[Rect]| {
            rows.iter().zip(rects).find_map(|(row, rect)| match row {
                Row::Item(item) if item.action == *action => Some(rect.center()),
                _ => None,
            })
        };
        find(&rows, &main.1).or_else(|| sub.and_then(|(rows, (_, rects))| find(&rows, &rects)))
    }

    pub(super) fn start_menu(&mut self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        let (rows, main, sub) = self.menu_boxes(screen);
        let mut chosen = Area::new(Id::unique("start-menu"))
            .order(Order::Foreground)
            .fixed_pos(main.0.min)
            .show(ctx, |ui| {
                ui.allocate_exact_size(main.0.size(), Sense::hover());
                paint(ui, &rows, main.0, &main.1, true)
            })
            .inner;
        if let Some((sub_rows, (rect, rects))) = &sub {
            let picked = Area::new(Id::unique("start-servers"))
                .order(Order::Foreground)
                .fixed_pos(rect.min)
                .show(ctx, |ui| {
                    ui.allocate_exact_size(rect.size(), Sense::hover());
                    paint(ui, sub_rows, *rect, rects, false)
                })
                .inner;
            chosen = chosen.or(picked);
        }
        match chosen {
            Some(action) => self.do_action(ctx, action),
            None => {
                // a click anywhere else closes the menu
                let pos = ctx.input(|i| i.pointer.interact_pos());
                let inside = |at: Pos2| {
                    main.0.contains(at) || sub.as_ref().is_some_and(|s| (s.1).0.contains(at))
                };
                let outside = ctx.input(|i| i.pointer.any_pressed())
                    && pos.is_some_and(|p| !inside(p) && p.y < screen.bottom() - TASKBAR);
                if outside {
                    self.start_open = false;
                    self.submenu = false;
                }
            }
        }
    }

    pub(super) fn do_action(&mut self, ctx: &egui::Context, action: Action) {
        if action == Action::Servers {
            self.submenu = !self.submenu;
            return;
        }
        match action {
            Action::Open(target) => self.open(target),
            Action::SwitchTo(i) => self.hosts.select(i),
            Action::AddServer => {
                if self.add_host.is_none() {
                    self.add_host = Some(TextDialog::new(
                        "add-host",
                        "Add a host",
                        "Gateway as user@host:",
                        false,
                    ));
                }
            }
            Action::Power(power) => self.ask_power(power),
            Action::About => {
                let text = format!(
                    "LANlord {}\nManages a retro network gateway: DNS, DHCP, an NT4 domain and more.",
                    env!("CARGO_PKG_VERSION")
                );
                self.confirm = Some((Confirm::About, Form::new("about", "About LANlord", &text)));
            }
            Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Action::Servers => {}
        }
        self.start_open = false;
        self.submenu = false;
    }

    fn ask_power(&mut self, power: Power) {
        let Some(host) = self.host() else { return };
        let (title, verb) = match power {
            Power::Reboot => ("Reboot server", "Reboot"),
            Power::PowerOff => ("Shut down server", "Shut down"),
        };
        let text = format!(
            "{verb} {host} now?\nAnything running on it stops. You may be asked for the sudo password."
        );
        self.confirm = Some((
            Confirm::Power(power, host),
            Form::new("power", title, &text),
        ));
    }

    /// the box asking before anything destructive runs; nothing happens until OK.
    pub(super) fn confirmation(&mut self, ctx: &egui::Context) {
        let Some((kind, mut form)) = self.confirm.take() else {
            return;
        };
        match form.show(ctx) {
            None => self.confirm = Some((kind, form)),
            Some(FormAnswer::Cancel) => {}
            Some(FormAnswer::Ok) => {
                if let Confirm::Power(power, host) = kind {
                    if !self.services.power(&host, power) {
                        emit(Kind::Err, format!("{host} is busy; try again in a moment"));
                    }
                }
            }
        }
    }

    pub(super) fn menu_button(&self, ui: &mut Ui) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(MENU_BUTTON, 22.0), Sense::click());
        let down = self.start_open || response.is_pointer_button_down_on();
        ui.painter().rect_filled(rect, 0.0, GRAY);
        bevel(ui.painter(), rect, !down);
        let nudge = if down {
            Vec2::new(1.0, 1.0)
        } else {
            Vec2::ZERO
        };
        let logo = Rect::from_min_size(
            rect.left_top() + Vec2::new(6.0, 4.0) + nudge,
            Vec2::splat(14.0),
        );
        ui.painter().rect_filled(logo, 0.0, NAVY);
        ui.painter().text(
            logo.center(),
            Align2::CENTER_CENTER,
            "L",
            theme::font(),
            WHITE,
        );
        ui.painter().text(
            rect.left_center() + Vec2::new(26.0, 0.0) + nudge,
            Align2::LEFT_CENTER,
            "LANlord",
            egui::FontId::proportional(14.0),
            BLACK,
        );
        response
    }
}
