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

fn title_bar(ui: &mut Ui, icon: &str, title: &str, active: bool) -> TitleActions {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, 0.0, if active { NAVY } else { SHADOW });
    icons::draw_small(ui.painter(), rect.left_top() + Vec2::new(3.0, 2.0), icon);
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
    let maximize = close.translate(Vec2::new(-size.x - 2.0, 0.0));
    let minimize = maximize.translate(Vec2::new(-size.x, 0.0));
    let base = ui.scope_id().with(title);
    let minimize = title_button(ui, minimize, base.with("min"), Glyph::Minimize);
    // the desktop has no use for maximize yet; it is drawn for the look
    title_button(ui, maximize, base.with("max"), Glyph::Maximize);
    TitleActions {
        close: title_button(ui, close, base.with("close"), Glyph::Close),
        minimize,
    }
}

fn padded(ui: &mut Ui, content: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .inner_margin(Margin::symmetric(7, 3))
        .show(ui, content);
}

/// show a window; returns what its title bar asked for and whether it was clicked. a `fixed`
/// window is a dialog: `size.x` wide, as tall as its contents, without resizing or scrolling.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    start: Pos2,
    size: Vec2,
    active: bool,
    fixed: bool,
    content: impl FnOnce(&mut Ui),
) -> Option<(TitleActions, bool)> {
    let mut actions = TitleActions::default();
    let window = egui::Window::new(title)
        .id(Id::unique(id))
        .title_bar(false)
        .collapsible(false)
        .default_pos(start)
        .frame(egui::Frame::new().fill(GRAY).inner_margin(Margin::same(3)));
    let window = if fixed {
        window
            .resizable(false)
            .auto_sized()
            .min_width(size.x)
            .max_width(size.x)
    } else {
        window
            .resizable(true)
            .default_size(size)
            .min_size(Vec2::new(320.0, 180.0))
    };
    let shown = window.show(ctx, |ui| {
        actions = title_bar(ui, id, title, active);
        ui.add_space(4.0);
        if fixed {
            padded(ui, content);
            return;
        }
        // a window smaller than its contents scrolls instead of cutting them off
        egui::ScrollArea::vertical()
            .id_salt(("window-scroll", id))
            .auto_shrink([false, false])
            .show(ui, |ui| padded(ui, content));
    })?;
    let painter = ctx.layer_painter(shown.response.layer_id);
    bevel(&painter, shown.response.rect, true);
    Some((actions, shown.response.is_pointer_button_down_on()))
}
