//! the LANlord menu: the windows, the servers, power actions and quitting.

use eframe::egui::{self, Align2, Area, Id, Order, Painter, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use lanlord_core::log::{emit, Kind};
use lanlord_core::power::Power;

pub use lanlord_app::menu::{self, Action, Row};

use super::{Desktop, MENU_BUTTON, TASKBAR};
use crate::form::{Form, FormAnswer, ShowForm};
use crate::icons;
use crate::theme::{self, bevel, BLACK, GRAY, NAVY, SHADOW, WHITE};

const MENU_WIDTH: f32 = 200.0;
const ROW: f32 = 26.0;
const SEPARATOR: f32 = 8.0;

/// what a confirmation box is about.
pub enum Confirm {
    /// reboot or power off this host; the host is fixed when asked.
    Power(Power, String),
    About,
    /// rebuild and relaunch; the box has one tick for pulling trunk first.
    Rebuild,
    /// a rebuild that did not work; the box only says why.
    Failure,
}

/// the menu's rows and box.
type Boxes = (Vec<Row>, Placed);
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
        let hot = response.hovered() && item.enabled && !item.header;
        if hot {
            ui.painter().rect_filled(*at, 0.0, NAVY);
        }
        let colour = match (hot, item.enabled || item.header) {
            (true, _) => WHITE,
            (false, true) => BLACK,
            (false, false) => SHADOW,
        };
        if let Some(art) = item.art {
            icons::draw(
                ui.painter(),
                at.left_top() + Vec2::new(4.0, 4.0),
                icons::small(art),
                1.0,
            );
        }
        if item.ticked {
            tick(ui.painter(), at.left_center() + Vec2::new(6.0, 0.0), colour);
        }
        let text_at = if item.art.is_some() || item.ticked || !strip {
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
        if response.clicked() && item.enabled && !item.header {
            chosen = Some(item.action.clone());
        }
    }
    chosen
}

impl Desktop {
    /// the main menu.
    pub(super) fn menu_rows(&self) -> Vec<Row> {
        let host = self.host();
        let who = self.signed_in.as_ref().map(|s| s.identity.user.clone());
        menu::rows(&menu::State {
            host: host.as_deref(),
            signed_in: self.auth.as_ref().map(|_| who.as_deref().unwrap_or("")),
            ready: host.as_ref().is_some_and(|h| !self.services.busy(h)),
            updates: self.updates.enabled(),
            developer: self.dev.is_some(),
        })
    }

    /// the line at the top of the menu naming the server every action targets.
    #[cfg(test)]
    pub(super) fn server_line(&self) -> String {
        menu::server_line(self.host().as_deref())
    }

    /// the Developer submenu, only there in a development build.
    pub(super) fn dev_rows(&self) -> Vec<Row> {
        menu::developer_rows(self.is_rebuilding())
    }

    /// where the Developer submenu sits, beside the Developer row of the main menu.
    fn dev_boxes(&self, rows: &[Row], main: &Placed) -> Option<Boxes> {
        if !self.dev_open {
            return None;
        }
        let at = rows.iter().zip(&main.1).find_map(|(row, rect)| match row {
            Row::Item(item) if item.action == Action::Developer => Some(*rect),
            _ => None,
        })?;
        let sub = self.dev_rows();
        let placed = geometry(&sub, main.0.right() - 2.0, at.bottom() + 7.0, false);
        Some((sub, placed))
    }

    /// where the menu sits on `screen`.
    fn menu_boxes(&self, screen: Rect) -> Boxes {
        let rows = self.menu_rows();
        let main = geometry(&rows, screen.left() + 2.0, screen.bottom() - TASKBAR, true);
        (rows, main)
    }

    /// the centre of the menu item that does `action`, for tests.
    #[cfg(test)]
    pub(super) fn menu_item_at(&self, screen: Rect, action: &Action) -> Option<Pos2> {
        let (rows, main) = self.menu_boxes(screen);
        rows.iter().zip(&main.1).find_map(|(row, rect)| match row {
            Row::Item(item) if item.action == *action => Some(rect.center()),
            _ => None,
        })
    }

