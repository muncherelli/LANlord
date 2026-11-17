//! the few things the desktop window and the browser do differently; everything else is shared.

/// give the window (or the browser tab) a new title.
pub fn set_title(ctx: &eframe::egui::Context, title: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Title(title.to_string()));
    #[cfg(target_arch = "wasm32")]
    {
        let _ = ctx;
        if let Some(document) = web_sys::window().and_then(|w| w.document()) {
            document.set_title(title);
        }
    }
}
