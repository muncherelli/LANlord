//! the LANlord menu's upkeep actions in the terminal desktop: the update notice (the tray label,
//! the box that lists what is new, the switch and "check for updates now"), rebuilding a
//! development build and the Developer submenu that holds them.

use std::path::PathBuf;

use lanlord_app::form::Form;
use lanlord_app::menu::{self, Action, Row};
use lanlord_app::rebuild::Started;
use lanlord_app::update_box::{self, Choice, Msg};
use lanlord_app::updates::Notice;
use lanlord_app::wake::Wake;
use lanlord_core::log::{emit, Kind};
use lanlord_core::update::{self, Source};
use lanlord_core::updater::State;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::Widget;

use super::{drive_dialog, Answer, Modal, Pass, Shell, ViewState};
use crate::paint::chrome::{Menu, MenuRow};
use lanlord_app::desktop::Target;

/// the menu rows as the menu widget draws them.
pub(super) fn menu_rows(rows: &[Row]) -> Vec<MenuRow<'_>> {
    rows.iter()
        .map(|row| match row {
            Row::Item(item) => MenuRow::Item {
                label: &item.label,
                ticked: item.ticked,
                arrow: item.arrow,
                enabled: item.enabled,
                header: item.header,
            },
            Row::Separator => MenuRow::Separator,
        })
        .collect()
}

/// what the tray says about the update, in as few columns as will do.
pub(super) fn label_for(state: &State) -> Option<&'static str> {
    Some(match state {
        State::Idle => return None,
        State::Available => "Update ready",
        State::Downloading => "Downloading",
        State::Installing => "Installing",
        State::Paused(_) => "Update paused",
        State::Failed(_) => "Update failed",
        State::Done => "Restart now",
    })
}

impl Shell {
    /// look for updates with `notice`, and rebuild from `repo` as the running `exe` when this is a
    /// development build.
    pub fn watch_updates(&mut self, notice: Notice, repo: Option<PathBuf>, exe: Option<PathBuf>) {
        self.updates = notice;
        if let (Some(repo), Some(exe)) = (&repo, &exe) {
            self.updates.watch_checkout(repo.clone(), exe.clone());
        }
        self.rebuild.watch(repo, exe);
    }

    /// sign in again at once to `host`, as a rebuilt build does with the server it was on.
    pub fn resume(&mut self, host: &str) {
        let Some(auth) = self.auth.clone() else {
            return;
        };
        let mut login = lanlord_app::login::LoginBox::new(auth, Wake::none(), Some(host));
        login.submit();
        self.login = Some(login);
    }

    /// the new build to start in this terminal once the tui has ended, and the server it should
    /// sign in to again; `None` unless a rebuild asked for it.
    pub fn take_restart(&mut self) -> Option<(PathBuf, Option<String>)> {
        self.restart.take()
    }

    /// the update item in the tray, when an update is waiting or going on.
    pub(super) fn update_label(&self) -> Option<String> {
        if !self.updates.visible() {
            return None;
        }
        label_for(self.updates.state()).map(String::from)
    }

    /// is the box listing what is new open?
    pub(super) fn update_dialog_open(&self) -> bool {
        self.updates.dialog && self.updates.found().is_some()
    }

    /// the switch in the menu: turn the background check on or off, and look at once when on.
    pub(super) fn toggle_updates(&mut self) {
        let on = !self.updates.enabled();
        self.updates.set_enabled(on);
        if on {
            self.updates.check_now(&Wake::none());
        }
    }

    pub(super) fn check_updates(&mut self) {
        emit(Kind::Info, "looking for updates");
        self.updates.check_now(&Wake::none());
    }

    /// ask before rebuilding, with a tick for pulling trunk first.
    pub(super) fn ask_rebuild(&mut self) {
        let Some(text) = self.rebuild.question() else {
            return;
        };
        let form = Form::new("rebuild", "Rebuild and relaunch", &text)
            .check("Pull the latest trunk first", false);
        self.modal = Some(Modal::Rebuild(form));
    }

