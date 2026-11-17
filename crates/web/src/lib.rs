//! the browser build: the egui app on a canvas, served by `lanlord webui` on the gateway. for now
//! it draws a placeholder; the real desktop (lanlord-ui rendering the shared lanlord-app view
//! tree) drops in here once those crates build for wasm.

mod placeholder;

pub use placeholder::{draw, title};

#[cfg(target_arch = "wasm32")]
mod web;