    /// the centre of the Developer submenu item that does `action`, for tests.
    #[cfg(all(test, unix))]
    pub(super) fn dev_item_at(&self, screen: Rect, action: &Action) -> Option<Pos2> {
        let (rows, main) = self.menu_boxes(screen);
        let (sub, placed) = self.dev_boxes(&rows, &main)?;
        sub.iter().zip(&placed.1).find_map(|(row, rect)| match row {
            Row::Item(item) if item.action == *action => Some(rect.center()),
            _ => None,
        })
    }

    pub(super) fn start_menu(&mut self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        let (rows, main) = self.menu_boxes(screen);
        let dev = self.dev_boxes(&rows, &main);
        let mut chosen = Area::new(Id::unique("start-menu"))
            .order(Order::Foreground)
            .fixed_pos(main.0.min)
            .show(ctx, |ui| {
                ui.allocate_exact_size(main.0.size(), Sense::hover());
                paint(ui, &rows, main.0, &main.1, true)
            })
            .inner;
        if let Some((sub, placed)) = &dev {
            let from_sub = Area::new(Id::unique("dev-menu"))
                .order(Order::Foreground)
                .fixed_pos(placed.0.min)
                .show(ctx, |ui| {
                    ui.allocate_exact_size(placed.0.size(), Sense::hover());
                    paint(ui, sub, placed.0, &placed.1, false)
                })
                .inner;
            chosen = chosen.or(from_sub);
        }
        match chosen {
            Some(action) => self.do_action(ctx, action),
            None => {
                // a click anywhere else closes the menu
                let pos = ctx.input(|i| i.pointer.interact_pos());
                let inside = |p: Pos2| {
                    main.0.contains(p)
                        || dev.as_ref().is_some_and(|(_, placed)| placed.0.contains(p))
                };
                let outside = ctx.input(|i| i.pointer.any_pressed())
                    && pos.is_some_and(|p| !inside(p) && p.y < screen.bottom() - TASKBAR);
                if outside {
                    self.start_open = false;
                    self.dev_open = false;
                }
            }
        }
    }

    pub(super) fn do_action(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::Developer => {
                // the submenu opens beside the row; the menu stays up
                self.dev_open = !self.dev_open;
                return;
            }
            Action::ToggleUpdates => {
                let on = !self.updates.enabled();
                self.updates.set_enabled(on);
                if on {
                    self.updates.check_now(ctx);
                }
            }
            Action::CheckUpdates => {
                emit(Kind::Info, "looking for updates");
                self.updates.check_now(ctx);
            }
            Action::Rebuild => self.ask_rebuild(),
            Action::Open(target) => self.open(target),
            Action::SwitchServer | Action::LogOff => self.log_off(),
            Action::Power(power) => self.ask_power(power),
            Action::About => {
                let text = format!(
                    "LANlord {}\nManages a retro network gateway: DNS, DHCP, an NT4 domain and more.",
                    env!("CARGO_PKG_VERSION")
                );
                self.confirm = Some((Confirm::About, Form::new("about", "About LANlord", &text)));
            }
            Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Action::ServerLine => {}
        }
        self.start_open = false;
        self.dev_open = false;
    }

    fn ask_rebuild(&mut self) {
        let Some(repo) = self.dev.clone() else { return };
        let text = format!(
            "Rebuild LANlord from {} and start it again?\nThis window closes when the build works, and you sign in again.",
            repo.display()
        );
        self.confirm = Some((
            Confirm::Rebuild,
            Form::new("rebuild", "Rebuild and relaunch", &text)
                .check("Pull the latest trunk first", false),
        ));
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
            Some(FormAnswer::Ok) => match kind {
                Confirm::Power(power, host) => {
                    if !self.services.power(&host, power) {
                        emit(Kind::Err, format!("{host} is busy; try again in a moment"));
                    }
                }
                Confirm::Rebuild => self.start_rebuild(form.checked(0)),
                Confirm::About | Confirm::Failure => {}
            },
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
        icons::draw(
            ui.painter(),
            rect.left_top() + Vec2::new(5.0, 3.0) + nudge,
            icons::small("lanlord"),
            1.0,
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
