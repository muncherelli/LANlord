//! the taskbar alert for a server that lacks what its components need: a yellow warning sign
//! beside the clock and, once after logon, a balloon over it. what is wrong, and for which
//! components, is decided in `lanlord_model::compat`; this only draws it.

use std::time::Duration;

use eframe::egui::{
    self, Align2, Area, Color32, Frame, Id, Margin, Order, Pos2, Rect, Sense, Shape, Stroke, Ui,
    Vec2,
};

use crate::theme::BLACK;

/// the text of the balloon.
pub const BALLOON_TEXT: &str =
    "This server is missing features some components need. Click for details.";

/// how long the balloon stays up.
const BALLOON_FOR: Duration = Duration::from_secs(10);

/// the state of the alert: has its balloon been shown yet, and since when is it up.
#[derive(Default)]
pub struct Alert {
    since: Option<f64>,
    done: bool,
}

impl Alert {
    /// a new logon: the balloon may go up again.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// is the balloon up?
    #[cfg(test)]
    pub fn balloon_up(&self) -> bool {
        self.since.is_some()
    }

    /// the alert is raised: put the balloon up the first time, take it down when it times out
    /// or when nothing is wrong any more.
    pub fn tick(&mut self, raised: bool, now: f64) {
        if !raised {
            self.since = None;
            return;
        }
        if self.since.is_none() && !self.done {
            self.since = Some(now);
            self.done = true;
        }
        if self
            .since
            .is_some_and(|s| now - s > BALLOON_FOR.as_secs_f64())
        {
            self.since = None;
        }
    }

    /// take the balloon down: it was clicked or closed.
    pub fn dismiss(&mut self) {
        self.since = None;
        self.done = true;
    }

    /// the balloon over the clock; true when it was clicked.
    pub fn balloon(&mut self, ctx: &egui::Context, taskbar: f32) -> bool {
        if self.since.is_none() {
            return false;
        }
        ctx.request_repaint_after(Duration::from_millis(500));
        let mut clicked = false;
        let mut closed = false;
        Area::new(Id::unique("compat-balloon"))
            .order(Order::Tooltip)
            .anchor(Align2::RIGHT_BOTTOM, Vec2::new(-8.0, -(taskbar + 10.0)))
            .show(ctx, |ui| {
                let framed = Frame::new()
                    .fill(Color32::from_rgb(255, 255, 225))
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(250.0);
                        ui.horizontal(|ui| {
                            let (at, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                            warning_sign(ui, at);
                            ui.colored_label(
                                BLACK,
                                egui::RichText::new("LANlord compatibility").strong(),
                            );
                        });
                        ui.colored_label(BLACK, BALLOON_TEXT);
                    });
                let rect = framed.response.rect;
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    Stroke::new(1.0, BLACK),
                    egui::StrokeKind::Inside,
                );
                let body = ui.interact(rect, Id::unique("compat-balloon-body"), Sense::click());
                let mark = Rect::from_center_size(
                    rect.right_top() + Vec2::new(-10.0, 10.0),
                    Vec2::splat(12.0),
                );
                let close = ui.interact(mark, Id::unique("compat-balloon-close"), Sense::click());
                let c = mark.center();
                for (from, to) in [((-3.0, -3.0), (3.0, 3.0)), ((-3.0, 3.0), (3.0, -3.0))] {
                    ui.painter().line_segment(
                        [c + Vec2::new(from.0, from.1), c + Vec2::new(to.0, to.1)],
                        Stroke::new(1.5, BLACK),
                    );
                }
                #[cfg(test)]
                {
                    crate::theme::probe::record(ui, "compat-balloon", rect, true);
                    crate::theme::probe::record(ui, "compat-balloon-close", mark, true);
                }
                if close.clicked() {
                    closed = true;
                } else if body.clicked() {
                    clicked = true;
                }
            });
        if closed || clicked {
            self.dismiss();
        }
        clicked
    }
}

/// a yellow triangle with a black exclamation mark, filling `rect`.
pub fn warning_sign(ui: &Ui, rect: Rect) {
    let c = rect.center();
    let (w, h) = (rect.width() * 0.5, rect.height() * 0.5);
    let top = Pos2::new(c.x, c.y - h + 1.0);
    let left = Pos2::new(c.x - w + 1.0, c.y + h - 1.0);
    let right = Pos2::new(c.x + w - 1.0, c.y + h - 1.0);
    let painter = ui.painter();
    painter.add(Shape::convex_polygon(
        vec![top, right, left],
        Color32::from_rgb(255, 221, 0),
        Stroke::new(1.0, BLACK),
    ));
    let stroke = Stroke::new(1.5, BLACK);
    painter.line_segment(
        [
            Pos2::new(c.x, c.y - h * 0.3),
            Pos2::new(c.x, c.y + h * 0.25),
        ],
        stroke,
    );
    painter.circle_filled(Pos2::new(c.x, c.y + h * 0.6), 0.9, BLACK);
}

/// the sign in the tray; true when it was clicked.
pub fn tray_sign(ui: &mut Ui, rect: Rect, tip: &str) -> bool {
    let response = ui
        .interact(rect, ui.scope_id().with("compat-tray"), Sense::click())
        .on_hover_text(tip);
    warning_sign(ui, Rect::from_center_size(rect.center(), Vec2::splat(16.0)));
    #[cfg(test)]
    crate::theme::probe::record(ui, "compat-tray", rect, true);
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_balloon_goes_up_once_per_logon_and_only_while_raised() {
        let mut alert = Alert::default();
        alert.tick(false, 0.0);
        assert!(!alert.balloon_up(), "nothing wrong, nothing shown");
        alert.tick(true, 1.0);
        assert!(alert.balloon_up());
        alert.tick(true, 100.0);
        assert!(!alert.balloon_up(), "timed out");
        alert.tick(true, 101.0);
        assert!(!alert.balloon_up(), "not again");
        alert.reset();
        alert.tick(true, 102.0);
        assert!(alert.balloon_up(), "a new logon");
        alert.dismiss();
        assert!(!alert.balloon_up());
        alert.tick(true, 103.0);
        assert!(!alert.balloon_up());
    }
}
