//! the update notice: a shield beside the taskbar clock, a balloon the first time an update
//! turns up, and a small box listing what is new. the check runs in the background and says
//! nothing when it fails.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Area, Color32, Frame, Id, Margin, Order, Sense, Ui, Vec2};
use lanlord_core::prefs::{self, Prefs};
use lanlord_core::update::{self, Runner, Update};
use lanlord_core::updater::{
    self, Clock, GitBackend, Pause, RealClock, State, Unsupported, Updater,
};

use crate::icons;
#[cfg(test)]
use crate::theme;
use crate::theme::BLACK;
use crate::view;
use lanlord_app::update_box;

/// how long the balloon stays up in full before it starts to fade, and how long the fade takes.
const BALLOON_FOR: Duration = Duration::from_secs(8);
const BALLOON_FADE: Duration = Duration::from_millis(600);
/// how long each frame of the tray icon shows: about seven a second.
const FRAME: Duration = Duration::from_millis(140);
/// how long the tray tooltip takes to fade once the pointer has left the shield.
const TIP_FADE: Duration = Duration::from_millis(250);
/// the pale yellow of a classic tooltip.
const TIP: Color32 = Color32::from_rgb(255, 255, 225);

/// which frame of the tray icon shows `now` after the clock started, out of `count`.
fn frame_at(now: Duration, count: usize) -> usize {
    (now.as_millis() / FRAME.as_millis()) as usize % count
}

/// is it worth drawing the next frame? not when the window is minimised or hidden behind others.
fn awake(minimized: Option<bool>, occluded: Option<bool>) -> bool {
    minimized != Some(true) && occluded != Some(true)
}

/// does the tray icon move in this state? only while work is going on, and not with reduced motion.
fn animated(state: &State, reduce_motion: bool) -> bool {
    !reduce_motion && matches!(state, State::Downloading | State::Installing)
}

/// the picture for `state` at time `now`; `None` when there is nothing to show.
fn icon_for(state: &State, now: Duration, reduce_motion: bool) -> Option<&'static [&'static str]> {
    Some(match state {
        State::Idle => return None,
        State::Available => icons::update_frame(0),
        State::Downloading if reduce_motion => icons::update_frame(2),
        State::Downloading => icons::update_frame(frame_at(now, icons::UPDATE_FRAMES)),
        State::Installing if reduce_motion => icons::install_frame(3),
        State::Installing => icons::install_frame(frame_at(now, icons::INSTALL_FRAMES)),
        State::Paused(_) => icons::update_paused(),
        State::Failed(_) => icons::update_failed(),
        State::Done => icons::update_done(),
    })
}

/// what the tooltip says in `state`; `retry_in` is the wait left after losing the network.
fn tip_text(state: &State, retry_in: Option<Duration>) -> String {
    match state {
        State::Idle | State::Available => "Updates are available for LANlord".into(),
        State::Downloading => "Downloading the update...".into(),
        State::Installing => "Installing the update...".into(),
        State::Paused(Pause::Manual) => "The update is paused. Click to resume it.".into(),
        State::Paused(Pause::Offline { .. }) => {
            let wait =
                retry_in.map_or(String::new(), |d| format!(" (trying again in {})", span(d)));
            format!("Waiting for the network{wait}")
        }
        State::Failed(why) => format!("The update failed: {}", why.lines().next().unwrap_or("")),
        State::Done => "The update is installed. Restart LANlord to use it.".into(),
    }
}

