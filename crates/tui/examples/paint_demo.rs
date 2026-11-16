//! a static desktop drawn with the terminal toolkit, to try the look and the mouse:
//! click an icon to select it, double-click to open its window, drag a title bar to move it,
//! click × to close, click LANlord for the menu. q or esc quits.
//!
//! run: cargo run -p lanlord-tui --example paint_demo

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lanlord_tui::paint::chrome::{place, Menu, MenuRow, TaskButton, Taskbar, WindowFrame};
use lanlord_tui::paint::controls::{Button, Checkbox, List, TextField};
use lanlord_tui::paint::desktop::{icon_rect, Desktop, GlyphTile};
use lanlord_tui::paint::mouse::{Capture, Gesture, HitMap, Mouse};
use lanlord_tui::paint::palette::Palette;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::Widget;
use ratatui::Frame;

const ICONS: [(&str, &str); 11] = [
    ("my-server", "My Server"),
    ("networks", "Networks"),
    ("gateway", "Gateway"),
    ("docker", "Docker"),
    ("nt4-domain", "NT4 Domain"),
    ("web-proxy", "Web Proxy"),
    ("remote-commands", "Remote Cmds"),
    ("dns", "DNS"),
    ("settings", "Settings"),
    ("console", "Console"),
    ("hosts", "Hosts"),
];

const MENU: [MenuRow<'static>; 5] = [
    MenuRow::Item {
        label: "Server: demo@gateway",
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
    MenuRow::Separator,
    MenuRow::Item {
        label: "Quit LANlord",
        ticked: false,
        arrow: false,
        enabled: true,
        header: false,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    Desktop,
    Icon(usize),
    Window(usize),
    Title(usize),
    Close(usize),
    MenuButton,
    TaskButton(usize),
    MenuRow(usize),
}

struct Window {
    icon: usize,
    rect: Rect,
}

struct Demo {
    palette: Palette,
    selected: Option<usize>,
    windows: Vec<Window>,
    menu_open: bool,
    hovered_row: Option<usize>,
    dragging: Option<(usize, i32, i32)>,
    hits: HitMap<Hit>,
    // the desktop area of the last frame, windows are kept inside it
    desk: Rect,
    quit: bool,
    // the text field and checkbox in the my server window
    field: String,
    checked: bool,
}

impl Demo {
    fn open(&mut self, icon: usize) {
        if let Some(i) = self.windows.iter().position(|w| w.icon == icon) {
            self.raise(i);
            return;
        }
        let n = self.windows.len() as u16;
        self.windows.push(Window {
            icon,
            rect: Rect::new(30 + 3 * n, 3 + 2 * n, 44, 18),
        });
    }

    fn raise(&mut self, i: usize) {
        let w = self.windows.remove(i);
        self.windows.push(w);
    }

    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let palette = self.palette;
        let buf = frame.buffer_mut();
        self.hits.clear();
        let desk = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        self.desk = desk;
        Desktop { palette }.render(desk, buf);
        self.hits.push(Hit::Desktop, desk);
        for (i, (id, label)) in ICONS.iter().enumerate() {
            let rect = icon_rect(desk, i);
            GlyphTile {
                id,
                label,
                selected: self.selected == Some(i),
                palette,
            }
            .render(rect, buf);
            self.hits.push(Hit::Icon(i), rect);
        }
        let top = self.windows.len().saturating_sub(1);
        for (i, w) in self.windows.iter().enumerate() {
            let (id, title) = ICONS[w.icon];
            let rect = w.rect.intersection(desk);
            WindowFrame {
                title,
                icon: Some(id),
                active: i == top && !self.menu_open,
                palette,
            }
            .render(rect, buf);
            let layout = WindowFrame::layout(rect);
            self.hits.push(Hit::Window(i), rect);
            self.hits.push(Hit::Title(i), layout.title_bar);
            self.hits.push(Hit::Close(i), layout.close);
            let body = layout.body;
            if body.height >= 14 && body.width >= 30 {
                TextField {
                    text: &self.field,
                    password: false,
                    focused: i == top,
                    palette,
                }
                .render(
                    Rect::new(body.x + 2, body.y + 1, body.width.saturating_sub(4), 3),
                    buf,
                );
                Checkbox {
                    label: "Automatic updates",
                    checked: self.checked,
                    focused: false,
                    palette,
                }
                .render(Rect::new(body.x + 2, body.y + 5, 24, 1), buf);
                List {
                    rows: &["alice", "bob", "carol", "dave"],
                    selected: Some(1),
                    palette,
                }
                .render(Rect::new(body.x + 2, body.y + 7, 20, 5), buf);
                Button {
                    label: "OK",
                    pressed: false,
                    default: true,
                    enabled: true,
                    palette,
                }
                .render(
                    Rect::new(body.right().saturating_sub(24), body.y + 13, 10, 1),
                    buf,
                );
                Button {
                    label: "Cancel",
                    pressed: false,
                    default: false,
                    enabled: true,
                    palette,
                }
                .render(
                    Rect::new(body.right().saturating_sub(13), body.y + 13, 10, 1),
                    buf,
                );
            }
        }
        let bar = Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1);
        let buttons: Vec<TaskButton> = self
            .windows
            .iter()
            .map(|w| TaskButton {
                icon: Some(ICONS[w.icon].0),
                label: ICONS[w.icon].1,
                pressed: false,
            })
            .collect();
        let clock = clock();
        let taskbar = Taskbar {
            menu_label: "LANlord",
            menu_icon: Some("lanlord"),
            menu_open: self.menu_open,
            buttons: &buttons,
            clock: &clock,
            palette,
        };
        let layout = taskbar.layout(bar);
        taskbar.render(bar, buf);
        self.hits.push(Hit::MenuButton, layout.menu);
        for (i, rect) in layout.buttons.iter().enumerate() {
            self.hits.push(Hit::TaskButton(i), *rect);
        }
        if self.menu_open {
            let menu = Menu {
                rows: &MENU,
                selected: self.hovered_row,
                palette,
            };
            let (w, h) = menu.size();
            let rect = Rect::new(area.x, bar.y.saturating_sub(h), w, h);
            for (i, row) in menu.row_rects(rect).into_iter().enumerate() {
                self.hits.push(Hit::MenuRow(i), row);
            }
            menu.render(rect, buf);
        }
    }

    fn gesture(&mut self, gesture: Gesture) {
        match gesture {
            Gesture::Press { at } => {
                if let Some(Hit::Title(i)) = self.hits.hit(at) {
                    let r = self.windows[i].rect;
                    self.dragging = Some((
                        i,
                        i32::from(at.x) - i32::from(r.x),
                        i32::from(at.y) - i32::from(r.y),
                    ));
                }
            }
            Gesture::Drag { to, .. } => {
                if let Some((i, dx, dy)) = self.dragging {
                    let r = self.windows[i].rect;
                    self.windows[i].rect =
                        place(r, i32::from(to.x) - dx, i32::from(to.y) - dy, self.desk);
                }
            }
            Gesture::DragEnd { .. } => {
                if let Some((i, _, _)) = self.dragging.take() {
                    self.raise(i);
                }
            }
            Gesture::Click { at } => self.click(at),
            Gesture::DoubleClick { at } => {
                if let Some(Hit::Icon(i)) = self.hits.hit(at) {
                    self.open(i);
                }
            }
            _ => {}
        }
    }

    fn click(&mut self, at: Position) {
        let hit = self.hits.hit(at);
        if !matches!(hit, Some(Hit::MenuButton | Hit::MenuRow(_))) {
            self.menu_open = false;
        }
        match hit {
            Some(Hit::Desktop) => self.selected = None,
            Some(Hit::Icon(i)) => self.selected = Some(i),
            Some(Hit::Close(i)) => {
                self.windows.remove(i);
            }
            Some(Hit::Window(i) | Hit::Title(i) | Hit::TaskButton(i)) => self.raise(i),
            Some(Hit::MenuButton) => self.menu_open = !self.menu_open,
            Some(Hit::MenuRow(2)) => self.checked = !self.checked,
            Some(Hit::MenuRow(4)) => self.quit = true,
            _ => {}
        }
    }
}

