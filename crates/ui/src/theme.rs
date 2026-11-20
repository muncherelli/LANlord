//! the classic desktop look: gray surfaces, two-pixel bevels, navy title bars.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Frame, Margin, Painter, Rect, Response, Sense,
    Stroke, Ui, Vec2,
};

/// how wide a scroll bar is, in points.
pub const SCROLL_BAR_WIDTH: f32 = 16.0;
pub const TEAL: Color32 = Color32::from_rgb(0, 128, 128);
pub const GRAY: Color32 = Color32::from_rgb(192, 192, 192);
pub const LIGHT: Color32 = Color32::from_rgb(223, 223, 223);
pub const SHADOW: Color32 = Color32::from_rgb(128, 128, 128);
pub const NAVY: Color32 = Color32::from_rgb(0, 0, 128);
pub const WHITE: Color32 = Color32::WHITE;
pub const BLACK: Color32 = Color32::BLACK;

pub const TEXT: f32 = 13.0;

pub fn font() -> FontId {
    FontId::proportional(TEXT)
}

fn edges(painter: &Painter, rect: Rect, top_left: Color32, bottom_right: Color32) {
    let (tl, tr, br, bl) = (
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    );
    painter.line_segment([bl, tl], Stroke::new(1.0, top_left));
    painter.line_segment([tl, tr], Stroke::new(1.0, top_left));
    painter.line_segment([tr, br], Stroke::new(1.0, bottom_right));
    painter.line_segment([br, bl], Stroke::new(1.0, bottom_right));
}

/// the two-pixel border that makes a surface look raised or sunken.
pub fn bevel(painter: &Painter, rect: Rect, raised: bool) {
    let outer = rect.shrink(0.5);
    let inner = rect.shrink(1.5);
    if raised {
        edges(painter, outer, WHITE, BLACK);
        edges(painter, inner, LIGHT, SHADOW);
    } else {
        edges(painter, outer, SHADOW, WHITE);
        edges(painter, inner, BLACK, LIGHT);
    }
}

/// a push button; `pressed` keeps it looking down, like the active window's taskbar button.
pub fn button(ui: &mut Ui, text: &str, pressed: bool) -> Response {
    sized_button(ui, text, pressed, true, None)
}

/// a push button of a given width, so a stack of them lines up.
pub fn sized_button(
    ui: &mut Ui,
    text: &str,
    pressed: bool,
    enabled: bool,
    width: Option<f32>,
) -> Response {
    let ink = if enabled { BLACK } else { SHADOW };
    let galley = ui.painter().layout_no_wrap(text.to_string(), font(), ink);
    let size = Vec2::new(
        width.unwrap_or_else(|| (galley.size().x + 22.0).max(75.0)),
        23.0,
    );
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    #[cfg(test)]
    probe::record(ui, text, rect, enabled);
    let down = pressed || (enabled && response.is_pointer_button_down_on());
    ui.painter().rect_filled(rect, 0.0, GRAY);
    bevel(ui.painter(), rect, !down);
    let nudge = if down {
        Vec2::new(1.0, 1.0)
    } else {
        Vec2::ZERO
    };
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0 + nudge, galley, ink);
    response
}

/// a button that does nothing and shows gray text when `enabled` is false.
pub fn action(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    if enabled {
        return button(ui, text, false);
    }
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font(), SHADOW);
    let size = Vec2::new((galley.size().x + 22.0).max(75.0), 23.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    #[cfg(test)]
    probe::record(ui, text, rect, false);
    ui.painter().rect_filled(rect, 0.0, GRAY);
    bevel(ui.painter(), rect, true);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, SHADOW);
    response
}

/// the margin inside a sunken box's bevel.
pub const SUNKEN: f32 = 3.0;

/// a white, sunken area for lists and text, like an edit box.
pub fn sunken<R>(ui: &mut Ui, fill: Color32, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let framed = Frame::new()
        .fill(fill)
        .inner_margin(Margin::same(SUNKEN as i8))
        .show(ui, add_contents);
    bevel(ui.painter(), framed.response.rect, false);
    framed.inner
}

/// a single row in a list box; navy when selected.
pub fn list_row(ui: &mut Ui, text: &str, selected: bool) -> Response {
    let height = 16.0;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    if selected {
        ui.painter().rect_filled(rect, 0.0, NAVY);
    }
    ui.painter().text(
        rect.left_center() + Vec2::new(3.0, 0.0),
        Align2::LEFT_CENTER,
        text,
        font(),
        if selected { WHITE } else { BLACK },
    );
    response
}

