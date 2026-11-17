//! the update state machine. one pure function says which state follows which event; a small
//! machine adds the clock (backoff after losing the network, and a watchdog for work that goes
//! quiet); an updater runs the downloading and installing in a background job that can always
//! be stopped. everything a front end shows (icon, tooltip, buttons) comes from the state.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::update::{self, Beat, Cancel, Step, StepFault};

/// the time the machine measures with; a real clock in the app and a fake one in tests.
pub trait Clock: Send + Sync {
    /// time since some fixed start.
    fn now(&self) -> Duration;
}

pub struct RealClock(Instant);

impl RealClock {
    pub fn new() -> Self {
        Self(Instant::now())
    }
}

impl Default for RealClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for RealClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
}

/// how long downloading or installing may go without any output before it counts as stuck.
pub const WATCHDOG: Duration = Duration::from_secs(600);

pub use lanlord_model::update::{Pause, State};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Found,
    Gone,
    Start,
    Fetched,
    Built,
    Pause,
    Resume,
    Offline,
    Fail(String),
    Cancel,
    Watchdog,
}

/// an event that makes no sense in the state it arrived in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Illegal {
    pub from: State,
    pub event: Event,
}

impl std::fmt::Display for Illegal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} is not possible while {:?}", self.event, self.from)
    }
}

impl std::error::Error for Illegal {}

/// the next state, or why there is none. this is the whole table; nothing else decides.
pub fn transition(state: &State, event: &Event) -> Result<State, Illegal> {
    use Event as E;
    use State::{Available, Done, Downloading, Failed, Idle, Installing, Paused};
    let same = || Ok(state.clone());
    match (state, event) {
        (Idle, E::Found) => Ok(Available),
        (Idle, E::Gone | E::Cancel) => Ok(Idle),
        (Available, E::Found | E::Cancel) => Ok(Available),
        (Available, E::Gone) => Ok(Idle),
        (Available | Failed(_), E::Start) => Ok(Downloading),
        (Downloading, E::Fetched) => Ok(Installing),
        (Downloading, E::Pause) => Ok(Paused(Pause::Manual)),
        (Downloading, E::Offline) => Ok(Paused(Pause::Offline { attempt: 1 })),
        (Downloading | Installing, E::Fail(why)) => Ok(Failed(why.clone())),
        (Downloading | Installing, E::Watchdog) => {
            Ok(Failed("it stopped making progress".to_string()))
        }
        (Installing, E::Built) => Ok(Done),
        (Paused(_), E::Resume) => Ok(Downloading),
        (Paused(Pause::Offline { attempt }), E::Offline) => Ok(Paused(Pause::Offline {
            attempt: attempt + 1,
        })),
        (Paused(Pause::Offline { .. }), E::Pause) => Ok(Paused(Pause::Manual)),
        (Downloading | Installing | Paused(_) | Failed(_) | Done, E::Found) => same(),
        (Downloading | Installing | Paused(_) | Failed(_), E::Cancel) => Ok(Available),
        (Done, E::Cancel) => Ok(Done),
        (Failed(_), E::Gone) => Ok(Idle),
        (Done, E::Gone) => Ok(Done),
        _ => Err(Illegal {
            from: state.clone(),
            event: event.clone(),
        }),
    }
}

/// how long to wait before the `attempt`th retry after losing the network: 30 seconds, a
/// minute, two minutes, and so on up to five minutes.
pub fn backoff(attempt: u32) -> Duration {
    let secs = 30u64.saturating_mul(1u64 << attempt.saturating_sub(1).min(8));
    Duration::from_secs(secs.min(300))
}

/// what the machine needs done now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Due {
    Nothing,
    /// the wait after losing the network is over: try again.
    Resume,
    /// downloading or installing went quiet for too long.
    Stalled,
}

/// the state, plus the timers that go with it.
pub struct Machine {
    state: State,
    clock: Arc<dyn Clock>,
    watchdog: Duration,
    last_beat: Duration,
    resume_at: Option<Duration>,
    attempts: u32,
}

