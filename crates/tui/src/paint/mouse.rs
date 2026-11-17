//! mouse support: turning terminal mouse reports into the gestures the gui has (click,
//! double-click, drag, scroll) and finding which region is under a cell.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Position, Rect};

/// ask the terminal to report mouse clicks, drags and the wheel.
pub fn enable(out: &mut impl Write) -> io::Result<()> {
    execute!(out, EnableMouseCapture)
}

/// stop mouse reporting; call before leaving the terminal to the shell.
pub fn disable(out: &mut impl Write) -> io::Result<()> {
    execute!(out, DisableMouseCapture)
}

/// mouse reporting that switches itself off when dropped, and on a panic before the previous
/// panic hook (ratatui's terminal restore) runs, so a crash never leaves the shell reporting
/// mouse moves as text.
#[derive(Debug)]
pub struct Capture {
    _private: (),
}

impl Capture {
    pub fn start() -> io::Result<Capture> {
        install_panic_hook();
        enable(&mut io::stdout())?;
        Ok(Capture { _private: () })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = disable(&mut io::stdout());
    }
}

/// chain a panic hook that switches mouse reporting off first; installed once.
pub fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = disable(&mut io::stdout());
            previous(info);
        }));
    });
}

/// how close together two clicks must be to count as a double-click, like the desktop default.
pub const DOUBLE_CLICK: Duration = Duration::from_millis(500);

/// what the user did with the mouse, in cell coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    /// the left button went down on a cell.
    Press { at: Position },
    /// the left button came up where it went down, without dragging.
    Click { at: Position },
    /// a second click on the same cell within the double-click time.
    DoubleClick { at: Position },
    /// the right button went down (context menus).
    RightClick { at: Position },
    /// the mouse moved with the left button held; `from` is where the drag started.
    Drag { from: Position, to: Position },
    /// the left button came up after a drag.
    DragEnd { from: Position, to: Position },
    /// the wheel turned: negative is up.
    Scroll { at: Position, lines: i8 },
}

/// remembers the last press and click so reports can be turned into gestures.
#[derive(Debug, Default)]
pub struct Mouse {
    down: Option<Position>,
    dragged: Option<Position>,
    last_click: Option<(Position, Instant)>,
    /// the double-click time; `DOUBLE_CLICK` unless changed.
    pub double_click: Option<Duration>,
}

impl Mouse {
    pub fn new() -> Mouse {
        Mouse::default()
    }

    /// the gestures one terminal mouse report makes, given the time it arrived.
    pub fn feed(&mut self, event: MouseEvent, now: Instant) -> Vec<Gesture> {
        let at = Position::new(event.column, event.row);
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.down = Some(at);
                self.dragged = None;
                vec![Gesture::Press { at }]
            }
            MouseEventKind::Down(MouseButton::Right) => vec![Gesture::RightClick { at }],
            MouseEventKind::Drag(MouseButton::Left) => match self.down {
                Some(from) if at != from || self.dragged.is_some() => {
                    self.dragged = Some(at);
                    vec![Gesture::Drag { from, to: at }]
                }
                _ => Vec::new(),
            },
            MouseEventKind::Up(MouseButton::Left) => {
                let Some(from) = self.down.take() else {
                    return Vec::new();
                };
                if self.dragged.take().is_some() {
                    self.last_click = None;
                    return vec![Gesture::DragEnd { from, to: at }];
                }
                let limit = self.double_click.unwrap_or(DOUBLE_CLICK);
                let double = matches!(self.last_click, Some((pos, when))
                    if pos == at && now.saturating_duration_since(when) <= limit);
                if double {
                    self.last_click = None;
                    vec![Gesture::Click { at }, Gesture::DoubleClick { at }]
                } else {
                    self.last_click = Some((at, now));
                    vec![Gesture::Click { at }]
                }
            }
            MouseEventKind::ScrollUp => vec![Gesture::Scroll { at, lines: -1 }],
            MouseEventKind::ScrollDown => vec![Gesture::Scroll { at, lines: 1 }],
            _ => Vec::new(),
        }
    }
}

/// the regions drawn this frame, in drawing order, so the last one under a cell is on top.
#[derive(Debug)]
pub struct HitMap<T> {
    regions: Vec<(T, Rect)>,
}

impl<T> Default for HitMap<T> {
    fn default() -> Self {
        HitMap {
            regions: Vec::new(),
        }
    }
}

impl<T: Clone> HitMap<T> {
    pub fn new() -> HitMap<T> {
        HitMap::default()
    }

