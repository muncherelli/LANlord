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

/// how long the balloon stays up in full before it starts to fade, and how long the fade takes.
const BALLOON_FOR: Duration = Duration::from_secs(8);
const BALLOON_FADE: Duration = Duration::from_millis(600);
/// how long the tray tooltip takes to fade once the pointer has left the shield.
const TIP_FADE: Duration = Duration::from_millis(250);
/// the pale yellow of a classic tooltip.
const TIP: Color32 = Color32::from_rgb(255, 255, 225);

/// the time the notice measures with; a real clock in the app and a fake one in tests.
pub trait Clock: Send + Sync {
    /// time since some fixed start.
    fn now(&self) -> Duration;
}

struct RealClock(Instant);

impl Clock for RealClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
}

/// how opaque the balloon is `age` after it went up: whole, then fading, then gone.
fn balloon_opacity(age: Duration) -> Option<f32> {
    if age < BALLOON_FOR {
        Some(1.0)
    } else if age < BALLOON_FOR + BALLOON_FADE {
        Some(1.0 - (age - BALLOON_FOR).as_secs_f32() / BALLOON_FADE.as_secs_f32())
    } else {
        None
    }
}

/// how opaque the tooltip is `since_left` after the pointer left the shield (`None` while over it).
fn tip_opacity(since_left: Option<Duration>) -> f32 {
    match since_left {
        None => 1.0,
        Some(gone) if gone >= TIP_FADE => 0.0,
        Some(gone) => 1.0 - gone.as_secs_f32() / TIP_FADE.as_secs_f32(),
    }
}

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
    clock: Arc<dyn Clock>,
    /// when the balloon went up; `None` before it does and after it was closed or timed out.
    balloon_since: Option<Duration>,
    /// when the pointer was last over the shield, for the tooltip's fade.
    tip_seen: Option<Duration>,
    tip_over: bool,
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
            clock: Arc::new(RealClock(Instant::now())),
            balloon_since: None,
            tip_seen: None,
            tip_over: false,
            balloon_done: false,
            snoozed: false,
            dialog: false,
        }
    }

    /// the same notice measuring time with `clock`, for tests.
    #[cfg(test)]
    fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// how opaque the balloon is now; `None` when it is not up.
    fn balloon_alpha(&self) -> Option<f32> {
        let since = self.balloon_since?;
        balloon_opacity(self.clock.now().saturating_sub(since))
    }

    /// how opaque the tray tooltip is now; 0 when it was never shown.
    fn tip_alpha(&self) -> f32 {
        let Some(seen) = self.tip_seen else {
            return 0.0;
        };
        if self.tip_over {
            return 1.0;
        }
        tip_opacity(Some(self.clock.now().saturating_sub(seen)))
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
                        self.balloon_since = Some(self.clock.now());
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

    /// the shield beside the clock; true when it was clicked. its tooltip fades out when the
    /// pointer leaves.
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
        self.tip_over = response.hovered();
        if self.tip_over {
            self.tip_seen = Some(self.clock.now());
        }
        let alpha = self.tip_alpha();
        if alpha > 0.0 {
            self.tooltip(ui.ctx(), rect, alpha);
            if alpha < 1.0 {
                ui.ctx().request_repaint_after(Duration::from_millis(30));
            }
        }
        response.clicked()
    }

    /// the small yellow tip above the shield, drawn at `alpha`.
    fn tooltip(&self, ctx: &egui::Context, shield: egui::Rect, alpha: f32) {
        Area::new(Id::unique("update-tip"))
            .order(Order::Tooltip)
            .pivot(Align2::RIGHT_BOTTOM)
            .fixed_pos(shield.right_top() + Vec2::new(0.0, -4.0))
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_opacity(alpha);
                let framed = Frame::new()
                    .fill(TIP)
                    .inner_margin(Margin::symmetric(6, 3))
                    .show(ui, |ui| {
                        ui.colored_label(BLACK, "Updates are available for LANlord");
                    });
                ui.painter().rect_stroke(
                    framed.response.rect,
                    0.0,
                    egui::Stroke::new(1.0, BLACK),
                    egui::StrokeKind::Inside,
                );
                #[cfg(test)]
                theme::probe::record(ui, "shield-tip", framed.response.rect, true);
            });
    }

    /// the balloon over the clock; true when it was clicked.
    pub fn balloon(&mut self, ctx: &egui::Context, taskbar: f32) -> bool {
        if self.balloon_since.is_none() {
            return false;
        }
        let Some(alpha) = self.balloon_alpha().filter(|_| self.visible()) else {
            self.balloon_since = None;
            self.balloon_done = true;
            return false;
        };
        // keep drawing while it fades, and come back for the moment it starts to
        ctx.request_repaint_after(Duration::from_millis(if alpha < 1.0 { 30 } else { 250 }));
        let mut clicked = false;
        let mut closed = false;
        Area::new(Id::unique("update-balloon"))
            .order(Order::Tooltip)
            .anchor(Align2::RIGHT_BOTTOM, Vec2::new(-8.0, -(taskbar + 10.0)))
            .show(ctx, |ui| {
                ui.set_opacity(alpha);
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
                if close.hovered() {
                    ui.painter().rect_stroke(
                        mark,
                        0.0,
                        egui::Stroke::new(1.0, Color32::from_rgb(128, 128, 128)),
                        egui::StrokeKind::Inside,
                    );
                }
                let c = mark.center();
                let stroke = egui::Stroke::new(1.5, BLACK);
                for (from, to) in [((-3.0, -3.0), (3.0, 3.0)), ((-3.0, 3.0), (3.0, -3.0))] {
                    ui.painter().line_segment(
                        [c + Vec2::new(from.0, from.1), c + Vec2::new(to.0, to.1)],
                        stroke,
                    );
                }
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

    /// a clock the tests move by hand.
    struct Fake(std::sync::atomic::AtomicU64);

    impl Fake {
        fn at(ms: u64) -> Arc<Self> {
            Arc::new(Self(std::sync::atomic::AtomicU64::new(ms)))
        }

        fn set(&self, ms: u64) {
            self.0.store(ms, std::sync::atomic::Ordering::SeqCst);
        }
    }

    impl Clock for Fake {
        fn now(&self) -> Duration {
            Duration::from_millis(self.0.load(std::sync::atomic::Ordering::SeqCst))
        }
    }

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
    fn the_balloon_is_whole_for_eight_seconds_then_fades_and_never_returns() {
        let clock = Fake::at(0);
        let mut updates = found().with_clock(clock.clone());
        let ctx = egui::Context::default();
        frame(&mut updates, &ctx, Vec::new());
        assert_eq!(updates.balloon_alpha(), Some(1.0));
        clock.set(7_900);
        assert_eq!(updates.balloon_alpha(), Some(1.0));
        // half way through the fade it is half there, and still drawn
        clock.set(8_300);
        let half = updates.balloon_alpha().unwrap();
        assert!((half - 0.5).abs() < 0.01, "{half}");
        frame(&mut updates, &ctx, Vec::new());
        assert!(updates.balloon_since.is_some());
        // after the fade it is gone for good
        clock.set(8_700);
        assert_eq!(updates.balloon_alpha(), None);
        frame(&mut updates, &ctx, Vec::new());
        assert!(updates.balloon_since.is_none() && updates.visible());
        updates.arrived(Some(pretend()));
        assert!(updates.balloon_since.is_none(), "it does not come back");
    }

    #[test]
    fn the_close_mark_dismisses_the_balloon_at_once_even_mid_fade() {
        let clock = Fake::at(0);
        let mut updates = found().with_clock(clock.clone());
        let ctx = egui::Context::default();
        clock.set(8_200);
        click(&mut updates, &ctx, "balloon-close");
        assert!(updates.balloon_since.is_none() && updates.balloon_done);
        assert!(updates.visible(), "the shield stays");
    }

    #[test]
    fn the_opacity_curves_are_whole_then_linear_then_gone() {
        assert_eq!(balloon_opacity(Duration::ZERO), Some(1.0));
        assert_eq!(balloon_opacity(BALLOON_FOR), Some(1.0));
        assert_eq!(balloon_opacity(BALLOON_FOR + BALLOON_FADE), None);
        assert_eq!(tip_opacity(None), 1.0);
        assert_eq!(tip_opacity(Some(TIP_FADE)), 0.0);
        let half = tip_opacity(Some(TIP_FADE / 2));
        assert!((half - 0.5).abs() < 0.01);
    }

    #[test]
    fn the_shield_tooltip_fades_out_after_the_pointer_leaves() {
        let clock = Fake::at(0);
        let mut updates = found().with_clock(clock.clone());
        let ctx = egui::Context::default();
        assert_eq!(updates.tip_alpha(), 0.0, "never shown before a hover");
        frame(&mut updates, &ctx, Vec::new());
        let (rect, _) = theme::probe::find(&ctx, "shield").expect("shield");
        // over the shield: the tip is whole
        frame(
            &mut updates,
            &ctx,
            vec![egui::Event::PointerMoved(rect.center())],
        );
        frame(&mut updates, &ctx, Vec::new());
        assert_eq!(updates.tip_alpha(), 1.0);
        assert!(theme::probe::find(&ctx, "shield-tip").is_some());
        // pointer leaves: it fades over a short time instead of popping away
        clock.set(100);
        frame(
            &mut updates,
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(900.0, 10.0))],
        );
        frame(&mut updates, &ctx, Vec::new());
        let fading = updates.tip_alpha();
        assert!(fading > 0.0 && fading < 1.0, "{fading}");
        clock.set(400);
        frame(&mut updates, &ctx, Vec::new());
        assert_eq!(updates.tip_alpha(), 0.0);
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