impl Machine {
    pub fn new(clock: Arc<dyn Clock>, watchdog: Duration) -> Self {
        let now = clock.now();
        Self {
            state: State::Idle,
            clock,
            watchdog,
            last_beat: now,
            resume_at: None,
            attempts: 0,
        }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// take `event`; an illegal one is refused and nothing changes.
    pub fn apply(&mut self, event: Event) -> Result<&State, Illegal> {
        let mut next = transition(&self.state, &event)?;
        let now = self.clock.now();
        if event == Event::Offline {
            self.attempts += 1;
            next = State::Paused(Pause::Offline {
                attempt: self.attempts,
            });
        }
        if matches!(
            event,
            Event::Fetched | Event::Start | Event::Built | Event::Cancel
        ) {
            self.attempts = 0;
        }
        if event != Event::Found {
            self.last_beat = now;
        }
        self.resume_at = match &next {
            State::Paused(Pause::Offline { attempt }) => Some(now + backoff(*attempt)),
            _ => None,
        };
        self.state = next;
        Ok(&self.state)
    }

    /// the work said something, so it is still alive.
    pub fn beat(&mut self) {
        if matches!(self.state, State::Downloading | State::Installing) {
            self.last_beat = self.clock.now();
        }
    }

    /// what is due now: a retry after losing the network, or a stuck job.
    pub fn due(&self) -> Due {
        let now = self.clock.now();
        match &self.state {
            State::Paused(Pause::Offline { .. }) if self.resume_at.is_some_and(|at| now >= at) => {
                Due::Resume
            }
            State::Downloading | State::Installing
                if now.saturating_sub(self.last_beat) >= self.watchdog =>
            {
                Due::Stalled
            }
            _ => Due::Nothing,
        }
    }

    /// how long until the next retry, while waiting for the network.
    pub fn retry_in(&self) -> Option<Duration> {
        let at = self.resume_at?;
        Some(at.saturating_sub(self.clock.now()))
    }

    /// set the state directly, for showing a state without doing the work.
    pub fn force(&mut self, state: State) {
        self.last_beat = self.clock.now();
        self.resume_at = match &state {
            State::Paused(Pause::Offline { attempt }) => Some(self.last_beat + backoff(*attempt)),
            _ => None,
        };
        self.state = state;
    }
}

/// why a piece of work did not finish.
#[derive(Debug, PartialEq, Eq)]
pub enum Fault {
    /// the network is gone.
    Offline(String),
    Failed(String),
    /// it was stopped on purpose.
    Cancelled,
}

/// the work behind the states.
pub trait Backend: Send + Sync + 'static {
    /// fetch what is new. call `beat` whenever there is progress.
    fn download(&self, cancel: &Cancel, beat: &Beat) -> Result<(), Fault>;
    /// build and check the new version.
    fn install(&self, cancel: &Cancel, beat: &Beat) -> Result<(), Fault>;
    /// has a stopped job left something half-finished that must be reported?
    fn problem(&self) -> Option<String>;
}

/// a backend for a build that cannot update itself from here.
pub struct Unsupported;

impl Backend for Unsupported {
    fn download(&self, _: &Cancel, _: &Beat) -> Result<(), Fault> {
        Err(Fault::Failed("this build cannot update itself".into()))
    }

    fn install(&self, _: &Cancel, _: &Beat) -> Result<(), Fault> {
        Err(Fault::Failed("this build cannot update itself".into()))
    }

    fn problem(&self) -> Option<String> {
        None
    }
}

/// updating a development build from its git checkout: a fast-forward pull, then the build.
pub struct GitBackend {
    pub repo: std::path::PathBuf,
    pub exe: std::path::PathBuf,
}

fn run(step: &Step, cancel: &Cancel, beat: &Beat) -> Result<(), Fault> {
    match update::run_step_with(step, cancel, beat) {
        Ok(()) => Ok(()),
        Err(StepFault::Cancelled) => Err(Fault::Cancelled),
        Err(StepFault::Failed { message, tail }) => {
            if update::is_network_error(&tail.join("\n")) {
                Err(Fault::Offline(message))
            } else {
                Err(Fault::Failed(format!("{message}{}", update::indent(&tail))))
            }
        }
    }
}

impl Backend for GitBackend {
    fn download(&self, cancel: &Cancel, beat: &Beat) -> Result<(), Fault> {
        update::preflight(&self.repo, &self.exe, true)
            .map_err(|e| Fault::Failed(format!("{e:#}")))?;
        let pull = update::rebuild_steps(&self.repo, &self.exe, true).remove(0);
        run(&pull, cancel, beat)
    }

