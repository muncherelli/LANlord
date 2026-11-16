//! the window chrome: bevelled boxes, window frames with a title bar, the taskbar and menus.
//!
//! a 1px gui edge is imitated with the one-eighth block glyphs (`▔ ▁ ▏ ▕`), which terminals
//! draw hugging the side of the cell: a raised box gets a light line along its outer top and
//! left and a dark one along its outer bottom and right; a sunken box puts its lines on the
//! inner side, against the white field it surrounds, dark above and left, light below and right.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use super::art::badge;
use super::palette::{Palette, Rgb, BLACK, GRAY, NAVY, SHADOW, WHITE};
use super::{bold, fill, style, text};

/// a gray box with a 3d edge one cell wide.
pub struct Bevel {
    pub sunken: bool,
    pub palette: Palette,
}

impl Bevel {
    /// the area inside the edge.
    pub fn inner(area: Rect) -> Rect {
        Rect::new(
            area.x.saturating_add(1),
            area.y.saturating_add(1),
            area.width.saturating_sub(2),
            area.height.saturating_sub(2),
        )
    }
}

impl Widget for Bevel {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = area.intersection(buf.area);
        fill(buf, area, self.palette, GRAY);
        if area.width < 2 || area.height < 2 {
            return;
        }
        let palette = self.palette;
        let (left, right, top, bottom) =
            (area.left(), area.right() - 1, area.top(), area.bottom() - 1);
        // (top, bottom, left, right) glyphs and their colours
        let (glyphs, colours): ([&str; 4], [Rgb; 4]) = if self.sunken {
            (["▁", "▔", "▕", "▏"], [SHADOW, WHITE, SHADOW, WHITE])
        } else {
            (["▔", "▁", "▏", "▕"], [WHITE, SHADOW, WHITE, SHADOW])
        };
        let edge = |rgb: Rgb| style(palette, rgb, GRAY);
        for x in left + 1..right {
            buf[(x, top)]
                .set_symbol(glyphs[0])
                .set_style(edge(colours[0]));
            buf[(x, bottom)]
                .set_symbol(glyphs[1])
                .set_style(edge(colours[1]));
        }
        for y in top + 1..bottom {
            buf[(left, y)]
                .set_symbol(glyphs[2])
                .set_style(edge(colours[2]));
            buf[(right, y)]
                .set_symbol(glyphs[3])
                .set_style(edge(colours[3]));
        }
        if !self.sunken {
            // the corners carry the line of the edge that owns them in the gui
            buf[(left, top)].set_symbol("▔").set_style(edge(WHITE));
            buf[(right, top)].set_symbol("▕").set_style(edge(SHADOW));
            buf[(left, bottom)].set_symbol("▏").set_style(edge(WHITE));
            buf[(right, bottom)].set_symbol("▁").set_style(edge(SHADOW));
        }
    }
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
    pub icon: Option<&'a [&'a str]>,
    pub active: bool,
    pub palette: Palette,
}

impl WindowFrame<'_> {
    /// the title bar is the first row inside the edge, the body everything under it.
    pub fn layout(area: Rect) -> WindowLayout {
        let inner = Bevel::inner(area);
        let title_bar = Rect::new(inner.x, inner.y, inner.width, inner.height.min(1));
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
            minimise: button(7),
            maximise: button(5),
            close: button(2),
            body: Rect::new(
                inner.x,
                inner.y.saturating_add(1),
                inner.width,
                inner.height.saturating_sub(1),
            ),
        }
    }
}

impl Widget for WindowFrame<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let palette = self.palette;
        Bevel {
            sunken: false,
            palette,
        }
        .render(area, buf);
        let layout = WindowFrame::layout(area);
        let (bar, ink) = if self.active {
            (NAVY, WHITE)
        } else {
            (SHADOW, GRAY)
        };
        fill(buf, layout.title_bar, palette, bar);
        let mut x = layout.title_bar.x + 1;
        if let Some(art) = self.icon {
            badge(buf, x, layout.title_bar.y, art, bar, palette);
            x += 2;
        }
        let room = layout.minimise.x.saturating_sub(x + 1);
        text(
            buf,
            x,
            layout.title_bar.y,
            room,
            self.title,
            bold(style(palette, ink, bar)),
        );
        let face = bold(style(palette, BLACK, GRAY));
        for (rect, glyph) in [
            (layout.minimise, "_"),
            (layout.maximise, "□"),
            (layout.close, "×"),
        ] {
            if rect.x > x && rect.width == 2 {
                text(buf, rect.x, rect.y, 1, glyph, face);
                // a dark right edge makes each title button read as raised
                buf[(rect.x + 1, rect.y)]
                    .set_symbol("▕")
                    .set_style(style(palette, BLACK, GRAY));
            }
        }
    }
}

