//! a stand-in page until the shared desktop builds for the browser. it borrows the gui's look
//! (teal desktop, a gray bevelled window with a navy title bar) without depending on lanlord-ui.

use egui::{Align2, Color32, FontId, Pos2, Rect, Stroke, Ui, Vec2};

const TEAL: Color32 = Color32::from_rgb(0, 128, 128);
const GRAY: Color32 = Color32::from_rgb(192, 192, 192);
const LIGHT: Color32 = Color32::from_rgb(223, 223, 223);
const SHADOW: Color32 = Color32::from_rgb(128, 128, 128);
const NAVY: Color32 = Color32::from_rgb(0, 0, 128);

const WINDOW: Vec2 = Vec2::new(360.0, 120.0);
const TITLE_BAR: f32 = 18.0;

/// the page and window title: the web ui only runs on the gateway, so it is always the server.
pub fn title() -> String {
    format!("LANlord Server v{}", env!("CARGO_PKG_VERSION"))
}

/// paint the placeholder into `ui`; returns the window's rect so tests can find it.
pub fn draw(ui: &mut Ui) -> Rect {
    let screen = ui.max_rect();
    let painter = ui.painter();
    painter.rect_filled(screen, 0.0, TEAL);

    let window = Rect::from_center_size(screen.center(), WINDOW);
    painter.rect_filled(window, 0.0, GRAY);
    // raised edges: white and light on the top left, black and shadow on the bottom right
    edges(ui, window.shrink(0.5), Color32::WHITE, Color32::BLACK);
    edges(ui, window.shrink(1.5), LIGHT, SHADOW);

    let bar = Rect::from_min_size(
        window.min + Vec2::splat(3.0),
        Vec2::new(window.width() - 6.0, TITLE_BAR),
    );
    let painter = ui.painter();
    painter.rect_filled(bar, 0.0, NAVY);
    painter.text(
        bar.left_center() + Vec2::new(4.0, 0.0),
        Align2::LEFT_CENTER,
        title(),
        FontId::proportional(13.0),
        Color32::WHITE,
    );
    painter.text(
        Pos2::new(
            window.center().x,
            bar.bottom() + (window.bottom() - bar.bottom()) / 2.0,
        ),
        Align2::CENTER_CENTER,
        "The LANlord web UI is coming soon.",
        FontId::proportional(13.0),
        Color32::BLACK,
    );
    window
}

fn edges(ui: &Ui, rect: Rect, light: Color32, dark: Color32) {
    let painter = ui.painter();
    let (tl, tr, bl, br) = (
        rect.left_top(),
        rect.right_top(),
        rect.left_bottom(),
        rect.right_bottom(),
    );
    painter.line_segment([bl, tl], Stroke::new(1.0, light));
    painter.line_segment([tl, tr], Stroke::new(1.0, light));
    painter.line_segment([tr, br], Stroke::new(1.0, dark));
    painter.line_segment([bl, br], Stroke::new(1.0, dark));
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, RawInput};

    #[test]
    fn the_title_names_the_server_and_its_version() {
        assert_eq!(
            title(),
            format!("LANlord Server v{}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn the_window_sits_in_the_middle_of_the_page() {
        let ctx = Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let mut window = Rect::NOTHING;
        let input = RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        ctx.run_ui(input, |ui| window = draw(ui))
            .textures_delta
            .clear();
        assert_eq!(window.center(), screen.center());
        assert_eq!(window.size(), WINDOW);
    }
}
