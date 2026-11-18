//! the one layout system of the terminal ui: how much room a frame leaves around its content,
//! how big a window opens and where it goes. every window, dialog and box takes its numbers
//! from here, so the screens agree.
//!
//! the rules:
//! - a terminal has no room to spare: the content of a window or dialog starts right inside
//!   the border, with no margin columns or rows. one blank row separates groups where it helps.
//! - a window is as big as its content, cut down to the desktop; a body that still does not
//!   fit scrolls with the wheel. lists, logs and consoles take whatever room they are given.
//! - labels share one column, buttons in a row share one width and sit at the right.

use ratatui::layout::Rect;

use crate::paint::chrome::WindowFrame;

/// columns of padding inside the border, on each side.
pub const PAD_X: u16 = 0;
/// rows of padding above and below the content of a window or dialog.
pub const PAD_Y: u16 = 0;
/// columns a light box (a list, a log, a field) keeps between its edge and its text, so a
/// selected row does not touch the letters.
pub const BOX_INSET: u16 = 1;
/// the width of the label column in a form row, like the gui's.
pub const LABEL: u16 = 18;
/// the least width of a push button, so `OK` is not a sliver.
pub const BUTTON_MIN: u16 = 8;
/// the widest a dialog gets; the first-administrator box and its fields get more.
pub const DIALOG_WIDTH: u16 = 60;
pub const DIALOG_WIDE: u16 = 72;
/// the narrowest a dialog gets, so its title and buttons always fit.
pub const DIALOG_MIN: u16 = 40;
/// columns the frame takes beside a title bar: the icon, the three title buttons and room.
pub const TITLE_ROOM: u16 = 16;
/// the cells a window's frame takes: the border on both sides, and the top line that carries
/// the title, and the bottom line.
const FRAME_COLS: u16 = 2;
const FRAME_ROWS: u16 = 2;

/// `area` less the padding on every side.
pub fn inset(area: Rect) -> Rect {
    Rect::new(
        area.x.saturating_add(PAD_X),
        area.y.saturating_add(PAD_Y),
        area.width.saturating_sub(2 * PAD_X),
        area.height.saturating_sub(2 * PAD_Y),
    )
}

/// where the content of a window or dialog framed in `frame` goes.
pub fn content(frame: Rect) -> Rect {
    inset(WindowFrame::layout(frame).body)
}

/// the width a content area of `width` columns needs the window to have.
pub fn frame_width(width: u16) -> u16 {
    width + 2 * PAD_X + FRAME_COLS
}

/// the height a content area of `height` rows needs the window to have.
pub fn frame_height(height: u16) -> u16 {
    height + 2 * PAD_Y + FRAME_ROWS
}

/// how many columns the content area of a window `frame_width` wide has.
pub fn content_width(frame_width: u16) -> u16 {
    frame_width.saturating_sub(2 * PAD_X + FRAME_COLS)
}

/// where a window of `size` goes on `desktop`: its top left corner at (`x`, `y`) if it fits
/// there, otherwise moved left and up until it does, and cut down only when the desktop itself
/// is smaller.
pub fn place(size: (u16, u16), x: u16, y: u16, desktop: Rect) -> Rect {
    let width = size.0.min(desktop.width);
    let height = size.1.min(desktop.height);
    let x = x.min(desktop.right().saturating_sub(width)).max(desktop.x);
    let y = y
        .min(desktop.bottom().saturating_sub(height))
        .max(desktop.y);
    Rect::new(x, y, width, height)
}
