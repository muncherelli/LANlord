//! the window chrome: boxes drawn with plain line characters, window frames with the title on
//! the top line, the taskbar and menus.
//!
//! a window or menu is a single-line box (`┌─┐│└┘`); a window's title and its minimise,
//! maximise and close controls sit on the top line, which is navy while the window has focus.
//! buttons are `[ label ]`, the pressed or focused one in navy. there is no 3d edge.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use super::glyphs::chip;
use super::palette::{Palette, BLACK, GRAY, NAVY, SHADOW, WHITE};
use super::{bold, fill, put, style, text};

/// a gray box with a single-line border one cell wide.
pub struct Frame {
    pub palette: Palette,
}

impl Frame {
    /// the area inside the border.
    pub fn inner(area: Rect) -> Rect {
        Rect::new(
            area.x.saturating_add(1),
            area.y.saturating_add(1),
            area.width.saturating_sub(2),
            area.height.saturating_sub(2),
        )
    }
}

impl Widget for Frame {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = area.intersection(buf.area);
        fill(buf, area, self.palette, GRAY);
        if area.width < 2 || area.height < 2 {
            return;
        }
        let line = style(self.palette, BLACK, GRAY);
        let (left, right, top, bottom) =
            (area.left(), area.right() - 1, area.top(), area.bottom() - 1);
        for x in left + 1..right {
            put(buf, (x, top), "─", line);
            put(buf, (x, bottom), "─", line);
        }
        for y in top + 1..bottom {
            put(buf, (left, y), "│", line);
            put(buf, (right, y), "│", line);
        }
        put(buf, (left, top), "┌", line);
        put(buf, (right, top), "┐", line);
        put(buf, (left, bottom), "└", line);
        put(buf, (right, bottom), "┘", line);
    }
}

/// where a window of `rect`'s size lands when moved to (`x`, `y`): kept inside `bounds`, so
/// its title bar can always be grabbed again.
pub fn place(rect: Rect, x: i32, y: i32, bounds: Rect) -> Rect {
    let width = rect.width.min(bounds.width);
    let height = rect.height.min(bounds.height);
    let max_x = i32::from(bounds.right() - width);
    let max_y = i32::from(bounds.bottom() - height);
    Rect::new(
        x.clamp(i32::from(bounds.x), max_x) as u16,
        y.clamp(i32::from(bounds.y), max_y) as u16,
        width,
        height,
    )
}

/// where the parts of a window frame are, for drawing the body and hit testing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowLayout {
    pub title_bar: Rect,
    pub minimise: Rect,
    pub maximise: Rect,
    pub close: Rect,
    pub body: Rect,
}

/// a window: a raised gray frame, a navy title bar (gray when inactive) with the icon, the
/// title at the left like the gui, and the minimise, maximise and close buttons.
pub struct WindowFrame<'a> {
    pub title: &'a str,
    pub icon: Option<&'a str>,
    pub active: bool,
    pub palette: Palette,
}

impl WindowFrame<'_> {
    /// the title bar is the top line of the box between its corners, the body everything
    /// inside the border.
    pub fn layout(area: Rect) -> WindowLayout {
        let inner = Frame::inner(area);
        let title_bar = Rect::new(inner.x, area.y, inner.width, area.height.min(1));
        let button = |from_right: u16| {
            Rect::new(
                title_bar.right().saturating_sub(from_right),
                title_bar.y,
                2.min(title_bar.width),
                title_bar.height,
            )
        };
        WindowLayout {
            title_bar,
            minimise: button(6),
            maximise: button(4),
            close: button(2),
            body: inner,
        }
    }
}

