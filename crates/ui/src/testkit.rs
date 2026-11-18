//! helpers for headless frame tests.

use eframe::egui::{self, Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Ui, Vec2};

pub const SCREEN: Vec2 = Vec2::new(960.0, 640.0);

/// run one frame with `events`, dropping the texture updates nothing uploads.
pub fn run(ctx: &egui::Context, events: Vec<Event>, draw: impl FnMut(&mut Ui)) {
    run_on(ctx, SCREEN, events, draw);
}

/// like `run`, on a screen of `size`.
pub fn run_on(ctx: &egui::Context, size: Vec2, events: Vec<Event>, draw: impl FnMut(&mut Ui)) {
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
        events,
        ..Default::default()
    };
    ctx.run_ui(input, draw).textures_delta.clear();
}

/// move to `at`, press and release the primary button.
pub fn click(at: Pos2) -> Vec<Event> {
    let button = |pressed| Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    vec![Event::PointerMoved(at), button(true), button(false)]
}

pub fn key(key: Key) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}
