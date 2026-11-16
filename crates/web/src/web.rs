//! the wasm entry: find the page's canvas and start the desktop on it.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// the id of the canvas in index.html.
const CANVAS: &str = "lanlord";

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
        }
    });
    Ok(())
}