    /// rebuild in the background with the output in the console. the tui only ends once the new
    /// build is ready; every other outcome leaves it open with the reason in the console and in a
    /// box.
    pub(super) fn start_rebuild(&mut self, pull: bool) {
        let host = self.signed_in.as_ref().map(|s| s.target.clone());
        match self.rebuild.start(pull, host.as_deref(), &Wake::none()) {
            Started::Running => self.open(Target::Console),
            Started::Failed(error) => self.rebuild_failed(&error),
            Started::Ignored => {}
        }
    }

    /// say that a rebuild did not work, in the console and in a box, and carry on.
    fn rebuild_failed(&mut self, error: &str) {
        emit(Kind::Err, format!("the rebuild failed: {error}"));
        self.show_failure(error);
    }

    /// the box that says a rebuild did not work.
    pub(super) fn show_failure(&mut self, error: &str) {
        let text = format!("The rebuild did not work, so LANlord stays open.\n{error}");
        self.modal = Some(Modal::Failure(Form::new(
            "rebuild-failed",
            "Rebuild and relaunch",
            &text,
        )));
    }

    /// the new build is ready: end here, and start it in this terminal.
    fn hand_over(&mut self) {
        let host = self.signed_in.as_ref().map(|s| s.target.clone());
        self.restart = self.rebuild.exe().map(|exe| (exe.to_path_buf(), host));
        self.quit = true;
    }

    /// once a frame: the update check, the updater and the rebuild.
    pub(super) fn poll_maintenance(&mut self) {
        self.updates.tick(&Wake::none());
        if self.updates.dialog && self.updates.found().is_none() {
            self.updates.dialog = false;
        }
        match self.rebuild.poll() {
            Some(Ok(())) => self.hand_over(),
            Some(Err(error)) => self.rebuild_failed(&error),
            None => {}
        }
    }

    /// install what the notice found: rebuild from trunk, or open the release page.
    fn install_update(&mut self) {
        let Some(found) = self.updates.found().cloned() else {
            return;
        };
        match found.source {
            Source::Git => {
                // the pull and the build run in the background, and say so in the console
                self.open(Target::Console);
                self.updates.install();
            }
            Source::Release { url } => {
                if let Err(error) = update::open_release(&url) {
                    emit(
                        Kind::Err,
                        format!("could not open the download page: {error}"),
                    );
                }
            }
        }
    }

    /// start the build that was just installed here, once it is known to run.
    fn restart_into_new_build(&mut self) {
        let host = self.signed_in.as_ref().map(|s| s.target.clone());
        self.rebuild.restart(host.as_deref(), &Wake::none());
    }

    pub(super) fn answer_update(&mut self, msg: Msg) {
        match msg {
            Msg::ToggleAutomatic => {
                let on = !self.updates.enabled();
                self.updates.set_enabled(on);
            }
            Msg::Choose(Choice::Install) => {
                self.updates.dialog = false;
                self.install_update();
            }
            Msg::Choose(Choice::Restart) => {
                self.updates.dialog = false;
                self.restart_into_new_build();
            }
            Msg::Choose(Choice::Later) => self.updates.remind_later(),
            Msg::Choose(Choice::Pause) => self.updates.pause(),
            Msg::Choose(Choice::Resume) => self.updates.resume(),
            Msg::Choose(Choice::Cancel) => self.updates.cancel(),
        }
    }

    /// the box listing what is new, drawn or answered; `None` when it is not open.
    pub(super) fn update_dialog(
        &mut self,
        pass: &mut Pass,
        state: &ViewState,
        escape: bool,
    ) -> Option<(String, Answer)> {
        if !self.update_dialog_open() {
            return None;
        }
        let found = self.updates.found()?.clone();
        let mut dialog = update_box::view(&found, self.updates.state(), self.updates.enabled());
        // closing the box is "remind me later"
        dialog.escape = Some(Msg::Choose(Choice::Later));
        let (answer, msg) = drive_dialog(&dialog, pass, state, self.palette, escape);
        if let Some(msg) = msg {
            self.answer_update(msg);
        }
        Some((dialog.title, answer))
    }

