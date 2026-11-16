//! how background work tells the front end to draw again. egui asks for a repaint, the tui
//! wakes its event loop; the app layer only knows there is something to call.

use std::sync::Arc;

/// a handle that asks the front end for a new frame. cheap to clone and safe to call from any
/// thread.
#[derive(Clone)]
pub struct Wake(Arc<dyn Fn() + Send + Sync>);

impl Wake {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self(Arc::new(wake))
    }

    /// a handle that does nothing, for tests and headless use.
    pub fn none() -> Self {
        Self::new(|| {})
    }

    pub fn wake(&self) {
        (self.0)();
    }
}

impl Default for Wake {
    fn default() -> Self {
        Self::none()
    }
}
