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

/// where a window goes and how much room it has: the desktop above the taskbar.
#[derive(Clone, Copy)]
pub struct Placement {
    /// the window's top left for a document window, its middle for a dialog.
    pub start: Pos2,
    /// the width it opens at.
    pub width: f32,
    /// the desktop it has to stay inside.
    pub area: Rect,
    /// for a window whose contents can grow (a console, a list, a text), the height it opens at
    /// instead of the contents' own: it then keeps whatever size the user gives it and its
    /// growable part takes the room. `None` opens the window as tall as its contents.
    pub height: Option<f32>,
}

/// what a window gave back: what its title bar asked for, whether it was clicked and where it is.
pub struct Shown {
    pub actions: TitleActions,
    pub clicked: bool,
    pub rect: Rect,
    /// the height the contents asked for, with their margins.
    pub content: f32,
}

/// the height a window spends on its own border, title bar, margins and the spacing between them.
const CHROME: f32 = 26.0;

/// show a window. every window opens as tall as what is in it, with an 8 pixel margin all round,
/// and when that is more than the desktop it stops at the desktop and scrolls. a `dialog` has a
/// close button only, cannot be resized and opens centred on `place.start`; any other window has
/// the three buttons, can be resized and opens with its top left there, moved if need be to stay
/// on the desktop.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    place: Placement,
    active: bool,
    dialog: bool,
    content: impl FnOnce(&mut Ui),
) -> Option<Shown> {
    let mut actions = TitleActions::default();
    let mut content_height = 0.0;
    let width = place.width;
    let max_height = place.area.height();
    let mut window = egui::Window::new(title)
        .id(Id::unique(id))
        .title_bar(false)
        .collapsible(false)
        .resizable(!dialog)
        .frame(egui::Frame::new().fill(GRAY).inner_margin(Margin::same(3)));
    window = if dialog {
        // `start` is where the middle of the window goes, so it stays centred once its size
        // is known
        window.pivot(Align2::CENTER_CENTER).default_pos(place.start)
    } else {
        let window = window.constrain_to(place.area).default_pos(place.start);
        match place.height {
            Some(h) => window
                .default_size(Vec2::new(width, h.min(max_height)))
                .min_size(Vec2::new(200.0, 100.0)),
            None => window.default_width(width).min_size(Vec2::new(200.0, 60.0)),
        }
    };
    let growable = place.height.is_some() && !dialog;
    let shown = window.show(ctx, |ui| {
        // the title bar spans the window inside its 3 pixel border; the contents sit in
        // an 8 pixel margin on every side
        if dialog {
            ui.set_width(width - 6.0);
        }
        let spacing = ui.spacing().item_spacing;
        ui.spacing_mut().item_spacing.y = 0.0;
        actions = title_bar(ui, id, title, active, dialog);
        // the contents are laid out in a space as tall as the desktop and the window takes the
        // size they use. asking the window's own ui would cap them at its last size, so a window
        // could never grow
        // a growable window gives its contents the room it has, so they follow its size
        let room = if growable {
            ui.available_height().max(40.0)
        } else {
            (max_height - CHROME).max(40.0)
        };
        let from = ui.cursor().min;
        let mut inner = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(
                    from,
                    Vec2::new(ui.available_width(), room),
                ))
                .layout(*ui.layout()),
        );
        egui::ScrollArea::vertical()
            .id_salt(("window-scroll", id))
            .max_height(room)
            .auto_shrink([false, !growable])
            .show(&mut inner, |ui| {
                let framed = egui::Frame::new()
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = spacing;
                        content(ui)
                    });
                content_height = framed.response.rect.height();
            });
        ui.allocate_rect(inner.min_rect(), Sense::hover());
    })?;
    let painter = ctx.layer_painter(shown.response.layer_id);
    bevel(&painter, shown.response.rect, true);
    Some(Shown {
        actions,
        clicked: shown.response.is_pointer_button_down_on(),
        rect: shown.response.rect,
        content: content_height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit;

    /// the height of a dialog holding `inner` points of content, on a screen `max` tall.
    fn shown(inner: f32, max: f32, dialog: bool) -> Rect {
        let ctx = egui::Context::default();
        let mut rect = None;
        for _ in 0..3 {
            testkit::run(&ctx, vec![], |ui| {
                let at = Pos2::new(480.0, 320.0);
                let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, max));
                let place = Placement {
                    start: at,
                    width: 300.0,
                    area,
                    height: None,
                };
                rect = show(ui.ctx(), "t", "T", place, true, dialog, |ui| {
                    ui.allocate_space(Vec2::new(10.0, inner));
                })
                .map(|s| s.rect);
            });
        }
        rect.expect("shown")
    }

    fn height(inner: f32, max: f32) -> f32 {
        shown(inner, max, true).height()
    }

    #[test]
    fn a_dialog_opens_centred_on_the_point_it_is_given() {
        // the screen is 960 by 640 and the dialog asks for its middle at (480, 320)
        for inner in [50.0, 150.0] {
            let rect = shown(inner, 600.0, true);
            assert!((rect.center().x - 480.0).abs() < 1.0, "{rect:?}");
            assert!((rect.center().y - 320.0).abs() < 1.0, "{rect:?}");
            assert!((rect.width() - 300.0).abs() < 1.0, "{rect:?}");
        }
    }

    #[test]
    fn a_dialog_is_as_tall_as_its_contents_and_no_taller_than_the_screen() {
        let short = height(50.0, 600.0);
        let tall = height(150.0, 600.0);
        assert!((tall - short - 100.0).abs() < 1.0, "{short} {tall}");
        // the 8 pixel margins above and below the contents, a 20 pixel title bar, a 3 pixel border
        assert!((short - (50.0 + 16.0 + 20.0 + 6.0)).abs() < 1.5, "{short}");
        // too much for a small screen: it stops at the screen and scrolls
        let clamped = height(2000.0, 300.0);
        assert!(clamped <= 300.0 + 1.0, "{clamped}");
        assert!(clamped > 250.0, "{clamped}");
    }

    #[test]
    fn a_document_window_grows_and_shrinks_with_its_contents_and_stays_on_the_desktop() {
        let ctx = egui::Context::default();
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 472.0));
        let run = |inner: f32| {
            let mut rect = None;
            for _ in 0..4 {
                testkit::run_on(&ctx, Vec2::new(800.0, 500.0), vec![], |ui| {
                    let place = Placement {
                        start: Pos2::new(110.0, 10.0),
                        width: 300.0,
                        area,
                        height: None,
                    };
                    rect = show(ui.ctx(), "doc", "Doc", place, true, false, |ui| {
                        ui.allocate_space(Vec2::new(10.0, inner));
                    })
                    .map(|s| s.rect);
                });
            }
            rect.unwrap()
        };
        let small = run(50.0);
        assert!(
            (small.height() - (50.0 + 16.0 + 26.0)).abs() < 1.5,
            "{small:?}"
        );
        let tall = run(300.0);
        assert!(
            (tall.height() - small.height() - 250.0).abs() < 1.5,
            "{tall:?}"
        );
        let huge = run(3000.0);
        assert!(huge.height() <= area.height() + 1.0, "{huge:?}");
        assert!(area.expand(1.0).contains_rect(huge), "{huge:?}");
    }

    #[test]
    fn the_contents_sit_the_same_distance_from_the_frame_on_the_left_right_and_bottom() {
        // a 3 pixel border and an 8 pixel margin: 11 pixels, for a dialog and a document window
        for dialog in [true, false] {
            let ctx = egui::Context::default();
            let mut got = None;
            for _ in 0..4 {
                testkit::run_on(&ctx, Vec2::new(800.0, 500.0), vec![], |ui| {
                    let place = Placement {
                        start: Pos2::new(400.0, 200.0),
                        width: 300.0,
                        area: Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 472.0)),
                        height: None,
                    };
                    let mut inner = Rect::NOTHING;
                    let shown = show(ui.ctx(), "t", "T", place, true, dialog, |ui| {
                        let wide = Vec2::new(ui.available_width(), 40.0);
                        inner = ui.allocate_exact_size(wide, Sense::hover()).0;
                    });
                    got = shown.map(|s| (s.rect, inner));
                });
            }
            let (frame, inner) = got.expect("shown");
            let left = inner.left() - frame.left();
            let right = frame.right() - inner.right();
            let bottom = frame.bottom() - inner.bottom();
            assert!(
                (left - 11.0).abs() < 0.5
                    && (right - 11.0).abs() < 0.5
                    && (bottom - 11.0).abs() < 0.5,
                "dialog {dialog}: left {left}, right {right}, bottom {bottom}"
            );
        }
    }
}
