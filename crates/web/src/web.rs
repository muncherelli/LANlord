//! the wasm entry: find the page's canvas and start the egui web runner on it.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// the id of the canvas in index.html.
const CANVAS: &str = "lanlord";

struct App;

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        crate::draw(ui);
    }
}

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or_else(|| JsValue::from_str("no document"))?;
    document.set_title(&crate::title());
    let canvas = document
        .get_element_by_id(CANVAS)
        .ok_or_else(|| JsValue::from_str("the page has no canvas named lanlord"))?
        .dyn_into::<web_sys::HtmlCanvasElement>()?;
    wasm_bindgen_futures::spawn_local(async move {
        let started = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|_cc| Ok(Box::new(App))),
            )
            .await;
        if let Err(error) = started {
            web_sys::console::error_1(&error);
        }
    });
    Ok(())
}