impl Widget for WindowFrame<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let palette = self.palette;
        Frame { palette }.render(area, buf);
        let layout = WindowFrame::layout(area);
        let (bar, ink) = if self.active {
            (NAVY, WHITE)
        } else {
            (GRAY, BLACK)
        };
        let line = style(palette, ink, bar);
        fill(buf, layout.title_bar, palette, bar);
        for x in layout.title_bar.left()..layout.title_bar.right() {
            put(buf, (x, layout.title_bar.y), "─", line);
        }
        // " icon title " from the left, then " _ □ × " at the right, on the line
        let bar_line = bold(line);
        let mut x = layout.title_bar.x;
        let title = format!(" {} ", self.title);
        if let Some(id) = self.icon {
            put(buf, (x, layout.title_bar.y), " ", line);
            chip(buf, x + 1, layout.title_bar.y, id, palette);
            x += 2;
        }
        let room = layout.minimise.x.saturating_sub(x + 1);
        text(buf, x, layout.title_bar.y, room, &title, bar_line);
        let controls = layout.minimise.x.saturating_sub(1)..layout.close.right();
        if controls.start > x && layout.close.width == 2 {
            for at in controls {
                put(buf, (at, layout.title_bar.y), " ", line);
            }
            for (rect, glyph) in [
                (layout.minimise, "_"),
                (layout.maximise, "□"),
                (layout.close, "×"),
            ] {
                text(buf, rect.x, rect.y, 1, glyph, bar_line);
            }
        }
    }
}

/// one button on the taskbar.
pub struct TaskButton<'a> {
    pub icon: Option<&'a str>,
    pub label: &'a str,
    pub pressed: bool,
}

/// where the parts of the taskbar are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskbarLayout {
    pub menu: Rect,
    pub buttons: Vec<Rect>,
    pub clock: Rect,
    /// the warning item beside the clock, when there is one.
    pub alert: Option<Rect>,
    /// the update item beside the clock, when an update is waiting or going on.
    pub shield: Option<Rect>,
    /// the red debug button, in a development build.
    pub bug: Option<Rect>,
}

/// the taskbar: one gray row with the LANlord menu button, a button per open window and the
/// clock at the right.
pub struct Taskbar<'a> {
    pub menu_label: &'a str,
    pub menu_icon: Option<&'a str>,
    pub menu_open: bool,
    pub buttons: &'a [TaskButton<'a>],
    pub clock: &'a str,
    /// a warning item left of the clock, such as `! Compatibility`.
    pub alert: Option<&'a str>,
    /// the state of the update, left of the warning, such as `Update available`.
    pub shield: Option<&'a str>,
    /// the red debug button of a development build, at the left of the tray.
    pub bug: bool,
    pub palette: Palette,
}

/// what the red debug button says: a bug, and the word for it.
#[cfg(feature = "devtools")]
pub const BUG_LABEL: &str = "Ж Debug";

/// the width of the debug button: its label and an edge and a space each side.
pub const BUG_WIDTH: u16 = 11;

/// the red of the bug.
pub const BUG_RED: (u8, u8, u8) = (200, 0, 0);

/// the widest a window button gets, like the gui's.
const TASK_BUTTON: u16 = 30;

/// the width of a one-row button: its edges, a space each side, the label and the icon.
fn row_button_width(label: &str, icon: bool) -> u16 {
    label.chars().count() as u16 + 4 + u16::from(icon) * 2
}

impl Taskbar<'_> {
    pub fn layout(&self, area: Rect) -> TaskbarLayout {
        let menu_width = row_button_width(self.menu_label, self.menu_icon.is_some());
        let menu = Rect::new(area.x, area.y, menu_width.min(area.width), 1);
        let clock_width = self.clock.chars().count() as u16 + 4;
        let clock = Rect::new(
            area.right().saturating_sub(clock_width),
            area.y,
            clock_width.min(area.width),
            1,
        );
        // the tray grows leftwards from the clock: the warning, then the update, then the bug
        let mut edge = clock.x;
        let mut tray = |width: u16| {
            let rect = Rect::new(
                edge.saturating_sub(width + 1).max(area.x),
                area.y,
                width.min(edge.saturating_sub(area.x)),
                1,
            );
            edge = rect.x;
            rect
        };
        let alert = self
            .alert
            .map(|label| tray(label.chars().count() as u16 + 2));
        let shield = self
            .shield
            .map(|label| tray(label.chars().count() as u16 + 2));
        let bug = self.bug.then(|| tray(BUG_WIDTH));
        let limit = edge;
        let mut x = menu.right() + 1;
        let buttons = self
            .buttons
            .iter()
            .map(|b| {
                let width = row_button_width(b.label, b.icon.is_some()).min(TASK_BUTTON);
                let width = width.min(limit.saturating_sub(x + 1));
                let rect = Rect::new(x, area.y, width, 1);
                x = x.saturating_add(width + 1);
                rect
            })
            .collect();
        TaskbarLayout {
            menu,
            buttons,
            clock,
            alert,
            shield,
            bug,
        }
    }
}