/// a short wait, like "45 s" or "2 min".
fn span(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 90 {
        format!("{secs} s")
    } else {
        format!("{} min", secs.div_ceil(60))
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

/// how long work may stay quiet before it counts as stuck; LANLORD_UPDATE_WATCHDOG_SECS changes it.
fn watchdog() -> Duration {
    std::env::var("LANLORD_UPDATE_WATCHDOG_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(updater::WATCHDOG, Duration::from_secs)
}

/// the state LANLORD_FAKE_UPDATE asks to be shown, for looking at it without doing the work.
fn demo_state() -> Option<State> {
    Some(match std::env::var("LANLORD_FAKE_UPDATE").ok()?.as_str() {
        "downloading" => State::Downloading,
        "installing" => State::Installing,
        "paused" => State::Paused(Pause::Manual),
        "offline" => State::Paused(Pause::Offline { attempt: 2 }),
        "failed" => State::Failed("could not reach the update server (demo)".into()),
        "done" => State::Done,
        _ => return None,
    })
}

pub use lanlord_app::update_box::Choice;

pub struct Updates {
    runner: Arc<dyn Runner>,
    enabled: bool,
    prefs_path: Option<PathBuf>,
    found: Option<Update>,
    pending: Option<Receiver<Option<Update>>>,
    last_check: Option<Instant>,
    clock: Arc<dyn Clock>,
    /// the state of the update and the work behind it; the one source for icon, tip and buttons.
    updater: Updater,
    /// the window, once it exists, so the background work can ask for a redraw.
    window: Arc<OnceLock<egui::Context>>,
    /// show static pictures instead of moving ones (LANLORD_REDUCE_MOTION).
    reduce_motion: bool,
    /// a state to show once for a screenshot (LANLORD_FAKE_UPDATE=<state>).
    demo: Option<State>,
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
        Self::with_clock(runner, prefs_path, enabled, Arc::new(RealClock::new()))
    }

    /// the same, measuring time with `clock`.
    pub fn with_clock(
        runner: Arc<dyn Runner>,
        prefs_path: Option<PathBuf>,
        enabled: bool,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let window: Arc<OnceLock<egui::Context>> = Arc::new(OnceLock::new());
        let waker = Arc::clone(&window);
        let wake: update::Beat = Arc::new(move || {
            if let Some(ctx) = waker.get() {
                ctx.request_repaint();
            }
        });
        let updater = Updater::new(Arc::new(Unsupported), Arc::clone(&clock), watchdog(), wake);
        Self {
            updater,
            window,
            reduce_motion: std::env::var_os("LANLORD_REDUCE_MOTION").is_some(),
            demo: demo_state(),
            runner,
            enabled,
            prefs_path,
            found: None,
            pending: None,
            last_check: None,
            clock,
            balloon_since: None,
            tip_seen: None,
            tip_over: false,
            balloon_done: false,
            snoozed: false,
            dialog: false,
        }
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
            self.updater.gone();
            if matches!(self.updater.state(), State::Idle) {
                self.found = None;
                self.dialog = false;
            }
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
        match self.updater.state() {
            State::Idle => false,
            State::Available => !self.snoozed,
            _ => true,
        }
    }

    #[cfg(test)]
    pub fn state(&self) -> &State {
        self.updater.state()
    }

    /// what to do when an update is installed from here: a git checkout is rebuilt.
    pub fn watch_checkout(&mut self, repo: PathBuf, exe: PathBuf) {
        self.updater.set_backend(Arc::new(GitBackend { repo, exe }));
    }

    /// start installing; the work goes on in the background.
    pub fn install(&mut self) {
        self.snoozed = false;
        let _ = self.updater.start();
    }

    pub fn pause(&mut self) {
        let _ = self.updater.pause();
    }

    pub fn resume(&mut self) {
        let _ = self.updater.resume();
    }

    /// stop whatever is going on; the update stays available.
    pub fn cancel(&mut self) {
        self.updater.cancel();
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
        let _ = self.window.set(ctx.clone());
        self.updater.poll();
        // the one timer that is not an animation: the wait after losing the network
        let (minimized, occluded) = ctx.input(|i| (i.viewport().minimized, i.viewport().occluded));
        if let Some(wait) = self.updater.retry_in() {
            if awake(minimized, occluded) {
                ctx.request_repaint_after(wait + Duration::from_millis(50));
            }
        }
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
                self.updater.found();
                if let Some(state) = self.demo.take() {
                    self.updater.demo(state);
                }
            }
            None => {
                self.updater.gone();
                if matches!(self.updater.state(), State::Idle) {
                    self.found = None;
                    self.dialog = false;
                }
            }
        }
    }

    /// the shield beside the clock; true when it was clicked. its tooltip fades out when the
    /// pointer leaves.
    pub fn shield(&mut self, ui: &mut Ui) -> bool {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(22.0, 22.0), Sense::click());
        #[cfg(test)]
        theme::probe::record(ui, "shield", rect, true);
        // the picture is the state's, and it only moves while work is going on; the next frame
        // is asked for only then, and only when the window can be seen
        let state = self.updater.state().clone();
        if let Some(art) = icon_for(&state, self.clock.now(), self.reduce_motion) {
            icons::draw(
                ui.painter(),
                rect.left_top() + Vec2::new(3.0, 3.0),
                art,
                1.0,
            );
        }
        let (minimized, occluded) = ui
            .ctx()
            .input(|i| (i.viewport().minimized, i.viewport().occluded));
        if animated(&state, self.reduce_motion) && awake(minimized, occluded) {
            ui.ctx().request_repaint_after(FRAME);
        }
        self.tip_over = response.hovered();
        if self.tip_over {
            self.tip_seen = Some(self.clock.now());
        }
        let alpha = self.tip_alpha();
        if alpha > 0.0 {
            let text = tip_text(&state, self.updater.retry_in());
            self.tooltip(ui.ctx(), rect, alpha, &text);
            if alpha < 1.0 {
                ui.ctx().request_repaint_after(Duration::from_millis(30));
            }
        }
        response.clicked()
    }

    /// the small yellow tip above the shield, drawn at `alpha`.
    fn tooltip(&self, ctx: &egui::Context, shield: egui::Rect, alpha: f32, text: &str) {
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
                        ui.colored_label(BLACK, text);
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
        let state = self.updater.state().clone();
        let mut automatic = self.enabled;
        let mut choice = None;
        let dialog = update_box::view(&update, &state, automatic);
        match view::dialog(ctx, &dialog) {
            Some(update_box::Msg::Choose(c)) => choice = Some(c),
            Some(update_box::Msg::ToggleAutomatic) => automatic = !automatic,
            None => {}
        }
        if automatic != self.enabled {
            self.set_enabled(automatic);
        }
        match choice {
            Some(Choice::Later) => self.remind_later(),
            Some(Choice::Install | Choice::Restart) => self.dialog = false,
            _ => {}
        }
        choice
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanlord_app::update_box::{buttons_for, label, status_text};
    use lanlord_core::update::Source;
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

    /// the same, measuring time with a clock the test moves.
    fn found_with(clock: Arc<Fake>) -> Updates {
        let mut updates = Updates::with_clock(Arc::new(Behind), None, true, clock);
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
        let mut updates = found_with(clock.clone());
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
        let mut updates = found_with(clock.clone());
        let ctx = egui::Context::default();
        clock.set(8_200);
        click(&mut updates, &ctx, "balloon-close");
        assert!(updates.balloon_since.is_none() && updates.balloon_done);
        assert!(updates.visible(), "the shield stays");
    }

    #[test]
    fn the_tray_icon_turns_one_frame_every_fraction_of_a_second_and_wraps() {
        let n = icons::UPDATE_FRAMES;
        assert_eq!(frame_at(Duration::ZERO, n), 0);
        assert_eq!(frame_at(FRAME - Duration::from_millis(1), n), 0);
        assert_eq!(frame_at(FRAME, n), 1);
        assert_eq!(frame_at(FRAME * 3, n), 3);
        assert_eq!(frame_at(FRAME * n as u32, n), 0, "a full turn");
        assert_eq!(
            frame_at(FRAME * icons::INSTALL_FRAMES as u32, icons::INSTALL_FRAMES),
            0
        );
        // about seven frames a second
        assert!((6.0..=8.0).contains(&(1000.0 / FRAME.as_millis() as f32)));
    }

    #[test]
    fn nothing_is_scheduled_for_a_hidden_window() {
        assert!(awake(None, None));
        assert!(awake(Some(false), Some(false)));
        assert!(!awake(Some(true), None));
        assert!(!awake(None, Some(true)));
    }

    #[test]
    fn the_shield_draws_the_frame_the_clock_asks_for() {
        let clock = Fake::at(0);
        let mut updates = found_with(clock.clone());
        let ctx = egui::Context::default();
        // the shield draws without error on every frame of a full turn
        for i in 0..icons::UPDATE_FRAMES as u64 {
            clock.set(i * FRAME.as_millis() as u64);
            frame(&mut updates, &ctx, Vec::new());
            assert!(theme::probe::find(&ctx, "shield").is_some());
        }
    }

    #[test]
    fn each_state_has_its_own_picture_tooltip_and_buttons() {
        let offline = State::Paused(Pause::Offline { attempt: 2 });
        let failed = State::Failed("it broke\nmore".into());
        let states = [
            State::Available,
            State::Downloading,
            State::Installing,
            State::Paused(Pause::Manual),
            offline.clone(),
            failed.clone(),
            State::Done,
        ];
        assert_eq!(
            icon_for(&State::Idle, Duration::ZERO, false),
            None,
            "idle shows nothing"
        );
        // the pictures of the states that stand still differ from each other
        let stills: Vec<_> = [
            State::Available,
            State::Paused(Pause::Manual),
            failed.clone(),
            State::Done,
        ]
        .iter()
        .map(|s| icon_for(s, Duration::ZERO, false).unwrap())
        .collect();
        for (i, a) in stills.iter().enumerate() {
            for b in &stills[i + 1..] {
                assert_ne!(a, b);
            }
        }
        // the working states move, the others do not
        let at = |s: &State, ms: u64| icon_for(s, Duration::from_millis(ms), false).unwrap();
        assert_ne!(
            at(&State::Downloading, 0),
            at(&State::Downloading, FRAME.as_millis() as u64)
        );
        assert_ne!(
            at(&State::Installing, 0),
            at(&State::Installing, FRAME.as_millis() as u64)
        );
        assert_eq!(at(&State::Done, 0), at(&State::Done, 5_000));
        // every state says something different in its tooltip, and has buttons that make sense
        let tips: Vec<String> = states
            .iter()
            .map(|s| tip_text(s, Some(Duration::from_secs(45))))
            .collect();
        for (i, a) in tips.iter().enumerate() {
            assert!(!a.is_empty());
            for b in &tips[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert!(tip_text(&offline, Some(Duration::from_secs(45))).contains("45 s"));
        assert!(tip_text(&offline, Some(Duration::from_secs(150))).contains("3 min"));
        assert!(tip_text(&offline, None).contains("Waiting for the network"));
        assert!(
            tip_text(&failed, None).ends_with("it broke"),
            "only the first line"
        );
        assert_eq!(
            buttons_for(&State::Available),
            [Choice::Install, Choice::Later]
        );
        assert_eq!(
            buttons_for(&State::Downloading),
            [Choice::Pause, Choice::Cancel]
        );
        assert_eq!(
            buttons_for(&State::Installing),
            [Choice::Cancel],
            "no pause while installing"
        );
        assert_eq!(
            buttons_for(&State::Paused(Pause::Manual)),
            [Choice::Resume, Choice::Cancel]
        );
        assert_eq!(buttons_for(&offline), [Choice::Resume, Choice::Cancel]);
        assert_eq!(buttons_for(&failed), [Choice::Install, Choice::Cancel]);
        assert_eq!(buttons_for(&State::Done), [Choice::Restart, Choice::Later]);
        assert_eq!(label(Choice::Install, &failed), "Try again");
        assert_eq!(label(Choice::Cancel, &failed), "Dismiss");
        assert_eq!(label(Choice::Cancel, &State::Downloading), "Cancel");
        assert!(status_text(&failed).contains("it broke"));
        assert!(status_text(&State::Available).is_empty());
        for state in &states[1..] {
            assert!(!status_text(state).is_empty(), "{state:?}");
        }
    }

    #[test]
    fn only_working_states_animate_and_reduced_motion_stills_them() {
        for state in [
            State::Idle,
            State::Available,
            State::Paused(Pause::Manual),
            State::Done,
            State::Failed("x".into()),
        ] {
            assert!(!animated(&state, false), "{state:?}");
        }
        assert!(animated(&State::Downloading, false) && animated(&State::Installing, false));
        assert!(!animated(&State::Downloading, true) && !animated(&State::Installing, true));
        // with reduced motion the picture never changes, and differs from the plain "available" one
        let still = |s: &State, ms| icon_for(s, Duration::from_millis(ms), true).unwrap();
        assert_eq!(
            still(&State::Downloading, 0),
            still(&State::Downloading, 900)
        );
        assert_eq!(still(&State::Installing, 0), still(&State::Installing, 900));
        assert_ne!(still(&State::Downloading, 0), still(&State::Available, 0));
    }

    #[test]
    fn the_shield_follows_the_state_and_installing_runs_in_the_background() {
        let mut updates = found();
        let ctx = egui::Context::default();
        assert!(updates.visible() && *updates.state() == State::Available);
        // an updater that cannot update says why, and the shield stays for the failure
        updates.install();
        for _ in 0..200 {
            updates.tick(&ctx);
            if matches!(updates.state(), State::Failed(_)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(matches!(updates.state(), State::Failed(_)));
        assert!(updates.visible(), "a failure keeps the shield");
        // dismissing goes back to available, and a manual pause stays paused
        updates.cancel();
        assert_eq!(*updates.state(), State::Available);
        updates.pause();
        assert_eq!(*updates.state(), State::Available, "nothing to pause");
        // no update any more: the shield goes
        updates.arrived(None);
        assert!(!updates.visible() && updates.found().is_none());
    }

    #[test]
    fn a_finished_update_keeps_its_shield_after_the_next_check_finds_nothing() {
        let mut updates = found();
        updates.updater.demo(State::Done);
        updates.arrived(None);
        assert_eq!(*updates.state(), State::Done);
        assert!(updates.visible() && updates.found().is_some());
    }

    #[test]
    fn a_working_update_survives_a_check_that_finds_nothing() {
        // the pull moved the checkout, so the next check sees nothing new: the work goes on
        let mut updates = found();
        updates.updater.demo(State::Downloading);
        updates.arrived(None);
        assert_eq!(*updates.state(), State::Downloading);
        assert!(updates.found().is_some(), "the box still has its text");
    }

    #[test]
    fn the_dialog_shows_the_buttons_of_the_state() {
        let mut updates = found();
        let ctx = egui::Context::default();
        updates.open_dialog();
        let answer = |updates: &mut Updates, ctx: &egui::Context, name: &str| {
            for _ in 0..3 {
                frame(updates, ctx, Vec::new());
            }
            let (rect, _) = theme::probe::find(ctx, name).unwrap_or_else(|| panic!("{name}"));
            let mut got = None;
            for event in testkit::click(rect.center()) {
                got = got.or(frame(updates, ctx, vec![event]).2);
            }
            got
        };
        updates.updater.demo(State::Downloading);
        assert_eq!(answer(&mut updates, &ctx, "Pause"), Some(Choice::Pause));
        updates.updater.demo(State::Paused(Pause::Manual));
        updates.open_dialog();
        assert_eq!(answer(&mut updates, &ctx, "Resume"), Some(Choice::Resume));
        updates.updater.demo(State::Done);
        updates.open_dialog();
        assert_eq!(
            answer(&mut updates, &ctx, "Restart now"),
            Some(Choice::Restart)
        );
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
        let mut updates = found_with(clock.clone());
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
