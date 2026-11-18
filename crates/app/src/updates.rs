//! the update notice without a window: whether the background check is on, what it found, the
//! updater that installs it and the state of the box that lists what is new. the gui adds its
//! balloon and tray tooltip on top of it, the tui its tray label.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lanlord_core::prefs;
use lanlord_core::update::{self, Runner, Update};
use lanlord_core::updater::{
    self, Clock, GitBackend, Pause, RealClock, State, Unsupported, Updater,
};

use crate::wake::Wake;

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

pub struct Notice {
    runner: Arc<dyn Runner>,
    enabled: bool,
    prefs_path: Option<PathBuf>,
    found: Option<Update>,
    pending: Option<Receiver<Option<Update>>>,
    last_check: Option<Instant>,
    /// the state of the update and the work behind it; the one source for icon, tip and buttons.
    pub updater: Updater,
    /// a state to show once for a screenshot (LANLORD_FAKE_UPDATE=<state>).
    demo: Option<State>,
    /// "remind me later": the shield hides until the next check finds something.
    snoozed: bool,
    /// the box listing what is new is open.
    pub dialog: bool,
    /// an update turned up that was not known before, and nobody was told yet.
    fresh: bool,
}

impl Notice {
    /// a notice that checks with `runner` when `enabled`, and keeps the switch in `prefs_path`.
    /// `beat` is called when the background work has something to say, so a window can redraw.
    pub fn new(
        runner: Arc<dyn Runner>,
        prefs_path: Option<PathBuf>,
        enabled: bool,
        clock: Arc<dyn Clock>,
        beat: update::Beat,
    ) -> Self {
        Self {
            updater: Updater::new(Arc::new(Unsupported), clock, watchdog(), beat),
            demo: demo_state(),
            runner,
            enabled,
            prefs_path,
            found: None,
            pending: None,
            last_check: None,
            snoozed: false,
            dialog: false,
            fresh: false,
        }
    }

    /// a notice that never checks: for tests and for lanlord running on the gateway.
    pub fn quiet() -> Self {
        Self::new(
            Arc::new(update::System),
            None,
            false,
            Arc::new(RealClock::new()),
            Arc::new(|| {}),
        )
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// turn the background check on or off and remember it.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.updater.gone();
            self.forget_if_idle();
        }
        if let Some(path) = &self.prefs_path {
            let mut prefs = prefs::load(path);
            prefs.check_updates = on;
            let _ = prefs::save(path, &prefs);
        }
    }

    /// nothing is going on and nothing is new: drop what was found and close its box.
    fn forget_if_idle(&mut self) {
        if matches!(self.updater.state(), State::Idle) {
            self.found = None;
            self.dialog = false;
        }
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

    /// open the box listing what is new; true when there was something to list.
    pub fn open_dialog(&mut self) -> bool {
        self.dialog = self.found.is_some();
        self.dialog
    }

    pub fn remind_later(&mut self) {
        self.snoozed = true;
        self.dialog = false;
    }

    /// look now, in the background; `wake` says when the answer is in.
    pub fn check_now(&mut self, wake: &Wake) {
        if self.pending.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let runner = self.runner.clone();
        let wake = wake.clone();
        std::thread::spawn(move || {
            let _ = tx.send(update::check(&*runner, true));
            wake.wake();
        });
        self.pending = Some(rx);
        self.last_check = Some(Instant::now());
    }

    /// once a frame: let the updater move on, collect a finished check and start the next one
    /// when it is time.
    pub fn tick(&mut self, wake: &Wake) {
        self.updater.poll();
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
            self.check_now(wake);
        }
    }

    /// is a check running?
    pub fn checking(&self) -> bool {
        self.pending.is_some()
    }

    /// what a check found.
    pub fn arrived(&mut self, found: Option<Update>) {
        match found {
            Some(update) => {
                if self.found.is_none() {
                    self.snoozed = false;
                    self.fresh = true;
                }
                self.found = Some(update);
                self.updater.found();
                if let Some(state) = self.demo.take() {
                    self.updater.demo(state);
                }
            }
            None => {
                self.updater.gone();
                self.forget_if_idle();
            }
        }
    }

    /// true once after an update that was not known before turned up.
    pub fn take_fresh(&mut self) -> bool {
        std::mem::take(&mut self.fresh)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use lanlord_core::prefs;
    use lanlord_core::update::{Runner, Source, Update};
    use lanlord_core::updater::RealClock;

    use super::Notice;
    use crate::wake::Wake;

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

    fn notice(enabled: bool) -> Notice {
        Notice::new(
            Arc::new(Behind),
            None,
            enabled,
            Arc::new(RealClock::new()),
            Arc::new(|| {}),
        )
    }

    fn news() -> Update {
        Update {
            source: Source::Git,
            headline: "1 new change on trunk".into(),
            lines: vec!["abc feat: something new".into()],
        }
    }

    #[test]
    fn a_quiet_notice_shows_nothing_and_never_checks() {
        let mut quiet = Notice::quiet();
        assert!(!quiet.enabled() && !quiet.visible() && !quiet.checking());
        quiet.tick(&Wake::none());
        assert!(!quiet.checking() && quiet.found().is_none());
        assert!(!quiet.open_dialog(), "nothing to list");
    }

    #[test]
    fn an_update_that_turns_up_shows_once_and_reminding_later_hides_it() {
        let mut notice = notice(true);
        notice.arrived(Some(news()));
        assert!(notice.visible() && notice.found().is_some());
        assert!(notice.take_fresh());
        assert!(!notice.take_fresh(), "told once");
        assert!(notice.open_dialog() && notice.dialog);
        notice.remind_later();
        assert!(!notice.visible() && !notice.dialog);
        // a later check that finds the same update does not bring it back
        notice.arrived(Some(news()));
        assert!(!notice.visible());
        // finding nothing forgets it
        notice.arrived(None);
        assert!(notice.found().is_none() && !notice.visible());
    }

    #[test]
    fn the_background_check_collects_its_answer_when_on_and_is_remembered_when_switched() {
        let dir = std::env::temp_dir().join(format!("lanlord-updates-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("prefs.toml");
        let mut notice = Notice::new(
            Arc::new(Behind),
            Some(file.clone()),
            true,
            Arc::new(RealClock::new()),
            Arc::new(|| {}),
        );
        notice.check_now(&Wake::none());
        assert!(notice.checking());
        let started = Instant::now();
        while notice.found().is_none() && started.elapsed() < Duration::from_secs(5) {
            notice.tick(&Wake::none());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            notice.found().is_some(),
            "the pretend git is one change behind"
        );
        notice.set_enabled(false);
        assert!(!prefs::load(&file).check_updates);
        notice.set_enabled(true);
        assert!(prefs::load(&file).check_updates);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