/// a one-row button: `[ label ]`, in navy when it is pressed or has the focus, with an
/// optional icon chip before the label.
pub fn row_button(
    buf: &mut Buffer,
    rect: Rect,
    icon: Option<&str>,
    label: &str,
    pressed: bool,
    palette: Palette,
) {
    if rect.width == 0 {
        return;
    }
    let (fg, bg) = if pressed {
        (WHITE, NAVY)
    } else {
        (BLACK, GRAY)
    };
    fill(buf, rect, palette, bg);
    if rect.width < 2 {
        return;
    }
    let edge = style(palette, fg, bg);
    put(buf, (rect.x, rect.y), "[", edge);
    put(buf, (rect.right() - 1, rect.y), "]", edge);
    let mut x = rect.x + 2;
    if let Some(id) = icon {
        chip(buf, x, rect.y, id, palette);
        x += 2;
    }
    text(
        buf,
        x,
        rect.y,
        rect.right().saturating_sub(x + 2),
        label,
        bold(edge),
    );
}

/// the red debug button `[Ж Debug]`, on the taskbar and in the corner of the logon screen.
#[cfg(feature = "devtools")]
pub fn bug_button(buf: &mut Buffer, rect: Rect, palette: Palette) {
    if rect.width == 0 {
        return;
    }
    fill(buf, rect, palette, GRAY);
    if rect.width < 2 {
        return;
    }
    put(buf, (rect.x, rect.y), "[", style(palette, BLACK, GRAY));
    put(
        buf,
        (rect.right() - 1, rect.y),
        "]",
        style(palette, BLACK, GRAY),
    );
    text(
        buf,
        rect.x + 2,
        rect.y,
        rect.width.saturating_sub(4),
        BUG_LABEL,
        bold(style(palette, BUG_RED, GRAY)),
    );
}

impl Widget for Taskbar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let palette = self.palette;
        fill(buf, area, palette, GRAY);
        let layout = self.layout(area);
        row_button(
            buf,
            layout.menu,
            self.menu_icon,
            self.menu_label,
            self.menu_open,
            palette,
        );
        for (button, rect) in self.buttons.iter().zip(&layout.buttons) {
            row_button(
                buf,
                *rect,
                button.icon,
                button.label,
                button.pressed,
                palette,
            );
        }
        if let (Some(label), Some(rect)) = (self.alert, layout.alert) {
            // black on yellow, so it stands out on the gray bar
            fill(buf, rect, palette, (255, 255, 0));
            text(
                buf,
                rect.x + 1,
                rect.y,
                rect.width.saturating_sub(1),
                label,
                bold(style(palette, BLACK, (255, 255, 0))),
            );
        }
        if let (Some(label), Some(rect)) = (self.shield, layout.shield) {
            // white on navy, so it reads apart from the yellow warning
            fill(buf, rect, palette, NAVY);
            text(
                buf,
                rect.x + 1,
                rect.y,
                rect.width.saturating_sub(1),
                label,
                bold(style(palette, WHITE, NAVY)),
            );
        }
        #[cfg(feature = "devtools")]
        if let Some(rect) = layout.bug {
            bug_button(buf, rect, palette);
        }
        let clock = layout.clock;
        if clock.width > 2 {
            text(
                buf,
                clock.x + 2,
                clock.y,
                clock.width.saturating_sub(3),
                self.clock,
                style(palette, BLACK, GRAY),
            );
        }
    }
}

/// one row of a menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuRow<'a> {
    Item {
        label: &'a str,
        ticked: bool,
        arrow: bool,
        enabled: bool,
        header: bool,
    },
    Separator,
}

impl<'a> MenuRow<'a> {
    /// a plain enabled item.
    pub fn item(label: &'a str) -> MenuRow<'a> {
        MenuRow::Item {
            label,
            ticked: false,
            arrow: false,
            enabled: true,
            header: false,
        }
    }
}

/// a raised popup menu: tick column, label, submenu arrow; the hovered row in navy.
pub struct Menu<'a> {
    pub rows: &'a [MenuRow<'a>],
    pub selected: Option<usize>,
    pub palette: Palette,
}

