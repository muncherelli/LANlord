//! the classic desktop look: gray surfaces, two-pixel bevels, navy title bars.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Frame, Margin, Painter, Rect, Response, Sense,
    Stroke, Ui, Vec2,
};

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
    let galley = ui.painter().layout_no_wrap(text.to_string(), font(), BLACK);
    let size = Vec2::new((galley.size().x + 22.0).max(75.0), 23.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    #[cfg(test)]
    probe::record(ui, text, rect, true);
    let down = pressed || response.is_pointer_button_down_on();
    ui.painter().rect_filled(rect, 0.0, GRAY);
    bevel(ui.painter(), rect, !down);
    let nudge = if down {
        Vec2::new(1.0, 1.0)
    } else {
        Vec2::ZERO
    };
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0 + nudge, galley, BLACK);
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

/// a white, sunken area for lists and text, like an edit box.
pub fn sunken<R>(ui: &mut Ui, fill: Color32, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let framed = Frame::new()
        .fill(fill)
        .inner_margin(Margin::same(3))
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
    let mut visuals = egui::Visuals::light();
    let square = CornerRadius::ZERO;
    visuals.window_corner_radius = square;
    visuals.menu_corner_radius = square;
    visuals.panel_fill = GRAY;
    visuals.window_fill = GRAY;
    visuals.override_text_color = Some(BLACK);
    visuals.window_shadow = egui::Shadow::NONE;
    visuals.popup_shadow = egui::Shadow::NONE;
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
    ctx.set_visuals(visuals);
    ctx.global_style_mut(|style| {
        for font_id in style.text_styles.values_mut() {
            font_id.size = TEXT;
        }
        style.spacing.item_spacing = Vec2::new(6.0, 4.0);
    });
}

/// lets headless tests find a button by its label.
#[cfg(test)]
pub mod probe {
    use std::collections::HashMap;

    use eframe::egui::{Context, Id, Rect, Ui};

    type Buttons = HashMap<String, (Rect, bool)>;

    pub fn record(ui: &Ui, text: &str, rect: Rect, enabled: bool) {
        ui.ctx().data_mut(|d| {
            let buttons: &mut Buttons = d.get_temp_mut_or_default(Id::unique("test-buttons"));
            buttons.insert(text.to_string(), (rect, enabled));
        });
    }

    /// where the button labelled `text` was drawn last, and whether it was enabled.
    pub fn find(ctx: &Context, text: &str) -> Option<(Rect, bool)> {
        ctx.data(|d| {
            d.get_temp::<Buttons>(Id::unique("test-buttons"))
                .and_then(|b| b.get(text).copied())
        })
    }
}