/// the time of day in utc as `hh:mm`; the real taskbar shows the server's local time.
fn clock() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!("{:02}:{:02} UTC", secs / 3600 % 24, secs / 60 % 60)
}

fn main() -> std::io::Result<()> {
    let mut terminal = ratatui::init();
    // switches mouse reporting off when dropped or on a panic, before ratatui's restore
    let capture = Capture::start();
    let mut demo = Demo {
        palette: Palette::detect(),
        selected: None,
        windows: Vec::new(),
        menu_open: false,
        hovered_row: None,
        dragging: None,
        hits: HitMap::new(),
        desk: Rect::default(),
        quit: false,
        field: "demo@gateway".into(),
        checked: true,
    };
    demo.open(0);
    let mut gestures = Mouse::new();
    let result = (|| -> std::io::Result<()> {
        while !demo.quit {
            terminal.draw(|frame| demo.draw(frame))?;
            if !event::poll(Duration::from_millis(500))? {
                continue;
            }
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                        demo.quit = true;
                    }
                }
                Event::Mouse(report) => {
                    if let Some(Hit::MenuRow(i)) =
                        demo.hits.hit(Position::new(report.column, report.row))
                    {
                        demo.hovered_row = Some(i);
                    }
                    for g in gestures.feed(report, Instant::now()) {
                        demo.gesture(g);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    })();
    drop(capture);
    ratatui::restore();
    result
}