impl Menu<'_> {
    /// the size the menu wants: (columns, rows), edge included.
    pub fn size(&self) -> (u16, u16) {
        let widest = self
            .rows
            .iter()
            .map(|row| match row {
                MenuRow::Item { label, .. } => label.chars().count(),
                MenuRow::Separator => 0,
            })
            .max()
            .unwrap_or(0) as u16;
        (widest + 7, self.rows.len() as u16 + 2)
    }

    /// the screen row of each menu row inside `area`.
    pub fn row_rects(&self, area: Rect) -> Vec<Rect> {
        let inner = Frame::inner(area);
        (0..self.rows.len() as u16)
            .map(|i| Rect::new(inner.x, inner.y + i, inner.width, 1))
            .filter(|r| r.y < inner.bottom())
            .collect()
    }
}

impl Widget for Menu<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let palette = self.palette;
        Frame { palette }.render(area, buf);
        for (i, (row, rect)) in self.rows.iter().zip(self.row_rects(area)).enumerate() {
            match row {
                MenuRow::Separator => {
                    let line = style(palette, SHADOW, GRAY);
                    for x in rect.left() + 1..rect.right().saturating_sub(1) {
                        put(buf, (x, rect.y), "─", line);
                    }
                }
                MenuRow::Item {
                    label,
                    ticked,
                    arrow,
                    enabled,
                    header,
                } => {
                    let selected = self.selected == Some(i) && *enabled && !*header;
                    let (fg, bg) = match (selected, *enabled) {
                        (true, _) => (WHITE, NAVY),
                        (false, true) => (BLACK, GRAY),
                        (false, false) => (SHADOW, GRAY),
                    };
                    fill(buf, rect, palette, bg);
                    let face = style(palette, fg, bg);
                    let face = if *header { bold(face) } else { face };
                    if *ticked {
                        text(buf, rect.x + 1, rect.y, 1, "✓", face);
                    }
                    text(
                        buf,
                        rect.x + 3,
                        rect.y,
                        rect.width.saturating_sub(5),
                        label,
                        face,
                    );
                    if *arrow {
                        text(buf, rect.right().saturating_sub(2), rect.y, 1, "▸", face);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint::lines;
    use crate::paint::palette::Depth;
    use ratatui::style::{Color, Modifier};

    fn palette() -> Palette {
        Palette::new(Depth::TrueColor)
    }

    #[test]
    fn a_dragged_window_stays_on_the_desktop() {
        let desk = Rect::new(0, 0, 80, 23);
        let window = Rect::new(10, 5, 44, 18);
        assert_eq!(place(window, 70, 20, desk), Rect::new(36, 5, 44, 18));
        assert_eq!(place(window, -9, -3, desk), Rect::new(0, 0, 44, 18));
        assert_eq!(place(window, 20, 2, desk), Rect::new(20, 2, 44, 18));
        // a window bigger than the desktop is cut to fit
        assert_eq!(
            place(window, 5, 5, Rect::new(0, 0, 30, 10)),
            Rect::new(0, 0, 30, 10)
        );
    }

    #[test]
    fn every_widget_clips_to_the_buffer() {
        // areas that run past the right and bottom edge of a small buffer must not panic
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 4));
        let off = Rect::new(6, 2, 30, 12);
        WindowFrame {
            title: "Console",
            icon: Some("console"),
            active: true,
            palette: palette(),
        }
        .render(off, &mut buf);
        let rows = [MenuRow::item("Quit"), MenuRow::Separator];
        Menu {
            rows: &rows,
            selected: None,
            palette: palette(),
        }
        .render(off, &mut buf);
        Taskbar {
            menu_label: "LANlord",
            menu_icon: Some("lanlord"),
            menu_open: true,
            buttons: &[TaskButton {
                icon: None,
                label: "Console",
                pressed: true,
            }],
            clock: "10:55 AM",
            alert: None,
            shield: None,
            bug: false,
            palette: palette(),
        }
        .render(Rect::new(4, 3, 60, 1), &mut buf);
        row_button(
            &mut buf,
            Rect::new(8, 1, 12, 1),
            None,
            "OK",
            false,
            palette(),
        );
        assert_eq!(buf[(6, 2)].symbol(), "┌", "the visible corner is drawn");
    }

    #[test]
    fn a_frame_is_a_single_line_box() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 5, 3));
        Frame { palette: palette() }.render(buf.area, &mut buf);
        assert_eq!(lines(&buf), ["┌───┐", "│   │", "└───┘"]);
        assert_eq!(buf[(1, 0)].fg, Color::Rgb(0, 0, 0));
        assert_eq!(buf[(2, 1)].bg, Color::Rgb(192, 192, 192));
        assert_eq!(Frame::inner(buf.area), Rect::new(1, 1, 3, 1));
    }

    #[test]
    fn a_window_has_its_title_and_buttons_on_a_navy_top_line() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 24, 4));
        WindowFrame {
            title: "My Server",
            icon: Some("my-server"),
            active: true,
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(
            lines(&buf),
            [
                "┌ ≡ My Server ── _ □ × ┐",
                "│                      │",
                "│                      │",
                "└──────────────────────┘",
            ]
        );
        assert_eq!(buf[(5, 0)].bg, Color::Rgb(0, 0, 128));
        assert_eq!(buf[(5, 0)].fg, Color::Rgb(255, 255, 255));
        assert_eq!(
            buf[(21, 0)].bg,
            Color::Rgb(0, 0, 128),
            "close sits on the bar"
        );
        let layout = WindowFrame::layout(buf.area);
        assert_eq!(layout.close, Rect::new(21, 0, 2, 1));
        assert_eq!(layout.body, Rect::new(1, 1, 22, 2));
    }

    #[test]
    fn an_inactive_window_has_a_gray_top_line() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 4));
        WindowFrame {
            title: "Console",
            icon: None,
            active: false,
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(buf[(3, 0)].bg, Color::Rgb(192, 192, 192));
        assert_eq!(buf[(3, 0)].fg, Color::Rgb(0, 0, 0));
    }

    #[test]
    fn the_taskbar_has_bracketed_buttons_and_a_plain_clock() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 44, 1));
        let buttons = [TaskButton {
            icon: None,
            label: "Console",
            pressed: true,
        }];
        let bar = Taskbar {
            menu_label: "LANlord",
            menu_icon: Some("lanlord"),
            menu_open: false,
            buttons: &buttons,
            clock: "10:55 AM",
            alert: None,
            shield: None,
            bug: false,
            palette: palette(),
        };
        let layout = bar.layout(buf.area);
        bar.render(buf.area, &mut buf);
        assert_eq!(
            lines(&buf),
            ["[ L LANlord ] [ Console ]         10:55 AM  "]
        );
        assert_eq!(layout.menu, Rect::new(0, 0, 13, 1));
        assert_eq!(layout.buttons, [Rect::new(14, 0, 11, 1)]);
        assert_eq!(layout.clock, Rect::new(32, 0, 12, 1));
        // the open window's button is navy, the others gray
        assert_eq!(buf[(0, 0)].bg, Color::Rgb(192, 192, 192));
        assert_eq!(buf[(14, 0)].bg, Color::Rgb(0, 0, 128));
        assert_eq!(buf[(14, 0)].fg, Color::Rgb(255, 255, 255));
        assert!(buf[(4, 0)].modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn a_menu_marks_ticks_arrows_separators_and_the_selection() {
        let rows = [
            MenuRow::Item {
                label: "Server: demo",
                ticked: false,
                arrow: false,
                enabled: true,
                header: true,
            },
            MenuRow::Separator,
            MenuRow::Item {
                label: "Automatic updates",
                ticked: true,
                arrow: false,
                enabled: true,
                header: false,
            },
            MenuRow::Item {
                label: "Developer",
                ticked: false,
                arrow: true,
                enabled: true,
                header: false,
            },
        ];
        let menu = Menu {
            rows: &rows,
            selected: Some(3),
            palette: palette(),
        };
        let (w, h) = menu.size();
        assert_eq!((w, h), (24, 6));
        let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
        menu.render(buf.area, &mut buf);
        assert_eq!(
            lines(&buf),
            [
                "┌──────────────────────┐",
                "│   Server: demo       │",
                "│ ──────────────────── │",
                "│ ✓ Automatic updates  │",
                "│   Developer        ▸ │",
                "└──────────────────────┘",
            ]
        );
        assert_eq!(buf[(5, 4)].bg, Color::Rgb(0, 0, 128));
        assert_eq!(buf[(5, 3)].bg, Color::Rgb(192, 192, 192));
    }
}
