//! the update notice: a shield beside the taskbar clock, a balloon the first time an update
//! turns up, and a small box listing what is new. the check runs in the background and says
//! nothing when it fails.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Area, Color32, Frame, Id, Margin, Order, Sense, Ui, Vec2};
use lanlord_core::prefs::{self, Prefs};
use lanlord_core::update::{self, Runner, Source, Update};

use crate::dialogs::modal;
use crate::icons;
use crate::theme::{self, BLACK};

/// how long the balloon stays up before it goes away on its own.
const BALLOON_FOR: Duration = Duration::from_secs(12);
/// the pale yellow of a classic tooltip.
const TIP: Color32 = Color32::from_rgb(255, 255, 225);

/// what the box was answered with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Install,
    Later,
}

pub struct Updates {
    runner: Arc<dyn Runner>,
    enabled: bool,
    prefs_path: Option<PathBuf>,
    found: Option<Update>,
    pending: Option<Receiver<Option<Update>>>,
    last_check: Option<Instant>,
    /// when the balloon went up; `None` before it does and after it was closed or timed out.
    balloon_since: Option<Instant>,
    balloon_done: bool,
    /// "remind me later": the shield hides until the next check finds something.
    snoozed: bool,
    dialog: bool,
}

impl Updates {
    /// a notice that checks with `runner` when `enabled`, and keeps the switch in `prefs_path`.
    pub fn new(runner: Arc<dyn Runner>, prefs_path: Option<PathBuf>, enabled: bool) -> Self {
        Self {
            runner,
            enabled,
            prefs_path,
            found: None,
            pending: None,
            last_check: None,
            balloon_since: None,
            balloon_done: false,
            snoozed: false,
            dialog: false,
        }
    }

