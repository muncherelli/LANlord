//! rebuilding a development build from its checkout and starting it again, and restarting into
//! an update that was just installed. the work runs in the background; the front end asks once
//! a frame whether it answered, and quits when the new build is up.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use lanlord_core::update::{self, Grace};

use crate::log::{emit, Kind};
use crate::wake::Wake;

/// how the new build takes over from this one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Takeover {
    /// a window of its own opens beside this one, which closes when the new one is up (the gui).
    Window,
    /// the new build replaces this one in the terminal once this one has ended (the tui).
    Terminal,
}

/// what starting a rebuild came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Started {
    /// the build is running in the background.
    Running,
    /// nothing was started, and the console already says why.
    Ignored,
    /// the checkout or the tools were not there, so nothing was touched.
    Failed(String),
}

pub struct Rebuild {
    /// the checkout a development build rebuilds from.
    repo: Option<PathBuf>,
    /// the build that is running.
    exe: Option<PathBuf>,
    takeover: Takeover,
    /// how long a started build has to stay up before this one may quit.
    pub grace: Grace,
    /// a rebuild in progress; it answers once: `Ok` only when the new build is ready.
    running: Option<Receiver<Result<(), String>>>,
}

impl Rebuild {
    pub fn new(takeover: Takeover) -> Self {
        Self {
            repo: None,
            exe: None,
            takeover,
            grace: Grace::default(),
            running: None,
        }
    }

    /// this is a development build that can rebuild itself from `repo`.
    pub fn watch(&mut self, repo: Option<PathBuf>, exe: Option<PathBuf>) {
        self.repo = repo;
        self.exe = exe;
    }

    /// the checkout a development build rebuilds from; `None` in a release build.
    pub fn repo(&self) -> Option<&Path> {
        self.repo.as_deref()
    }

    /// the build that is running.
    pub fn exe(&self) -> Option<&Path> {
        self.exe.as_deref()
    }

    /// is a rebuild or a restart running?
    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    /// the words of the box asking before a rebuild.
    pub fn question(&self) -> Option<String> {
        let repo = self.repo.as_ref()?;
        let after = match self.takeover {
            Takeover::Window => "This window closes when the build works, and you sign in again.",
            Takeover::Terminal => "This ends when the build works, and the new build starts here.",
        };
        Some(format!(
            "Rebuild LANlord from {} and start it again?\n{after}",
            repo.display()
        ))
    }

    /// rebuild in the background, with the output in the console. nothing is touched before the
    /// checkout and the tools were looked for.
    pub fn start(&mut self, pull: bool, host: Option<&str>, wake: &Wake) -> Started {
        let (Some(repo), Some(exe)) = (self.repo.clone(), self.exe.clone()) else {
            emit(Kind::Err, "this build cannot be rebuilt from here");
            return Started::Ignored;
        };
        if self.busy() {
            return Started::Ignored;
        }
        if let Err(error) = update::preflight(&repo, &exe, pull) {
            return Started::Failed(format!("{error:#}"));
        }
        let (host, grace, takeover) = (host.map(String::from), self.grace, self.takeover);
        self.spawn(wake, move || match takeover {
            Takeover::Window => update::install_build(&repo, &exe, host.as_deref(), pull, grace),
            Takeover::Terminal => update::build(&repo, &exe, pull),
        });
        Started::Running
    }

    /// start the build that was just installed; it answers once it is up and still running.
    pub fn restart(&mut self, host: Option<&str>, wake: &Wake) -> bool {
        let Some(exe) = self.exe.clone() else {
            emit(Kind::Err, "this build cannot be restarted from here");
            return false;
        };
        if self.busy() {
            return false;
        }
        let (host, grace, takeover) = (host.map(String::from), self.grace, self.takeover);
        self.spawn(wake, move || {
            update::verify_built(&exe).and_then(|()| match takeover {
                Takeover::Window => update::relaunch(&exe, host.as_deref(), grace),
                Takeover::Terminal => Ok(()),
            })
        });
        true
    }

    fn spawn(&mut self, wake: &Wake, work: impl FnOnce() -> anyhow::Result<()> + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        let wake = wake.clone();
        std::thread::spawn(move || {
            let _ = tx.send(work().map_err(|e| format!("{e:#}")));
            wake.wake();
        });
        self.running = Some(rx);
    }

    /// once a frame: what the rebuild answered, when it did.
    pub fn poll(&mut self) -> Option<Result<(), String>> {
        let result = match self.running.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err("the rebuild stopped unexpectedly".into()),
        };
        self.running = None;
        Some(result)
    }

    /// how the new build takes over once this one is done.
    pub fn takeover(&self) -> Takeover {
        self.takeover
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Rebuild, Started, Takeover};
    use crate::wake::Wake;

    #[test]
    fn a_release_build_cannot_rebuild_or_restart() {
        let mut rebuild = Rebuild::new(Takeover::Terminal);
        assert!(rebuild.question().is_none() && rebuild.repo().is_none());
        assert_eq!(rebuild.start(false, None, &Wake::none()), Started::Ignored);
        assert!(!rebuild.restart(None, &Wake::none()));
        assert!(!rebuild.busy() && rebuild.poll().is_none());
    }

    #[test]
    fn a_missing_checkout_fails_before_anything_runs_and_says_how_it_ends() {
        let mut rebuild = Rebuild::new(Takeover::Window);
        rebuild.watch(
            Some("/no/such/checkout".into()),
            Some("/no/such/lanlord".into()),
        );
        assert!(rebuild.question().unwrap().contains("closes"));
        let Started::Failed(why) = rebuild.start(false, None, &Wake::none()) else {
            panic!("a missing checkout fails");
        };
        assert!(why.contains("is gone"), "{why}");
        assert!(!rebuild.busy());
        assert_eq!(rebuild.takeover(), Takeover::Window);
        assert_eq!(rebuild.exe(), Some(Path::new("/no/such/lanlord")));
    }

    #[test]
    fn a_restart_without_a_program_answers_with_why() {
        let mut rebuild = Rebuild::new(Takeover::Terminal);
        rebuild.watch(None, Some("/no/such/lanlord".into()));
        assert!(rebuild.restart(Some("ann@gateway.example"), &Wake::none()));
        assert!(rebuild.busy());
        assert!(!rebuild.restart(None, &Wake::none()), "one at a time");
        let answer = loop {
            if let Some(answer) = rebuild.poll() {
                break answer;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(answer.unwrap_err().contains("is missing"));
        assert!(!rebuild.busy());
    }
}
