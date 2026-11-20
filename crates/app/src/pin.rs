//! the build's version and changelog, which a snapshot test can pin to fixed values so a
//! release bump does not change what it compares. the pin is per thread and ends when the
//! guard drops; nothing outside a test sets one, so users see the real values.

use std::cell::Cell;

thread_local! {
    static PINNED: Cell<Option<(&'static str, &'static str)>> = const { Cell::new(None) };
}

/// the crate version, or the pinned one.
pub fn version() -> &'static str {
    PINNED
        .with(Cell::get)
        .map_or(lanlord_model::version::display(), |(v, _)| v)
}

/// the changelog text, or the pinned one.
pub fn changelog() -> &'static str {
    PINNED
        .with(Cell::get)
        .map_or(crate::changelog::TEXT, |(_, c)| c)
}

/// ends the pin when dropped.
#[must_use = "the pin ends when the guard is dropped"]
pub struct Pin(());

impl Drop for Pin {
    fn drop(&mut self) {
        PINNED.with(|p| p.set(None));
    }
}

/// pins the version and the changelog on this thread until the guard drops.
#[doc(hidden)]
pub fn pin(version: &'static str, changelog: &'static str) -> Pin {
    PINNED.with(|p| p.set(Some((version, changelog))));
    Pin(())
}
