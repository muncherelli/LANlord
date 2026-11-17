//! the console lines the app writes. on the desktop they go through the cli's log, so the
//! terminal and the console window show the same; in the browser the app keeps its own list
//! of listeners.

pub use lanlord_model::log::{Kind, Line};

#[cfg(feature = "native")]
pub use lanlord_core::log::{emit, subscribe};

#[cfg(not(feature = "native"))]
pub use own::{emit, subscribe};

#[cfg(not(feature = "native"))]
mod own {
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::sync::Mutex;

    use super::{Kind, Line};

    static SUBSCRIBERS: Mutex<Vec<Sender<Line>>> = Mutex::new(Vec::new());

    /// a receiver for every line written from now on.
    pub fn subscribe() -> Receiver<Line> {
        let (tx, rx) = mpsc::channel();
        if let Ok(mut subscribers) = SUBSCRIBERS.lock() {
            subscribers.push(tx);
        }
        rx
    }

    /// write a line to everyone listening; listeners that went away are dropped.
    pub fn emit(kind: Kind, text: impl Into<String>) {
        let line = Line {
            kind,
            text: text.into(),
        };
        if let Ok(mut subscribers) = SUBSCRIBERS.lock() {
            subscribers.retain(|tx| tx.send(line.clone()).is_ok());
        }
    }
}