    fn install(&self, cancel: &Cancel, beat: &Beat) -> Result<(), Fault> {
        for step in update::rebuild_steps(&self.repo, &self.exe, false) {
            run(&step, cancel, beat)?;
        }
        update::verify_built(&self.exe).map_err(|e| Fault::Failed(format!("{e:#}")))
    }

    fn problem(&self) -> Option<String> {
        update::checkout_problem(&self.repo).or_else(|| update::app_leftover(&self.exe))
    }
}

enum Msg {
    Beat,
    Fetched,
    Built,
    Fault(Fault),
}

struct Job {
    cancel: Cancel,
    rx: Receiver<Msg>,
    handle: JoinHandle<()>,
}

/// the state machine with its background work.
pub struct Updater {
    machine: Machine,
    backend: Arc<dyn Backend>,
    wake: Beat,
    job: Option<Job>,
}

impl Updater {
    /// `wake` is called when the background work has something to say, so a window can redraw.
    pub fn new(
        backend: Arc<dyn Backend>,
        clock: Arc<dyn Clock>,
        watchdog: Duration,
        wake: Beat,
    ) -> Self {
        Self {
            machine: Machine::new(clock, watchdog),
            backend,
            wake,
            job: None,
        }
    }

    pub fn state(&self) -> &State {
        self.machine.state()
    }

    pub fn retry_in(&self) -> Option<Duration> {
        self.machine.retry_in()
    }

    /// the work to do when an update is installed; only while nothing is running.
    pub fn set_backend(&mut self, backend: Arc<dyn Backend>) {
        if self.job.is_none() {
            self.backend = backend;
        }
    }

    /// an update turned up.
    pub fn found(&mut self) {
        let _ = self.machine.apply(Event::Found);
    }

    /// no update any more, as far as the last check could tell.
    pub fn gone(&mut self) {
        let _ = self.machine.apply(Event::Gone);
    }

    /// show `state` without doing any work, for looking at it.
    pub fn demo(&mut self, state: State) {
        self.machine.force(state);
    }

    pub fn start(&mut self) -> Result<(), Illegal> {
        self.machine.apply(Event::Start)?;
        self.spawn();
        Ok(())
    }

    pub fn resume(&mut self) -> Result<(), Illegal> {
        self.machine.apply(Event::Resume)?;
        self.spawn();
        Ok(())
    }

    /// pause: the work stops for now. a stopped job that left something half-finished is a
    /// failure, not a pause, so it is never continued silently.
    pub fn pause(&mut self) -> Result<(), Illegal> {
        transition(self.machine.state(), &Event::Pause)?;
        self.stop_or_fail(Event::Pause)
    }

    /// cancel whatever is going on and go back to "an update is available".
    pub fn cancel(&mut self) {
        let _ = self.stop_or_fail(Event::Cancel);
    }

    fn stop_or_fail(&mut self, event: Event) -> Result<(), Illegal> {
        if self.stop_job() {
            if let Some(problem) = self.backend.problem() {
                return self.machine.apply(Event::Fail(problem)).map(|_| ());
            }
        }
        self.machine.apply(event).map(|_| ())
    }

    /// stop the job and wait for it; true when there was one.
    fn stop_job(&mut self) -> bool {
        let Some(job) = self.job.take() else {
            return false;
        };
        job.cancel.cancel();
        let _ = job.handle.join();
        true
    }

    fn spawn(&mut self) {
        let cancel = Cancel::default();
        let (tx, rx) = mpsc::channel();
        let backend = Arc::clone(&self.backend);
        let wake = Arc::clone(&self.wake);
        let token = cancel.clone();
        let handle = std::thread::spawn(move || {
            let send = |msg: Msg| {
                let _ = tx.send(msg);
                wake();
            };
            let beat: Beat = {
                let tx = tx.clone();
                let wake = Arc::clone(&wake);
                Arc::new(move || {
                    let _ = tx.send(Msg::Beat);
                    wake();
                })
            };
            if let Err(fault) = backend.download(&token, &beat) {
                return send(Msg::Fault(fault));
            }
            if token.is_cancelled() {
                return;
            }
            send(Msg::Fetched);
            match backend.install(&token, &beat) {
                Ok(()) => send(Msg::Built),
                Err(fault) => send(Msg::Fault(fault)),
            }
        });
        self.job = Some(Job { cancel, rx, handle });
    }

