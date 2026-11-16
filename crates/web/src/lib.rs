//! the browser build: the LANlord desktop from lanlord-ui on a canvas, served by `lanlord webui`
//! on the gateway. the desktop, its windows and its look are the same code as the native window.

#[cfg(target_arch = "wasm32")]
mod web;
