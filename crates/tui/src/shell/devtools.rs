//! developer tools, compiled only with the `devtools` cargo feature: the red bug on the taskbar
//! (and in the corner of the logon screen) that opens a debug menu with a demo desktop on canned
//! data, window-manager outlines and "open all windows". the same tools as the gui's; release
//! builds never contain any of it.

use std::sync::Arc;

use lanlord_app::admin_setup::AdminSetup;
use lanlord_app::devtools::{self, Choice};
use lanlord_app::gateway::Fake;
use lanlord_app::hostlist::HostList;
use lanlord_app::login::{Auth, LoginBox, SignedIn};
use lanlord_app::services::Services;
use lanlord_app::wake::Wake;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Position, Rect};
use ratatui::style::Color;
use ratatui::widgets::Widget;

use super::Shell;
use crate::paint::chrome::{bug_button, Menu, MenuRow, BUG_RED, BUG_WIDTH};
use crate::paint::palette::GRAY;
use crate::paint::{bold, style, text};

/// what the desktop was showing before the demo took over; leaving the demo puts it back.
struct Stash {
    services: Services,
    hosts: HostList,
    login: Option<LoginBox>,
    auth: Option<Arc<dyn Auth>>,
    signed_in: Option<SignedIn>,
    admin_setup: Option<AdminSetup>,
    demo: bool,
}

#[derive(Default)]
pub(super) struct State {
    /// the bug is not drawn; screenshots and snapshots of the plain desktop turn it off.
    hidden: bool,
    /// the debug menu is open, with the keyboard on this row.
    pub(super) menu: Option<usize>,
    /// outline every window and say how big it is.
    wm_debug: bool,
    /// the real desktop, kept while the demo desktop is showing.
    stash: Option<Stash>,
    /// the bug in the corner of the logon screen, as drawn last frame.
    corner: Option<Rect>,
    /// the open debug menu and its rows, as drawn last frame.
    rows: Option<(Rect, Vec<Rect>)>,
}

impl State {
    pub(super) fn new() -> Self {
        Self::default()
    }
}

impl Shell {
    /// is the red bug showing?
    pub(super) fn bug_shown(&self) -> bool {
        !self.devtools.hidden
    }

    /// leave the bug out, so a picture of the plain desktop is the same with and without it.
    #[cfg(test)]
    pub(super) fn hide_bug(&mut self) {
        self.devtools.hidden = true;
    }

    /// start of a frame: forget where the bug and its menu were drawn.
    pub(super) fn devtools_begin(&mut self) {
        self.devtools.corner = None;
        self.devtools.rows = None;
    }

    /// is the demo desktop (canned data, no session) showing?
    pub(super) fn in_demo(&self) -> bool {
        self.devtools.stash.is_some()
    }

    /// is the debug menu open?
    #[cfg(test)]
    pub(super) fn debug_menu_open(&self) -> bool {
        self.devtools.menu.is_some()
    }

    /// are the window outlines on?
    #[cfg(test)]
    pub(super) fn wm_debug(&self) -> bool {
        self.devtools.wm_debug
    }

    /// the bug, on the taskbar or in the corner of the logon screen.
    fn bug_rect(&self) -> Option<Rect> {
        self.hits
            .taskbar
            .as_ref()
            .and_then(|bar| bar.bug)
            .or(self.devtools.corner)
    }

    /// forget what the windows showed, so the next desktop starts clean.
    fn reset_session(&mut self) {
        self.windows.clear();
        self.views.clear();
        self.active = None;
        self.selected = None;
        self.menu = None;
        self.submenu = None;
        self.modal = None;
        self.reset_windows();
    }

    /// switch to a desktop on the made-up server: nothing here talks to the real one. the real
    /// logon, session and hosts come back when it is left.
    pub(super) fn enter_demo(&mut self) {
        if self.in_demo() {
            return;
        }
        self.reset_session();
        let gateway = Fake::demo();
        let services = Services::new(Arc::new(gateway), Wake::none());
        self.devtools.stash = Some(Stash {
            services: std::mem::replace(&mut self.services, services),
            hosts: std::mem::replace(&mut self.hosts, HostList::new(None)),
            login: self.login.take(),
            auth: self.auth.take(),
            signed_in: self.signed_in.take(),
            admin_setup: self.admin_setup.take(),
            demo: std::mem::replace(&mut self.demo, true),
        });
    }

    /// leave the demo desktop and show what was there before; false outside the demo.
    pub(super) fn leave_demo(&mut self) -> bool {
        let Some(stash) = self.devtools.stash.take() else {
            return false;
        };
        self.reset_session();
        self.services = stash.services;
        self.hosts = stash.hosts;
        self.login = stash.login;
        self.auth = stash.auth;
        self.signed_in = stash.signed_in;
        self.admin_setup = stash.admin_setup;
        self.demo = stash.demo;
        true
    }

    /// open every window there is, for looking at the window manager.
    fn open_all(&mut self) {
        for target in devtools::open_all(&self.icons) {
            self.open(target);
        }
    }