    /// once a frame: take what the job said, then do what the clock says is due.
    pub fn poll(&mut self) {
        let mut finished = false;
        let mut messages = Vec::new();
        if let Some(job) = &self.job {
            loop {
                match job.rx.try_recv() {
                    Ok(msg) => messages.push(msg),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                }
            }
        }
        for msg in messages {
            match msg {
                Msg::Beat => self.machine.beat(),
                Msg::Fetched => {
                    let _ = self.machine.apply(Event::Fetched);
                }
                Msg::Built => {
                    let _ = self.machine.apply(Event::Built);
                }
                Msg::Fault(Fault::Offline(_)) => {
                    let _ = self.machine.apply(Event::Offline);
                }
                Msg::Fault(Fault::Failed(why)) => {
                    let why = match self.backend.problem() {
                        Some(problem) => format!("{why}\n{problem}"),
                        None => why,
                    };
                    let _ = self.machine.apply(Event::Fail(why));
                }
                Msg::Fault(Fault::Cancelled) => {}
            }
        }
        if finished {
            self.stop_job();
        }
        match self.machine.due() {
            Due::Nothing => {}
            Due::Resume => {
                self.stop_job();
                let _ = self.resume();
            }
            Due::Stalled => {
                self.stop_job();
                let _ = self.machine.apply(Event::Watchdog);
            }
        }
    }

    /// stop all background work and wait for it; safe to call more than once.
    pub fn shutdown(&mut self) {
        self.stop_job();
    }
}