    /// forget last frame's regions.
    pub fn clear(&mut self) {
        self.regions.clear();
    }

    /// add a region drawn above everything added before it.
    pub fn push(&mut self, id: T, rect: Rect) {
        self.regions.push((id, rect));
    }

    /// the topmost region under `at`.
    pub fn hit(&self, at: Position) -> Option<T> {
        self.regions
            .iter()
            .rev()
            .find(|(_, rect)| rect.contains(at))
            .map(|(id, _)| id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn report(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn click(mouse: &mut Mouse, x: u16, y: u16, at: Instant) -> Vec<Gesture> {
        mouse.feed(report(MouseEventKind::Down(MouseButton::Left), x, y), at);
        mouse.feed(report(MouseEventKind::Up(MouseButton::Left), x, y), at)
    }

    #[test]
    fn a_capture_starts_and_switches_itself_off_when_dropped() {
        let capture = Capture::start().expect("mouse reporting starts");
        drop(capture);
    }

    #[test]
    fn two_quick_clicks_on_one_cell_are_a_double_click() {
        let mut mouse = Mouse::new();
        let start = Instant::now();
        let p = Position::new(3, 4);
        assert_eq!(click(&mut mouse, 3, 4, start), [Gesture::Click { at: p }]);
        assert_eq!(
            click(&mut mouse, 3, 4, start + Duration::from_millis(300)),
            [Gesture::Click { at: p }, Gesture::DoubleClick { at: p }]
        );
        // a third click starts over
        assert_eq!(
            click(&mut mouse, 3, 4, start + Duration::from_millis(400)),
            [Gesture::Click { at: p }]
        );
    }

    #[test]
    fn slow_or_moved_clicks_stay_single() {
        let mut mouse = Mouse::new();
        let start = Instant::now();
        click(&mut mouse, 3, 4, start);
        assert_eq!(
            click(&mut mouse, 3, 4, start + Duration::from_millis(600)).len(),
            1
        );
        let later = start + Duration::from_millis(700);
        assert_eq!(click(&mut mouse, 9, 4, later).len(), 1, "another cell");
    }

    #[test]
    fn the_double_click_time_can_be_changed() {
        let mut mouse = Mouse {
            double_click: Some(Duration::from_millis(100)),
            ..Mouse::default()
        };
        let start = Instant::now();
        click(&mut mouse, 1, 1, start);
        assert_eq!(
            click(&mut mouse, 1, 1, start + Duration::from_millis(200)).len(),
            1
        );
    }

    #[test]
    fn dragging_reports_from_where_it_started() {
        let mut mouse = Mouse::new();
        let now = Instant::now();
        mouse.feed(report(MouseEventKind::Down(MouseButton::Left), 5, 5), now);
        let from = Position::new(5, 5);
        assert_eq!(
            mouse.feed(report(MouseEventKind::Drag(MouseButton::Left), 7, 6), now),
            [Gesture::Drag {
                from,
                to: Position::new(7, 6)
            }]
        );
        assert_eq!(
            mouse.feed(report(MouseEventKind::Up(MouseButton::Left), 8, 6), now),
            [Gesture::DragEnd {
                from,
                to: Position::new(8, 6)
            }]
        );
    }

    #[test]
    fn the_wheel_and_right_button_are_reported() {
        let mut mouse = Mouse::new();
        let now = Instant::now();
        let at = Position::new(2, 2);
        assert_eq!(
            mouse.feed(report(MouseEventKind::ScrollUp, 2, 2), now),
            [Gesture::Scroll { at, lines: -1 }]
        );
        assert_eq!(
            mouse.feed(report(MouseEventKind::Down(MouseButton::Right), 2, 2), now),
            [Gesture::RightClick { at }]
        );
    }

    #[test]
    fn the_hit_map_finds_the_topmost_region() {
        let mut hits = HitMap::new();
        hits.push("desktop", Rect::new(0, 0, 80, 24));
        hits.push("window", Rect::new(10, 5, 20, 10));
        hits.push("close", Rect::new(26, 6, 3, 1));
        assert_eq!(hits.hit(Position::new(1, 1)), Some("desktop"));
        assert_eq!(hits.hit(Position::new(12, 8)), Some("window"));
        assert_eq!(hits.hit(Position::new(27, 6)), Some("close"));
        assert_eq!(hits.hit(Position::new(90, 1)), None);
        hits.clear();
        assert_eq!(hits.hit(Position::new(1, 1)), None);
    }
}