    /// a notice that never checks: for tests and for lanlord running on the gateway.
    pub fn quiet() -> Self {
        Self::new(Arc::new(update::System), None, false)
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// turn the background check on or off and remember it.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.found = None;
            self.dialog = false;
        }
        if let Some(path) = &self.prefs_path {
            let _ = prefs::save(path, &Prefs { check_updates: on });
        }
    }

    /// pretend a check found `update`, for tests.
    #[cfg(test)]
    pub fn pretend(&mut self, update: Update) {
        self.arrived(Some(update));
    }

    /// the update that is waiting, when there is one.
    pub fn found(&self) -> Option<&Update> {
        self.found.as_ref()
    }

    /// is the shield showing?
    pub fn visible(&self) -> bool {
        self.found.is_some() && !self.snoozed
    }

    #[cfg(test)]
    pub fn dialog_open(&self) -> bool {
        self.dialog
    }

    pub fn open_dialog(&mut self) {
        if self.found.is_some() {
            self.dialog = true;
            self.balloon_since = None;
            self.balloon_done = true;
        }
    }

    pub fn remind_later(&mut self) {
        self.snoozed = true;
        self.dialog = false;
    }

    /// look now, in the background.
    pub fn check_now(&mut self, ctx: &egui::Context) {
        if self.pending.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let runner = self.runner.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(update::check(&*runner, true));
            ctx.request_repaint();
        });
        self.pending = Some(rx);
        self.last_check = Some(Instant::now());
    }

    /// once a frame: collect a finished check and start the next one when it is time.
    pub fn tick(&mut self, ctx: &egui::Context) {
        if !self.enabled {
            return;
        }
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(update) => {
                    self.pending = None;
                    self.arrived(update);
                }
                Err(TryRecvError::Disconnected) => self.pending = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        let due = self
            .last_check
            .is_none_or(|at| at.elapsed() >= update::CHECK_EVERY);
        if due && self.pending.is_none() {
            self.check_now(ctx);
        }
    }

    /// what a check found: show the shield, and the balloon the first time.
    fn arrived(&mut self, found: Option<Update>) {
        match found {
            Some(update) => {
                if self.found.is_none() {
                    self.snoozed = false;
                    if !self.balloon_done {
                        self.balloon_since = Some(Instant::now());
                    }
                }
                self.found = Some(update);
            }
            None => {
                self.found = None;
                self.dialog = false;
            }
        }
    }

    /// the shield beside the clock; true when it was clicked.
    pub fn shield(&mut self, ui: &mut Ui) -> bool {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(22.0, 22.0), Sense::click());
        #[cfg(test)]
        theme::probe::record(ui, "shield", rect, true);
        icons::draw(
            ui.painter(),
            rect.left_top() + Vec2::new(3.0, 3.0),
            icons::small("update"),
            1.0,
        );
        response
            .on_hover_text("Updates are available for LANlord")
            .clicked()
    }

    /// the balloon over the clock; true when it was clicked.
    pub fn balloon(&mut self, ctx: &egui::Context, taskbar: f32) -> bool {
        let Some(since) = self.balloon_since else {
            return false;
        };
        if !self.visible() || since.elapsed() >= BALLOON_FOR {
            self.balloon_since = None;
            self.balloon_done = true;
            return false;
        }
        let mut clicked = false;
        let mut closed = false;
        Area::new(Id::unique("update-balloon"))
            .order(Order::Tooltip)
            .anchor(Align2::RIGHT_BOTTOM, Vec2::new(-8.0, -(taskbar + 10.0)))
            .show(ctx, |ui| {
                let framed = Frame::new()
                    .fill(TIP)
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(250.0);
                        ui.horizontal(|ui| {
                            let (art, _) =
                                ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                            icons::draw(ui.painter(), art.min, icons::small("update"), 1.0);
                            ui.colored_label(
                                BLACK,
                                egui::RichText::new("LANlord updates").strong(),
                            );
                        });
                        ui.colored_label(
                            BLACK,
                            "Updates are available for LANlord. Click here to install them.",
                        );
                    });
                let rect = framed.response.rect;
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(1.0, BLACK),
                    egui::StrokeKind::Inside,
                );
                let body = ui.interact(rect, Id::unique("update-balloon-body"), Sense::click());
                // the close mark in the corner, on top of the body so it gets its own clicks
                let mark = egui::Rect::from_center_size(
                    rect.right_top() + Vec2::new(-10.0, 10.0),
                    Vec2::splat(12.0),
                );
                let close = ui.interact(mark, Id::unique("update-balloon-close"), Sense::click());
                ui.painter().text(
                    mark.center(),
                    Align2::CENTER_CENTER,
                    "x",
                    theme::font(),
                    BLACK,
                );
                #[cfg(test)]
                {
                    theme::probe::record(ui, "balloon", rect, true);
                    theme::probe::record(ui, "balloon-close", mark, true);
                }
                if close.clicked() {
                    closed = true;
                } else if body.clicked() {
                    clicked = true;
                }
            });
        if closed {
            self.balloon_since = None;
            self.balloon_done = true;
        }
        clicked
    }

    /// the box listing what is new; `Some` once it was answered.
    pub fn dialog(&mut self, ctx: &egui::Context) -> Option<Choice> {
        if !self.dialog {
            return None;
        }
        let Some(update) = self.found.clone() else {
            self.dialog = false;
            return None;
        };
        let mut automatic = self.enabled;
        let mut choice = None;
        modal(ctx, "update-box", "LANlord Update", |ui| {
            ui.horizontal(|ui| {
                let (art, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                icons::draw(ui.painter(), art.min, icons::small("update"), 1.0);
                ui.label(egui::RichText::new(&update.headline).strong());
            });
            ui.label("The following updates are available:");
            theme::sunken(ui, Color32::WHITE, |ui| {
                ui.set_width(ui.available_width() - 6.0);
                egui::ScrollArea::vertical()
                    .max_height(140.0)
                    .id_salt("update-list")
                    .show(ui, |ui| {
                        for line in &update.lines {
                            ui.colored_label(BLACK, line);
                        }
                        if update.lines.is_empty() {
                            ui.colored_label(BLACK, "(no details available)");
                        }
                    });
            });
            ui.add_space(4.0);
            ui.label(match update.source {
                Source::Git => "Install rebuilds LANlord from trunk and restarts it.",
                Source::Release { .. } => "Install opens the download page.",
            });
            crate::controls::check(ui, &mut automatic, "Check for updates automatically");
            ui.add_space(6.0);
            crate::controls::buttons(ui, |ui| {
                if theme::button(ui, "Remind me later", false).clicked() {
                    choice = Some(Choice::Later);
                }
                if theme::button(ui, "Install", false).clicked() {
                    choice = Some(Choice::Install);
                }
            });
        });
        if automatic != self.enabled {
            self.set_enabled(automatic);
        }
        match choice {
            Some(Choice::Later) => self.remind_later(),
            Some(Choice::Install) => self.dialog = false,
            None => {}
        }
        choice
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit;
    use std::path::Path;

    /// pretends to be git: always one new commit.
    struct Behind;

    impl Runner for Behind {
        fn run(&self, _p: &str, args: &[&str], _d: Option<&Path>) -> anyhow::Result<String> {
            Ok(match args.first().copied() {
                Some("rev-list") => "1\n".into(),
                Some("log") => "abc feat: something new\n".into(),
                _ => String::new(),
            })
        }
    }

    fn pretend() -> Update {
        Update {
            source: Source::Git,
            headline: "1 new change on trunk".into(),
            lines: vec!["abc feat: something new".into()],
        }
    }

    fn found() -> Updates {
        let mut updates = Updates::new(Arc::new(Behind), None, true);
        updates.arrived(Some(pretend()));
        updates
    }

    fn frame(
        updates: &mut Updates,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> (bool, bool, Option<Choice>) {
        let (mut shield, mut balloon, mut choice) = (false, false, None);
        testkit::run(ctx, events, |ui| {
            theme::apply(ui.ctx());
            if updates.visible() {
                shield = updates.shield(ui);
            }
            balloon = updates.balloon(ui.ctx(), 30.0);
            choice = updates.dialog(ui.ctx());
        });
        (shield, balloon, choice)
    }

    fn click(
        updates: &mut Updates,
        ctx: &egui::Context,
        label: &str,
    ) -> (bool, bool, Option<Choice>) {
        for _ in 0..3 {
            frame(updates, ctx, Vec::new());
        }
        let (rect, _) = theme::probe::find(ctx, label).expect(label);
        let mut result = (false, false, None);
        for event in testkit::click(rect.center()) {
            let now = frame(updates, ctx, vec![event]);
            result = (result.0 || now.0, result.1 || now.1, result.2.or(now.2));
        }
        result
    }

    #[test]
    fn nothing_shows_until_something_is_found() {
        let mut updates = Updates::new(Arc::new(Behind), None, true);
        let ctx = egui::Context::default();
        assert!(!updates.visible());
        frame(&mut updates, &ctx, Vec::new());
        assert!(theme::probe::find(&ctx, "shield").is_none());
        assert!(updates.found().is_none());
    }

    #[test]
    fn a_found_update_shows_the_shield_and_a_balloon_once() {
        let mut updates = found();
        let ctx = egui::Context::default();
        assert!(updates.visible());
        frame(&mut updates, &ctx, Vec::new());
        assert!(theme::probe::find(&ctx, "shield").is_some());
        assert!(theme::probe::find(&ctx, "balloon").is_some());
        // closing the balloon keeps the shield, and the balloon does not come back
        click(&mut updates, &ctx, "balloon-close");
        assert!(updates.visible() && updates.balloon_since.is_none());
        updates.arrived(Some(pretend()));
        assert!(updates.balloon_since.is_none(), "one balloon only");
    }

    #[test]
    fn clicking_the_balloon_or_the_shield_opens_the_box() {
        let mut updates = found();
        let ctx = egui::Context::default();
        let (_, balloon, _) = click(&mut updates, &ctx, "balloon");
        assert!(balloon);
        updates.open_dialog();
        assert!(updates.dialog_open());
        let mut again = found();
        let ctx = egui::Context::default();
        let (shield, _, _) = click(&mut again, &ctx, "shield");
        assert!(shield);
    }

    #[test]
    fn the_box_installs_or_reminds_later() {
        let mut updates = found();
        let ctx = egui::Context::default();
        updates.open_dialog();
        let (_, _, choice) = click(&mut updates, &ctx, "Install");
        assert_eq!(choice, Some(Choice::Install));
        assert!(!updates.dialog_open());

        let mut later = found();
        let ctx = egui::Context::default();
        later.open_dialog();
        let (_, _, choice) = click(&mut later, &ctx, "Remind me later");
        assert_eq!(choice, Some(Choice::Later));
        assert!(!later.visible(), "the shield hides until the next check");
        later.arrived(None);
        later.arrived(Some(pretend()));
        assert!(later.visible(), "a new find brings it back");
    }

    #[test]
    fn nothing_found_clears_the_shield_and_the_box() {
        let mut updates = found();
        updates.open_dialog();
        updates.arrived(None);
        assert!(!updates.visible() && !updates.dialog_open());
        let ctx = egui::Context::default();
        assert_eq!(frame(&mut updates, &ctx, Vec::new()).2, None);
    }

    #[test]
    fn the_background_check_finds_updates_and_respects_the_switch() {
        let ctx = egui::Context::default();
        let mut off = Updates::quiet();
        off.tick(&ctx);
        assert!(
            off.pending.is_none() && off.last_check.is_none(),
            "off means no check"
        );

        let mut updates = Updates::new(Arc::new(Behind), None, true);
        updates.tick(&ctx);
        assert!(updates.pending.is_some());
        for _ in 0..200 {
            updates.tick(&ctx);
            if updates.pending.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(updates.pending.is_none());
        // the finished check either found the commit or, with no checkout here, nothing
        let _ = updates.found();
        // a second tick inside the hour starts nothing
        updates.tick(&ctx);
        assert!(updates.pending.is_none());
        updates.check_now(&ctx);
        updates.check_now(&ctx);
        assert!(updates.pending.is_some());
    }

    #[test]
    fn the_switch_is_remembered_in_the_preferences_file() {
        let dir = std::env::temp_dir().join(format!("lanlord-upd-{}", std::process::id()));
        let file = dir.join("prefs");
        let mut updates = Updates::new(Arc::new(Behind), Some(file.clone()), true);
        updates.arrived(Some(pretend()));
        updates.set_enabled(false);
        assert!(!updates.enabled() && !updates.visible());
        assert!(!prefs::load(&file).check_updates);
        updates.set_enabled(true);
        assert!(prefs::load(&file).check_updates);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_box_has_a_switch_for_automatic_checks() {
        let dir = std::env::temp_dir().join(format!("lanlord-upd2-{}", std::process::id()));
        let file = dir.join("prefs");
        let mut updates = Updates::new(Arc::new(Behind), Some(file.clone()), true);
        updates.arrived(Some(pretend()));
        updates.open_dialog();
        let ctx = egui::Context::default();
        for _ in 0..3 {
            frame(&mut updates, &ctx, Vec::new());
        }
        assert!(updates.dialog_open());
        let _ = std::fs::remove_dir_all(dir);
    }
}