impl Drop for Updater {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashSet, VecDeque};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Mutex;

    struct FakeClock(AtomicU64);

    impl FakeClock {
        fn new() -> Arc<Self> {
            Arc::new(Self(AtomicU64::new(0)))
        }

        fn advance(&self, secs: u64) {
            self.0.fetch_add(secs * 1000, Ordering::SeqCst);
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> Duration {
            Duration::from_millis(self.0.load(Ordering::SeqCst))
        }
    }

    fn fail(why: &str) -> Event {
        Event::Fail(why.to_string())
    }

    fn all_states() -> Vec<State> {
        vec![
            State::Idle,
            State::Available,
            State::Downloading,
            State::Installing,
            State::Paused(Pause::Manual),
            State::Paused(Pause::Offline { attempt: 2 }),
            State::Failed("x".into()),
            State::Done,
        ]
    }

    fn all_events() -> Vec<Event> {
        vec![
            Event::Found,
            Event::Gone,
            Event::Start,
            Event::Fetched,
            Event::Built,
            Event::Pause,
            Event::Resume,
            Event::Offline,
            fail("x"),
            Event::Cancel,
            Event::Watchdog,
        ]
    }

    #[test]
    fn the_transition_table_allows_exactly_these_moves() {
        use State::*;
        let off = Paused(Pause::Offline { attempt: 2 });
        let failed = Failed("x".into());
        let stalled = Failed("it stopped making progress".into());
        // (from, event, to): every other pair must be refused
        let table: Vec<(State, Event, State)> = vec![
            (Idle, Event::Found, Available),
            (Idle, Event::Gone, Idle),
            (Idle, Event::Cancel, Idle),
            (Available, Event::Found, Available),
            (Available, Event::Gone, Idle),
            (Available, Event::Cancel, Available),
            (Available, Event::Start, Downloading),
            (Downloading, Event::Found, Downloading),
            (Downloading, Event::Fetched, Installing),
            (Downloading, Event::Pause, Paused(Pause::Manual)),
            (
                Downloading,
                Event::Offline,
                Paused(Pause::Offline { attempt: 1 }),
            ),
            (Downloading, fail("x"), failed.clone()),
            (Downloading, Event::Cancel, Available),
            (Downloading, Event::Watchdog, stalled.clone()),
            (Installing, Event::Found, Installing),
            (Installing, Event::Built, Done),
            (Installing, fail("x"), failed.clone()),
            (Installing, Event::Cancel, Available),
            (Installing, Event::Watchdog, stalled),
            (Paused(Pause::Manual), Event::Found, Paused(Pause::Manual)),
            (Paused(Pause::Manual), Event::Resume, Downloading),
            (Paused(Pause::Manual), Event::Cancel, Available),
            (off.clone(), Event::Found, off.clone()),
            (off.clone(), Event::Resume, Downloading),
            (
                off.clone(),
                Event::Offline,
                Paused(Pause::Offline { attempt: 3 }),
            ),
            (off.clone(), Event::Pause, Paused(Pause::Manual)),
            (off.clone(), Event::Cancel, Available),
            (failed.clone(), Event::Found, failed.clone()),
            (failed.clone(), Event::Gone, Idle),
            (failed.clone(), Event::Start, Downloading),
            (failed.clone(), Event::Cancel, Available),
            (Done, Event::Found, Done),
            (Done, Event::Gone, Done),
            (Done, Event::Cancel, Done),
        ];
        let legal: HashSet<(State_, Event_)> =
            table.iter().map(|(s, e, _)| (key(s), ekey(e))).collect();
        for (from, event, to) in &table {
            assert_eq!(
                transition(from, event).as_ref(),
                Ok(to),
                "{from:?} + {event:?}"
            );
        }
        for from in all_states() {
            for event in all_events() {
                if !legal.contains(&(key(&from), ekey(&event))) {
                    let err = transition(&from, &event).unwrap_err();
                    assert_eq!(
                        err,
                        Illegal {
                            from: from.clone(),
                            event: event.clone()
                        }
                    );
                    assert!(err.to_string().contains("not possible"));
                }
            }
        }
    }

    // the table above is keyed by the kind of state and event, not by their details
    type State_ = String;
    type Event_ = String;

    fn key(state: &State) -> State_ {
        format!("{state:?}")
    }

    fn ekey(event: &Event) -> Event_ {
        format!("{event:?}")
    }

    #[test]
    fn the_backoff_doubles_from_half_a_minute_up_to_five() {
        let secs: Vec<u64> = (1..=7).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, [30, 60, 120, 240, 300, 300, 300]);
        assert_eq!(backoff(0).as_secs(), 30);
        assert_eq!(backoff(u32::MAX).as_secs(), 300);
    }

    #[test]
    fn losing_the_network_waits_longer_each_time_and_resumes_when_due() {
        let clock = FakeClock::new();
        let mut m = Machine::new(clock.clone(), WATCHDOG);
        m.apply(Event::Found).unwrap();
        m.apply(Event::Start).unwrap();
        for (attempt, wait) in [(1u32, 30u64), (2, 60), (3, 120)] {
            m.apply(Event::Offline).unwrap();
            assert_eq!(m.state(), &State::Paused(Pause::Offline { attempt }));
            assert_eq!(m.retry_in(), Some(Duration::from_secs(wait)));
            clock.advance(wait - 1);
            assert_eq!(m.due(), Due::Nothing);
            clock.advance(1);
            assert_eq!(m.due(), Due::Resume);
            m.apply(Event::Resume).unwrap();
            assert_eq!(m.retry_in(), None);
        }
        // getting through resets the count
        m.apply(Event::Fetched).unwrap();
        m.apply(fail("x")).unwrap();
        m.apply(Event::Start).unwrap();
        m.apply(Event::Offline).unwrap();
        assert_eq!(m.state(), &State::Paused(Pause::Offline { attempt: 1 }));
    }

    #[test]
    fn a_manual_pause_never_resumes_by_itself() {
        let clock = FakeClock::new();
        let mut m = Machine::new(clock.clone(), WATCHDOG);
        m.apply(Event::Found).unwrap();
        m.apply(Event::Start).unwrap();
        m.apply(Event::Pause).unwrap();
        clock.advance(100_000);
        assert_eq!(m.due(), Due::Nothing);
        assert_eq!(m.retry_in(), None);
        // pausing while waiting for the network makes it manual too
        m.apply(Event::Resume).unwrap();
        m.apply(Event::Offline).unwrap();
        m.apply(Event::Pause).unwrap();
        clock.advance(100_000);
        assert_eq!(m.due(), Due::Nothing);
    }

    #[test]
    fn the_watchdog_only_watches_work_and_progress_resets_it() {
        let clock = FakeClock::new();
        let mut m = Machine::new(clock.clone(), Duration::from_secs(600));
        clock.advance(10_000);
        assert_eq!(m.due(), Due::Nothing, "idle has nothing to watch");
        m.apply(Event::Found).unwrap();
        assert_eq!(m.due(), Due::Nothing, "available has nothing to watch");
        m.apply(Event::Start).unwrap();
        clock.advance(599);
        assert_eq!(m.due(), Due::Nothing);
        m.beat();
        clock.advance(599);
        assert_eq!(m.due(), Due::Nothing, "output resets the watchdog");
        clock.advance(1);
        assert_eq!(m.due(), Due::Stalled);
        m.apply(Event::Watchdog).unwrap();
        assert!(matches!(m.state(), State::Failed(_)));
        clock.advance(10_000);
        assert_eq!(m.due(), Due::Nothing, "a failure is not watched");
        // the same goes for a pause
        m.apply(Event::Start).unwrap();
        m.apply(Event::Pause).unwrap();
        clock.advance(10_000);
        assert_eq!(m.due(), Due::Nothing);
    }

    #[test]
    fn an_illegal_event_is_refused_and_changes_nothing() {
        let clock = FakeClock::new();
        let mut m = Machine::new(clock.clone(), WATCHDOG);
        assert!(m.apply(Event::Start).is_err());
        assert!(m.apply(Event::Pause).is_err());
        assert_eq!(m.state(), &State::Idle);
        m.force(State::Paused(Pause::Offline { attempt: 4 }));
        assert_eq!(m.retry_in(), Some(Duration::from_secs(240)));
    }

    /// what the fake backend does on a call.
    #[derive(Clone, Copy)]
    enum Script {
        Ok,
        Offline,
        Failed,
        /// wait until stopped, beating each time round when `beats` is set.
        Block {
            beats: bool,
        },
    }

    struct Fake {
        download: Mutex<VecDeque<Script>>,
        install: Mutex<VecDeque<Script>>,
        problem: Mutex<Option<String>>,
        saw_cancel: AtomicBool,
        downloads: AtomicU64,
    }

    impl Fake {
        fn new(download: Vec<Script>, install: Vec<Script>) -> Arc<Self> {
            Arc::new(Self {
                download: Mutex::new(download.into()),
                install: Mutex::new(install.into()),
                problem: Mutex::new(None),
                saw_cancel: AtomicBool::new(false),
                downloads: AtomicU64::new(0),
            })
        }

        fn play(&self, script: Script, cancel: &Cancel, beat: &Beat) -> Result<(), Fault> {
            match script {
                Script::Ok => Ok(()),
                Script::Offline => Err(Fault::Offline("no network".into())),
                Script::Failed => Err(Fault::Failed("it broke".into())),
                Script::Block { beats } => {
                    while !cancel.is_cancelled() {
                        if beats {
                            beat();
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    self.saw_cancel.store(true, Ordering::SeqCst);
                    Err(Fault::Cancelled)
                }
            }
        }
    }

    impl Backend for Fake {
        fn download(&self, cancel: &Cancel, beat: &Beat) -> Result<(), Fault> {
            self.downloads.fetch_add(1, Ordering::SeqCst);
            let next = self
                .download
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Script::Ok);
            self.play(next, cancel, beat)
        }

        fn install(&self, cancel: &Cancel, beat: &Beat) -> Result<(), Fault> {
            let next = self
                .install
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Script::Ok);
            self.play(next, cancel, beat)
        }

        fn problem(&self) -> Option<String> {
            self.problem.lock().unwrap().clone()
        }
    }

    fn updater(fake: &Arc<Fake>, clock: &Arc<FakeClock>) -> Updater {
        let backend: Arc<dyn Backend> = fake.clone();
        let wake: Beat = Arc::new(|| {});
        let mut u = Updater::new(backend, clock.clone(), Duration::from_secs(600), wake);
        u.found();
        u
    }

    /// poll until the state is the one wanted; give up after two seconds.
    fn settle(u: &mut Updater, wanted: impl Fn(&State) -> bool) {
        for _ in 0..400 {
            u.poll();
            if wanted(u.state()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("stuck in {:?}", u.state());
    }

    #[test]
    fn a_whole_update_goes_from_available_to_done() {
        let (fake, clock) = (Fake::new(vec![], vec![]), FakeClock::new());
        let mut u = updater(&fake, &clock);
        assert_eq!(u.state(), &State::Available);
        u.start().unwrap();
        assert_eq!(u.state(), &State::Downloading);
        settle(&mut u, |s| *s == State::Done);
        assert!(u.start().is_err(), "nothing to start once it is done");
    }

    #[test]
    fn losing_the_network_pauses_and_resumes_by_itself_after_the_backoff() {
        let fake = Fake::new(vec![Script::Offline, Script::Offline, Script::Ok], vec![]);
        let clock = FakeClock::new();
        let mut u = updater(&fake, &clock);
        u.start().unwrap();
        settle(&mut u, |s| {
            matches!(s, State::Paused(Pause::Offline { attempt: 1 }))
        });
        assert_eq!(u.retry_in(), Some(Duration::from_secs(30)));
        u.poll();
        assert_eq!(fake.downloads.load(Ordering::SeqCst), 1, "it waits");
        clock.advance(30);
        settle(&mut u, |s| {
            matches!(s, State::Paused(Pause::Offline { attempt: 2 }))
        });
        assert_eq!(u.retry_in(), Some(Duration::from_secs(60)));
        clock.advance(60);
        settle(&mut u, |s| *s == State::Done);
        assert_eq!(fake.downloads.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn pausing_stops_the_work_and_resuming_starts_it_again() {
        let fake = Fake::new(vec![Script::Block { beats: true }], vec![]);
        let clock = FakeClock::new();
        let mut u = updater(&fake, &clock);
        u.start().unwrap();
        u.pause().unwrap();
        assert_eq!(u.state(), &State::Paused(Pause::Manual));
        assert!(
            fake.saw_cancel.load(Ordering::SeqCst),
            "the job was stopped"
        );
        clock.advance(10_000);
        u.poll();
        assert_eq!(
            u.state(),
            &State::Paused(Pause::Manual),
            "no timer resumes it"
        );
        u.resume().unwrap();
        settle(&mut u, |s| *s == State::Done);
        assert!(u.pause().is_err(), "nothing to pause once done");
    }

    #[test]
    fn cancel_works_in_every_state_and_leaves_an_update_available() {
        let clock = FakeClock::new();
        // idle: nothing happens
        let fake = Fake::new(vec![], vec![]);
        let backend: Arc<dyn Backend> = fake.clone();
        let mut idle = Updater::new(backend, clock.clone(), WATCHDOG, Arc::new(|| {}));
        idle.cancel();
        assert_eq!(idle.state(), &State::Idle);
        // available
        let mut u = updater(&fake, &clock);
        u.cancel();
        assert_eq!(u.state(), &State::Available);
        // downloading
        let fake = Fake::new(vec![Script::Block { beats: false }], vec![]);
        let mut u = updater(&fake, &clock);
        u.start().unwrap();
        u.cancel();
        assert_eq!(u.state(), &State::Available);
        assert!(fake.saw_cancel.load(Ordering::SeqCst));
        // installing
        let fake = Fake::new(vec![], vec![Script::Block { beats: false }]);
        let mut u = updater(&fake, &clock);
        u.start().unwrap();
        settle(&mut u, |s| *s == State::Installing);
        u.cancel();
        assert_eq!(u.state(), &State::Available);
        assert!(fake.saw_cancel.load(Ordering::SeqCst));
        // paused
        let mut u = updater(&Fake::new(vec![Script::Offline], vec![]), &clock);
        u.start().unwrap();
        settle(&mut u, |s| matches!(s, State::Paused(_)));
        u.cancel();
        assert_eq!(u.state(), &State::Available);
        // failed
        let mut u = updater(&Fake::new(vec![Script::Failed], vec![]), &clock);
        u.start().unwrap();
        settle(&mut u, |s| matches!(s, State::Failed(_)));
        u.cancel();
        assert_eq!(u.state(), &State::Available);
        // done stays done
        let mut u = updater(&Fake::new(vec![], vec![]), &clock);
        u.start().unwrap();
        settle(&mut u, |s| *s == State::Done);
        u.cancel();
        assert_eq!(u.state(), &State::Done);
    }

    #[test]
    fn a_cancel_that_leaves_something_half_finished_is_reported_not_hidden() {
        let fake = Fake::new(vec![Script::Block { beats: false }], vec![]);
        *fake.problem.lock().unwrap() = Some("index.lock was left behind".into());
        let clock = FakeClock::new();
        let mut u = updater(&fake, &clock);
        u.start().unwrap();
        u.cancel();
        match u.state() {
            State::Failed(why) => assert!(why.contains("index.lock"), "{why}"),
            other => panic!("{other:?}"),
        }
        // pausing does the same
        let fake = Fake::new(vec![Script::Block { beats: false }], vec![]);
        *fake.problem.lock().unwrap() = Some("a merge is half done".into());
        let mut u = updater(&fake, &clock);
        u.start().unwrap();
        u.pause().unwrap();
        assert!(matches!(u.state(), State::Failed(why) if why.contains("merge")));
    }

    #[test]
    fn a_failed_download_says_why_and_can_be_tried_again() {
        let fake = Fake::new(vec![Script::Failed, Script::Ok], vec![]);
        let clock = FakeClock::new();
        let mut u = updater(&fake, &clock);
        u.start().unwrap();
        settle(&mut u, |s| matches!(s, State::Failed(_)));
        assert!(matches!(u.state(), State::Failed(why) if why.contains("it broke")));
        u.start().unwrap();
        settle(&mut u, |s| *s == State::Done);
    }

    #[test]
    fn work_that_goes_quiet_for_too_long_fails_instead_of_hanging() {
        let fake = Fake::new(vec![Script::Block { beats: false }], vec![]);
        let clock = FakeClock::new();
        let mut u = updater(&fake, &clock);
        u.start().unwrap();
        clock.advance(599);
        u.poll();
        assert_eq!(u.state(), &State::Downloading);
        clock.advance(1);
        u.poll();
        assert!(matches!(u.state(), State::Failed(_)));
        assert!(
            fake.saw_cancel.load(Ordering::SeqCst),
            "the stuck job was stopped"
        );
    }

    #[test]
    fn output_keeps_slow_work_alive() {
        let fake = Fake::new(vec![Script::Block { beats: true }], vec![]);
        let clock = FakeClock::new();
        let mut u = updater(&fake, &clock);
        u.start().unwrap();
        for _ in 0..3 {
            std::thread::sleep(Duration::from_millis(30));
            clock.advance(400);
            u.poll();
        }
        assert_eq!(u.state(), &State::Downloading, "beats reset the watchdog");
        u.cancel();
    }

    #[test]
    fn shutdown_stops_and_joins_the_work_and_is_safe_twice() {
        let fake = Fake::new(vec![Script::Block { beats: false }], vec![]);
        let clock = FakeClock::new();
        let mut u = updater(&fake, &clock);
        u.start().unwrap();
        u.shutdown();
        assert!(fake.saw_cancel.load(Ordering::SeqCst));
        u.shutdown();
        // dropping does it too
        let fake = Fake::new(vec![Script::Block { beats: false }], vec![]);
        let mut u = updater(&fake, &clock);
        u.start().unwrap();
        drop(u);
        assert!(fake.saw_cancel.load(Ordering::SeqCst));
    }

    #[test]
    fn illegal_calls_are_refused_without_side_effects() {
        let fake = Fake::new(vec![], vec![]);
        let clock = FakeClock::new();
        let backend: Arc<dyn Backend> = fake.clone();
        let mut u = Updater::new(backend, clock, WATCHDOG, Arc::new(|| {}));
        assert!(u.start().is_err() && u.pause().is_err() && u.resume().is_err());
        assert_eq!(u.state(), &State::Idle);
        assert_eq!(fake.downloads.load(Ordering::SeqCst), 0);
        u.found();
        u.found();
        assert_eq!(u.state(), &State::Available);
        u.gone();
        assert_eq!(u.state(), &State::Idle);
        u.demo(State::Done);
        assert_eq!(u.state(), &State::Done);
    }

    #[test]
    fn an_updater_that_cannot_update_fails_clearly() {
        let backend: Arc<dyn Backend> = Arc::new(Unsupported);
        let clock = FakeClock::new();
        let mut u = Updater::new(backend, clock, WATCHDOG, Arc::new(|| {}));
        u.found();
        u.start().unwrap();
        settle(&mut u, |s| matches!(s, State::Failed(_)));
        assert!(matches!(u.state(), State::Failed(why) if why.contains("cannot update itself")));
        assert_eq!(Unsupported.problem(), None);
        u.set_backend(Fake::new(vec![], vec![]));
    }
}