/// colours, square corners and text sizes for everything egui draws by itself.
pub fn apply(ctx: &egui::Context) {
    // the look is the light one whatever the system prefers: set it on the theme in use
    ctx.set_theme(egui::Theme::Light);
    let mut visuals = egui::Visuals::light();
    let square = CornerRadius::ZERO;
    visuals.window_corner_radius = square;
    visuals.menu_corner_radius = square;
    visuals.panel_fill = GRAY;
    visuals.window_fill = GRAY;
    visuals.override_text_color = Some(BLACK);
    visuals.window_shadow = egui::Shadow::NONE;
    visuals.popup_shadow = egui::Shadow::NONE;
    // a blinking cursor would draw a frame every half second for nothing
    visuals.text_cursor.blink = false;
    visuals.selection.bg_fill = NAVY;
    visuals.selection.stroke = Stroke::new(1.0, WHITE);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = square;
    }
    // the scroll bar thumb is painted in the widget fill, its track in the extreme background
    visuals.extreme_bg_color = LIGHT;
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
    ] {
        widget.bg_fill = GRAY;
    }
    ctx.set_visuals(visuals);
    // a window dragged by an edge is drawn again once egui has sized it, and once more when it
    // took more than the pointer gave it
    ctx.options_mut(|o| o.max_passes = std::num::NonZeroUsize::new(3).unwrap_or(o.max_passes));
    ctx.global_style_mut(|style| {
        for font_id in style.text_styles.values_mut() {
            font_id.size = TEXT;
        }
        style.spacing.item_spacing = Vec2::new(6.0, 4.0);
        // solid, always visible, 16 points wide: no floating bar that fades away
        style.spacing.scroll = egui::style::ScrollStyle {
            bar_width: SCROLL_BAR_WIDTH,
            bar_inner_margin: 0.0,
            bar_outer_margin: 0.0,
            handle_min_length: SCROLL_BAR_WIDTH,
            ..egui::style::ScrollStyle::solid()
        };
        // nt4 had no animation: a scroll bar coming or going takes its whole width at once, not a
        // fraction of it that depends on how long the frames take, so sizes stay whole pixels
        style.animation_time = 0.0;
    });
}

/// lets headless tests find a button by its label.
#[cfg(test)]
pub mod probe {
    use std::collections::HashMap;

    use eframe::egui::{Context, Id, Rect, Ui};

    type Buttons = HashMap<String, (Rect, bool)>;
    type Cells = HashMap<String, (Rect, Rect)>;

    pub fn record(ui: &Ui, text: &str, rect: Rect, enabled: bool) {
        // a page being measured is not on the screen
        if ui
            .ctx()
            .data(|d| d.get_temp::<bool>(Id::unique("tabs-measuring")))
            == Some(true)
        {
            return;
        }
        ui.ctx().data_mut(|d| {
            let buttons: &mut Buttons = d.get_temp_mut_or_default(Id::unique("test-buttons"));
            buttons.insert(text.to_string(), (rect, enabled));
        });
    }

    /// a table cell's text and the column it sits in.
    pub fn record_cell(ui: &Ui, text: &str, text_rect: Rect, column: Rect) {
        if text.is_empty() {
            return;
        }
        ui.ctx().data_mut(|d| {
            let cells: &mut Cells = d.get_temp_mut_or_default(Id::unique("test-cells"));
            cells.insert(text.to_string(), (text_rect, column));
        });
    }

    /// where the text of a table cell was drawn last, and the column it was drawn in.
    pub fn find_cell(ctx: &Context, text: &str) -> Option<(Rect, Rect)> {
        ctx.data(|d| {
            d.get_temp::<Cells>(Id::unique("test-cells"))
                .and_then(|c| c.get(text).copied())
        })
    }

    /// where the button labelled `text` was drawn last, and whether it was enabled.
    pub fn find(ctx: &Context, text: &str) -> Option<(Rect, bool)> {
        ctx.data(|d| {
            d.get_temp::<Buttons>(Id::unique("test-buttons"))
                .and_then(|b| b.get(text).copied())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_animates_so_sizes_do_not_depend_on_timing() {
        let ctx = egui::Context::default();
        apply(&ctx);
        assert_eq!(ctx.global_style().animation_time, 0.0);
    }

    #[test]
    fn scroll_bars_are_solid_sixteen_wide_and_always_there() {
        let ctx = egui::Context::default();
        apply(&ctx);
        let style = ctx.global_style();
        let scroll = style.spacing.scroll;
        assert!(!scroll.floating, "a floating bar fades away");
        assert_eq!(scroll.bar_width, 16.0);
        assert_eq!(scroll.bar_inner_margin + scroll.bar_outer_margin, 0.0);
        let visuals = &style.visuals;
        assert_eq!(visuals.extreme_bg_color, LIGHT, "the track");
        for widget in [
            &visuals.widgets.inactive,
            &visuals.widgets.hovered,
            &visuals.widgets.active,
        ] {
            assert_eq!(widget.bg_fill, GRAY, "the thumb");
        }
    }
}
