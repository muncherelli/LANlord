//! the wasm entry: find the page's canvas and start the desktop on it.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// the id of the canvas in index.html.
const CANVAS: &str = "lanlord";
/// the id of the box in index.html that says why the desktop could not start.
const STATUS: &str = "lanlord-status";

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or_else(|| JsValue::from_str("no document"))?;
    let canvas = document
        .get_element_by_id(CANVAS)
        .ok_or_else(|| JsValue::from_str("the page has no canvas named lanlord"))?
        .dyn_into::<web_sys::HtmlCanvasElement>()?;
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(error) = lanlord_ui::start_web(canvas).await {
            web_sys::console::error_1(&error);
            let detail = error.as_string().unwrap_or_else(|| format!("{error:?}"));
            show_status(&format!(
                "LANlord could not start in this browser: {detail}"
            ));
        }
    });
    Ok(())
}

/// put `text` in the page's status box and show it.
fn show_status(text: &str) {
    let Some(status) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(STATUS))
    else {
        return;
    };
    status.set_text_content(Some(text));
    let _ = status.remove_attribute("hidden");
}