/// one button on the taskbar.
pub struct TaskButton<'a> {
    pub icon: Option<&'a [&'a str]>,
    pub label: &'a str,
    pub pressed: bool,
}

/// where the parts of the taskbar are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskbarLayout {
    pub menu: Rect,
    pub buttons: Vec<Rect>,
    pub clock: Rect,
}

/// the taskbar: one gray row with the raised LANlord menu button, a button per open window
/// and the sunken clock at the right.
pub struct Taskbar<'a> {
    pub menu_label: &'a str,
    pub menu_icon: Option<&'a [&'a str]>,
    pub menu_open: bool,
    pub buttons: &'a [TaskButton<'a>],
    pub clock: &'a str,
    pub palette: Palette,
}

/// the widest a window button gets, like the gui's.
const TASK_BUTTON: u16 = 22;

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
        let mut x = menu.right() + 1;
        let buttons = self
            .buttons
            .iter()
            .map(|b| {
                let width = row_button_width(b.label, b.icon.is_some()).min(TASK_BUTTON);
                let width = width.min(clock.x.saturating_sub(x + 1));
                let rect = Rect::new(x, area.y, width, 1);
                x = x.saturating_add(width + 1);
                rect
            })
            .collect();
        TaskbarLayout {
            menu,
            buttons,
            clock,
        }
    }
}

/// the edges of a one-row box: a thin line on its first and last cell, light then dark when
/// raised, dark then light when sunken.
fn row_edges(buf: &mut Buffer, rect: Rect, sunken: bool, palette: Palette) {
    if rect.width < 2 {
        return;
    }
    let (first, last) = if sunken {
        (SHADOW, WHITE)
    } else {
        (WHITE, BLACK)
    };
    buf[(rect.x, rect.y)]
        .set_symbol("▏")
        .set_style(style(palette, first, GRAY));
    buf[(rect.right() - 1, rect.y)]
        .set_symbol("▕")
        .set_style(style(palette, last, GRAY));
}

