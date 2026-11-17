//! windows with a navy title bar and the three title-bar buttons.

use eframe::egui::{self, Align2, Id, Margin, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use crate::icons;
use crate::theme::{self, bevel, BLACK, GRAY, NAVY, SHADOW, WHITE};

/// what the title-bar buttons asked for this frame.
#[derive(Default)]
pub struct TitleActions {
    pub close: bool,
    pub minimize: bool,
}

fn title_button(ui: &mut Ui, rect: Rect, id: Id, glyph: Glyph) -> bool {
    let response = ui.interact(rect, id, Sense::click());
    let down = response.is_pointer_button_down_on();
    ui.painter().rect_filled(rect, 0.0, GRAY);
    bevel(ui.painter(), rect, !down);
    let c = rect.center()
        + if down {
            Vec2::new(1.0, 1.0)
        } else {
            Vec2::ZERO
        };
    let stroke = Stroke::new(1.5, BLACK);
    match glyph {
        Glyph::Minimize => {
            let y = c.y + 3.0;
            ui.painter()
                .line_segment([Pos2::new(c.x - 4.0, y), Pos2::new(c.x + 3.0, y)], stroke);
        }
        Glyph::Maximize => {
            ui.painter().rect_stroke(
                Rect::from_center_size(c, Vec2::splat(8.0)),
                0.0,
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        Glyph::Close => {
            for (a, b) in [((-3.5, -3.5), (3.5, 3.5)), ((-3.5, 3.5), (3.5, -3.5))] {
                ui.painter()
                    .line_segment([c + Vec2::new(a.0, a.1), c + Vec2::new(b.0, b.1)], stroke);
            }
        }
    }
    response.clicked()
}

#[derive(Clone, Copy)]
enum Glyph {
    Minimize,
    Maximize,
    Close,
}

fn title_bar(ui: &mut Ui, icon: &str, title: &str, active: bool, dialog: bool) -> TitleActions {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, 0.0, if active { NAVY } else { SHADOW });
    icons::draw(
        ui.painter(),
        rect.left_top() + Vec2::new(3.0, 2.0),
        icons::small(icon),
        1.0,
    );
    ui.painter().text(
        rect.left_center() + Vec2::new(23.0, 0.0),
        Align2::LEFT_CENTER,
        title,
        theme::font(),
        if active { WHITE } else { GRAY },
    );
    let size = Vec2::new(16.0, 14.0);
    let top = rect.top() + 3.0;
    let close = Rect::from_min_size(Pos2::new(rect.right() - 2.0 - size.x, top), size);
    let base = ui.scope_id().with(title);
    // a dialog has only a close button
    if dialog {
        return TitleActions {
            close: title_button(ui, close, base.with("close"), Glyph::Close),
            minimize: false,
        };
    }
    let maximize = close.translate(Vec2::new(-size.x - 2.0, 0.0));
    let minimize = maximize.translate(Vec2::new(-size.x, 0.0));
    let minimize = title_button(ui, minimize, base.with("min"), Glyph::Minimize);
    // the desktop has no use for maximize yet; it is drawn for the look
    title_button(ui, maximize, base.with("max"), Glyph::Maximize);
    TitleActions {
        close: title_button(ui, close, base.with("close"), Glyph::Close),
        minimize,
    }
}

/// show a window; returns what its title bar asked for and whether it was clicked. a `dialog`
/// has a close button only, cannot be resized, and fits its contents: `size.x` is its width and
/// its height is what the contents need, up to `max_height` (the desktop), past which it
/// scrolls.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    start: Pos2,
    size: Vec2,
    max_height: f32,
    active: bool,
    dialog: bool,
    content: impl FnOnce(&mut Ui),
) -> Option<(TitleActions, bool)> {
    let mut actions = TitleActions::default();
    if dialog {
        return dialog_window(ctx, id, title, start, size.x, max_height, active, content)
            .map(|(actions, clicked, _)| (actions, clicked));
    }
    let shown = egui::Window::new(title)
        .id(Id::unique(id))
        .title_bar(false)
        .collapsible(false)
        .resizable(true)
        .default_pos(start)
        .default_size(size)
        .min_size(Vec2::new(320.0, 180.0))
        .frame(egui::Frame::new().fill(GRAY).inner_margin(Margin::same(3)))
        .show(ctx, |ui| {
            actions = title_bar(ui, id, title, active, false);
            ui.add_space(4.0);
            // a window smaller than its contents scrolls instead of cutting them off
            egui::ScrollArea::vertical()
                .id_salt(("window-scroll", id))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Frame::new()
                        .inner_margin(Margin::symmetric(7, 3))
                        .show(ui, content);
                });
        })?;
    let painter = ctx.layer_painter(shown.response.layer_id);
    bevel(&painter, shown.response.rect, true);
    Some((actions, shown.response.is_pointer_button_down_on()))
}

/// the height a dialog spends on its own border, title bar, margins and the spacing between them.
const DIALOG_CHROME: f32 = 50.0;

/// a dialog: fixed width, as tall as what is in it (or as the desktop allows, then it scrolls),
/// with an 8 pixel margin all round. also gives the window's rectangle.
#[allow(clippy::too_many_arguments)]
fn dialog_window(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    start: Pos2,
    width: f32,
    max_height: f32,
    active: bool,
    content: impl FnOnce(&mut Ui),
) -> Option<(TitleActions, bool, Rect)> {
    let mut actions = TitleActions::default();
    let shown = egui::Window::new(title)
        .id(Id::unique(id))
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .default_pos(start)
        .frame(egui::Frame::new().fill(GRAY).inner_margin(Margin::same(3)))
        .show(ctx, |ui| {
            // the window's width less its own 3 pixel border and the 8 pixel margin
            ui.set_width(width - 22.0);
            actions = title_bar(ui, id, title, active, true);
            ui.add_space(8.0);
            egui::ScrollArea::vertical()
                .id_salt(("dialog-scroll", id))
                .max_height((max_height - DIALOG_CHROME).max(40.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    egui::Frame::new()
                        .inner_margin(Margin::symmetric(8, 0))
                        .show(ui, content);
                });
            ui.add_space(8.0);
        })?;
    let painter = ctx.layer_painter(shown.response.layer_id);
    bevel(&painter, shown.response.rect, true);
    Some((
        actions,
        shown.response.is_pointer_button_down_on(),
        shown.response.rect,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit;

    /// the height of a dialog holding `inner` points of content, on a screen `max` tall.
    fn height(inner: f32, max: f32) -> f32 {
        let ctx = egui::Context::default();
        let mut rect = None;
        for _ in 0..3 {
            testkit::run(&ctx, vec![], |ui| {
                let at = Pos2::new(10.0, 10.0);
                rect = dialog_window(ui.ctx(), "t", "T", at, 300.0, max, true, |ui| {
                    ui.allocate_space(Vec2::new(10.0, inner));
                })
                .map(|(_, _, rect)| rect);
            });
        }
        rect.expect("shown").height()
    }

    #[test]
    fn a_dialog_is_as_tall_as_its_contents_and_no_taller_than_the_screen() {
        let short = height(50.0, 600.0);
        let tall = height(150.0, 600.0);
        assert!((tall - short - 100.0).abs() < 1.0, "{short} {tall}");
        assert!(short >= 50.0 + 40.0, "{short}");
        // too much for a small screen: it stops at the screen and scrolls
        let clamped = height(2000.0, 300.0);
        assert!(clamped <= 300.0 + 1.0, "{clamped}");
        assert!(clamped > 250.0, "{clamped}");
    }
}