    /// the bug in the lower right corner of the logon screen, which has no taskbar.
    pub(super) fn render_corner_bug(&mut self, area: Rect, buf: &mut Buffer) {
        if !self.bug_shown() {
            return;
        }
        let width = BUG_WIDTH.min(area.width);
        let rect = Rect::new(
            area.right().saturating_sub(width + 1).max(area.x),
            area.bottom().saturating_sub(1),
            width,
            1,
        );
        bug_button(buf, rect, self.palette);
        self.devtools.corner = Some(rect);
    }

    /// the debug menu, above the bug, when it is open.
    pub(super) fn render_debug_menu(&mut self, buf: &mut Buffer) {
        let Some(hover) = self.devtools.menu else {
            return;
        };
        let Some(bug) = self.bug_rect() else {
            self.devtools.menu = None;
            return;
        };
        let entries = devtools::entries(self.in_demo(), self.devtools.wm_debug);
        let items: Vec<MenuRow> = entries
            .iter()
            .map(|entry| MenuRow::Item {
                label: entry.label,
                ticked: entry.ticked,
                arrow: false,
                enabled: true,
                header: false,
            })
            .collect();
        let menu = Menu {
            rows: &items,
            selected: Some(hover),
            palette: self.palette,
        };
        let (width, height) = menu.size();
        let rect = Rect::new(
            bug.right().saturating_sub(width).max(buf.area.x),
            bug.y.saturating_sub(height),
            width,
            height,
        )
        .intersection(buf.area);
        let rows = menu.row_rects(rect);
        menu.render(rect, buf);
        self.devtools.rows = Some((rect, rows));
    }

    /// the window-manager outlines: each window's edge in red, and how big it is.
    pub(super) fn outline_windows(&mut self, buf: &mut Buffer) {
        if !self.devtools.wm_debug {
            return;
        }
        let red = style(self.palette, BUG_RED, GRAY);
        for win in self.windows.iter().filter(|w| !w.minimized) {
            let rect = win.rect.intersection(self.hits.desktop);
            if rect.is_empty() {
                continue;
            }
            for x in rect.left()..rect.right() {
                for y in [rect.top(), rect.bottom() - 1] {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_fg(Color::Red);
                    }
                }
            }
            for y in rect.top()..rect.bottom() {
                for x in [rect.left(), rect.right() - 1] {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_fg(Color::Red);
                    }
                }
            }
            // on the top border, so it never covers a status bar at the bottom
            let size = format!("{}: {}x{}", win.target.id(), rect.width, rect.height);
            text(
                buf,
                rect.x + 2,
                rect.y,
                rect.width.saturating_sub(4),
                &size,
                bold(red),
            );
        }
    }

    pub(super) fn pick_debug(&mut self, n: usize) {
        let entries = devtools::entries(self.in_demo(), self.devtools.wm_debug);
        match entries.get(n).map(|entry| entry.choice) {
            Some(Choice::Demo) => {
                if !self.leave_demo() {
                    self.enter_demo();
                }
            }
            Some(Choice::WindowDebug) => self.devtools.wm_debug = !self.devtools.wm_debug,
            Some(Choice::OpenAll) => self.open_all(),
            None => {}
        }
    }

    /// a box is open that the demo desktop must not outlive: the logon and first-administrator
    /// screens are fine, they are what the bug sits on.
    fn box_open(&mut self) -> bool {
        !matches!(self.dialog_id(), None | Some("logon" | "admin-setup"))
    }

    /// a click on the bug or in its menu; false when the click is for something else.
    pub(super) fn click_devtools(&mut self, at: Position) -> bool {
        if self.box_open() {
            self.devtools.menu = None;
            return false;
        }
        if let Some((rect, rows)) = self.devtools.rows.clone() {
            if rect.contains(at) {
                if let Some(n) = rows.iter().position(|row| row.contains(at)) {
                    self.pick_debug(n);
                }
                return true;
            }
        }
        if self.bug_rect().is_some_and(|bug| bug.contains(at)) {
            self.devtools.menu = self.devtools.menu.is_none().then_some(0);
            return true;
        }
        self.devtools.menu = None;
        false
    }

    /// F12 opens the debug menu; while it is open the arrows, Enter and Escape drive it.
    pub(super) fn key_devtools(&mut self, key: KeyEvent) -> bool {
        if self.box_open() {
            self.devtools.menu = None;
            return false;
        }
        if key.code == KeyCode::F(12) {
            self.devtools.menu = self.devtools.menu.is_none().then_some(0);
            return true;
        }
        let Some(hover) = self.devtools.menu else {
            return false;
        };
        let count = devtools::entries(false, false).len();
        match key.code {
            KeyCode::Esc => self.devtools.menu = None,
            KeyCode::Up => self.devtools.menu = Some(hover.checked_sub(1).unwrap_or(count - 1)),
            KeyCode::Down => self.devtools.menu = Some((hover + 1) % count),
            KeyCode::Enter => self.pick_debug(hover),
            _ => {}
        }
        true
    }
}