/// a one-row raised button (sunk when pressed) with an optional icon badge.
pub fn row_button(
    buf: &mut Buffer,
    rect: Rect,
    icon: Option<&[&str]>,
    label: &str,
    pressed: bool,
    palette: Palette,
) {
    if rect.width == 0 {
        return;
    }
    fill(buf, rect, palette, GRAY);
    row_edges(buf, rect, pressed, palette);
    let mut x = rect.x + 2;
    if let Some(art) = icon {
        badge(buf, x, rect.y, art, GRAY, palette);
        x += 2;
    }
    text(
        buf,
        x,
        rect.y,
        rect.right().saturating_sub(x + 2),
        label,
        bold(style(palette, BLACK, GRAY)),
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
        let clock = layout.clock;
        if clock.width > 2 {
            row_edges(buf, clock, true, palette);
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
        let inner = Bevel::inner(area);
        (0..self.rows.len() as u16)
            .map(|i| Rect::new(inner.x, inner.y + i, inner.width, 1))
            .filter(|r| r.y < inner.bottom())
            .collect()
    }
}

impl Widget for Menu<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let palette = self.palette;
        Bevel {
            sunken: false,
            palette,
        }
        .render(area, buf);
        for (i, (row, rect)) in self.rows.iter().zip(self.row_rects(area)).enumerate() {
            match row {
                MenuRow::Separator => {
                    let line = style(palette, SHADOW, GRAY);
                    for x in rect.left() + 1..rect.right().saturating_sub(1) {
                        buf[(x, rect.y)].set_symbol("─").set_style(line);
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
    use crate::paint::art::small;
    use crate::paint::lines;
    use crate::paint::palette::Depth;
    use ratatui::style::{Color, Modifier};

    fn palette() -> Palette {
        Palette::new(Depth::TrueColor)
    }

    #[test]
    fn a_raised_bevel_is_light_top_left_and_dark_bottom_right() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 5, 3));
        Bevel {
            sunken: false,
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(lines(&buf), ["▔▔▔▔▕", "▏   ▕", "▏▁▁▁▁"]);
        assert_eq!(buf[(1, 0)].fg, Color::Rgb(255, 255, 255));
        assert_eq!(buf[(0, 1)].fg, Color::Rgb(255, 255, 255));
        assert_eq!(buf[(1, 2)].fg, Color::Rgb(128, 128, 128));
        assert_eq!(buf[(4, 1)].fg, Color::Rgb(128, 128, 128));
        assert_eq!(buf[(2, 1)].bg, Color::Rgb(192, 192, 192));
    }

    #[test]
    fn a_sunken_bevel_hugs_its_inside_dark_then_light() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 3));
        Bevel {
            sunken: true,
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(lines(&buf), [" ▁▁ ", "▕  ▏", " ▔▔ "]);
        assert_eq!(buf[(1, 0)].fg, Color::Rgb(128, 128, 128));
        assert_eq!(buf[(1, 2)].fg, Color::Rgb(255, 255, 255));
    }

    #[test]
    fn a_window_has_a_navy_title_bar_with_its_buttons() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 24, 4));
        WindowFrame {
            title: "My Server",
            icon: Some(small("my-server")),
            active: true,
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(
            lines(&buf),
            [
                "▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▕",
                "▏ ■ My Server   _▕□▕ ×▕▕",
                "▏                      ▕",
                "▏▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁",
            ]
        );
        assert_eq!(buf[(5, 1)].bg, Color::Rgb(0, 0, 128));
        assert_eq!(buf[(5, 1)].fg, Color::Rgb(255, 255, 255));
        assert_eq!(buf[(21, 1)].bg, Color::Rgb(192, 192, 192), "close face");
        let layout = WindowFrame::layout(buf.area);
        assert_eq!(layout.close, Rect::new(21, 1, 2, 1));
        assert_eq!(layout.body, Rect::new(1, 2, 22, 1));
    }

    #[test]
    fn an_inactive_window_has_a_gray_title_bar_and_gray_text() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 4));
        WindowFrame {
            title: "Console",
            icon: None,
            active: false,
            palette: palette(),
        }
        .render(buf.area, &mut buf);
        assert_eq!(buf[(3, 1)].bg, Color::Rgb(128, 128, 128));
        assert_eq!(buf[(3, 1)].fg, Color::Rgb(192, 192, 192));
    }

    #[test]
    fn the_taskbar_has_raised_buttons_and_a_sunken_clock() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 44, 1));
        let buttons = [TaskButton {
            icon: None,
            label: "Console",
            pressed: true,
        }];
        let bar = Taskbar {
            menu_label: "LANlord",
            menu_icon: Some(small("lanlord")),
            menu_open: false,
            buttons: &buttons,
            clock: "10:55 AM",
            palette: palette(),
        };
        let layout = bar.layout(buf.area);
        bar.render(buf.area, &mut buf);
        assert_eq!(
            lines(&buf),
            ["▏ ■ LANlord ▕ ▏ Console ▕       ▏ 10:55 AM ▕"]
        );
        assert_eq!(layout.menu, Rect::new(0, 0, 13, 1));
        assert_eq!(layout.buttons, [Rect::new(14, 0, 11, 1)]);
        assert_eq!(layout.clock, Rect::new(32, 0, 12, 1));
        // raised: light left edge; pressed and the clock: dark left edge
        assert_eq!(buf[(0, 0)].fg, Color::Rgb(255, 255, 255));
        assert_eq!(buf[(14, 0)].fg, Color::Rgb(128, 128, 128));
        assert_eq!(buf[(32, 0)].fg, Color::Rgb(128, 128, 128));
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
                "▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▕",
                "▏   Server: demo       ▕",
                "▏ ──────────────────── ▕",
                "▏ ✓ Automatic updates  ▕",
                "▏   Developer        ▸ ▕",
                "▏▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁",
            ]
        );
        assert_eq!(buf[(5, 4)].bg, Color::Rgb(0, 0, 128));
        assert_eq!(buf[(5, 3)].bg, Color::Rgb(192, 192, 192));
    }
}
