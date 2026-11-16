//! the few things the desktop window and the browser do differently; everything else is shared.

/// a new random password, from the operating system's random source on a desktop and from the
/// browser's crypto on the web.
pub fn password() -> String {
    use lanlord_model::passwords::{generate_with, DEFAULT_LEN};
    generate_with(DEFAULT_LEN, fill_random)
}

#[cfg(not(target_arch = "wasm32"))]
fn fill_random(bytes: &mut [u8]) {
    getrandom::fill(bytes).expect("the operating system has no random source");
}

#[cfg(target_arch = "wasm32")]
fn fill_random(bytes: &mut [u8]) {
    let crypto = web_sys::window()
        .and_then(|w| w.crypto().ok())
        .expect("the browser has no crypto");
    crypto
        .get_random_values_with_u8_array(bytes)
        .expect("the browser could not give random bytes");
}

/// the keys the ssh agent offers; the browser has no agent.
pub fn agent_keys() -> Vec<String> {
    #[cfg(feature = "native")]
    return lanlord_core::signin::agent_keys();
    #[cfg(not(feature = "native"))]
    Vec::new()
}

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

#[cfg(test)]
mod tests {
    #[test]
    fn passwords_are_full_length_and_differ() {
        let one = super::password();
        assert_eq!(one.len(), lanlord_model::passwords::DEFAULT_LEN);
        assert_ne!(one, super::password());
    }
}