    /// the Developer submenu, beside the menu row that opened it.
    pub(super) fn render_submenu(
        &mut self,
        rows: &[Row],
        row_rects: &[(Rect, usize)],
        menu_rect: Rect,
        buf: &mut Buffer,
    ) {
        let Some(hover) = self.submenu else {
            return;
        };
        let opener = rows
            .iter()
            .position(|row| matches!(row, Row::Item(item) if item.action == Action::Developer));
        let Some(row) = opener.and_then(|n| row_rects.iter().find(|(_, i)| *i == n)) else {
            self.submenu = None;
            return;
        };
        let developer = menu::developer_rows(self.rebuild.busy());
        let items = menu_rows(&developer);
        let menu = Menu {
            rows: &items,
            selected: Some(hover),
            palette: self.palette,
        };
        let (width, height) = menu.size();
        let desktop = self.hits.desktop;
        let right = menu_rect.right();
        let x = if right + width <= desktop.right() {
            right
        } else {
            menu_rect.x.saturating_sub(width)
        };
        let y = row
            .0
            .y
            .saturating_sub(1)
            .min(desktop.bottom().saturating_sub(height));
        let rect = Rect::new(x, y, width, height).intersection(desktop);
        let rects = menu.row_rects(rect).into_iter().zip(0..).collect();
        menu.render(rect, buf);
        self.hits.submenu = Some((rect, rects));
    }

    fn pick_submenu(&mut self, n: usize) {
        let rows = menu::developer_rows(self.rebuild.busy());
        if let Some(Row::Item(item)) = rows.get(n) {
            if item.enabled {
                self.do_action(item.action.clone());
            }
        }
    }

    /// clicks on the bug, in the Developer submenu or on the update item; false when the click
    /// is for something else.
    pub(super) fn click_extras(&mut self, at: Position) -> bool {
        // the bug sits above every box, the logon box included
        if self.click_devtools(at) {
            return true;
        }
        if self.dialog_id().is_some() {
            return false;
        }
        if let Some((rect, rows)) = self.hits.submenu.clone() {
            if rect.contains(at) {
                if let Some((_, n)) = rows.iter().find(|(r, _)| r.contains(at)) {
                    self.pick_submenu(*n);
                }
                return true;
            }
        }
        let shield = self.hits.taskbar.as_ref().and_then(|bar| bar.shield);
        if shield.is_some_and(|rect| rect.contains(at)) {
            self.menu = None;
            self.updates.open_dialog();
            return true;
        }
        false
    }

    /// keys for the debug menu, the Developer submenu and the update item; true when used.
    pub(super) fn key_extras(&mut self, key: KeyEvent) -> bool {
        if self.key_devtools(key) {
            return true;
        }
        if self.dialog_id().is_some() {
            return false;
        }
        if key.code == KeyCode::F(9) && !self.before_desktop() && self.updates.visible() {
            self.updates.open_dialog();
            return true;
        }
        self.key_submenu(key)
    }

    fn key_submenu(&mut self, key: KeyEvent) -> bool {
        let Some(hover) = self.submenu else {
            // Right opens the submenu from its row
            let on_developer = self.menu.is_some_and(|n| {
                matches!(self.menu_state().get(n),
                    Some(Row::Item(item)) if item.action == Action::Developer)
            });
            if on_developer && key.code == KeyCode::Right {
                self.submenu = Some(0);
                return true;
            }
            return false;
        };
        let count = menu::developer_rows(self.rebuild.busy()).len();
        match key.code {
            KeyCode::Esc | KeyCode::Left => self.submenu = None,
            KeyCode::Up => self.submenu = Some(hover.checked_sub(1).unwrap_or(count - 1)),
            KeyCode::Down => self.submenu = Some((hover + 1) % count),
            KeyCode::Enter => self.pick_submenu(hover),
            _ => {}
        }
        true
    }
}
