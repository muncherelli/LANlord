//! the developer tools are not in this build: nothing is drawn and no click or key is theirs.

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::{Position, Rect};

use super::Shell;

pub(super) struct State;

impl State {
    pub(super) fn new() -> Self {
        Self
    }
}

impl Shell {
    pub(super) fn devtools_begin(&mut self) {}

    pub(super) fn bug_shown(&self) -> bool {
        false
    }

    #[cfg(test)]
    pub(super) fn hide_bug(&mut self) {}

    pub(super) fn render_corner_bug(&mut self, _area: Rect, _buf: &mut Buffer) {}

    pub(super) fn render_debug_menu(&mut self, _buf: &mut Buffer) {}

    pub(super) fn outline_windows(&mut self, _buf: &mut Buffer) {}

    pub(super) fn click_devtools(&mut self, _at: Position) -> bool {
        false
    }

    pub(super) fn key_devtools(&mut self, _key: KeyEvent) -> bool {
        false
    }

    /// there is no demo desktop to leave.
    pub(super) fn leave_demo(&mut self) -> bool {
        false
    }
}
