//! background tasks: work runs off the ui thread so the window never blocks.
//! a task can ask for a sudo password; the front end shows a dialog and answers
//! through a channel.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};

use crate::ask::AskPassword;
use crate::log::{emit, Kind};
use anyhow::{anyhow, Result};
use zeroize::Zeroizing;

use crate::wake::Wake;

/// a task asking the user for a password; the answer, or `None` when
/// cancelled, goes back through `reply`.
pub struct PasswordRequest {
    pub prompt: String,
    /// the password, or `None` when it was cancelled; zeroized wherever it is dropped.
    pub reply: Sender<Option<Zeroizing<String>>>,
}

/// the result of a task that is still running or has just finished.
pub struct Job<T> {
    result: Receiver<Result<T, String>>,
    /// the count of the [`Tasks`] this came from, which goes up as an outcome is taken.
    finished: Arc<AtomicU64>,
}

impl<T> Job<T> {
    /// the outcome once the task is done; the error is a message to show.
    pub fn poll(&self) -> Option<Result<T, String>> {
        let outcome = match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("the task stopped unexpectedly".into())),
        };
        if outcome.is_some() {
            self.finished.fetch_add(1, Ordering::Relaxed);
        }
        outcome
    }
}

/// marks a host as busy until dropped, even if the task panics.
struct Busy {
    hosts: Arc<Mutex<HashSet<String>>>,
    host: String,
}

impl Drop for Busy {
    fn drop(&mut self) {
        if let Ok(mut hosts) = self.hosts.lock() {
            hosts.remove(&self.host);
        }
    }
}

/// runs tasks, one at a time per host.
pub struct Tasks {
    busy: Arc<Mutex<HashSet<String>>>,
    requests: Sender<PasswordRequest>,
    asked: Receiver<PasswordRequest>,
    /// how many outcomes have been taken from jobs.
    finished: Arc<AtomicU64>,
}

impl Default for Tasks {
    fn default() -> Self {
        let (requests, asked) = mpsc::channel();
        Self {
            busy: Arc::default(),
            requests,
            asked,
            finished: Arc::default(),
        }
    }
}

impl Tasks {
    /// how many task outcomes have been taken so far; it only grows, so a front end can tell that what it
    /// shows may have changed without asking every window.
    pub fn finished(&self) -> u64 {
        self.finished.load(Ordering::Relaxed)
    }

    /// is a task running against `host`?
    pub fn busy(&self, host: &str) -> bool {
        self.busy.lock().is_ok_and(|hosts| hosts.contains(host))
    }

    /// the next password a task is waiting for, if any.
    pub fn next_request(&self) -> Option<PasswordRequest> {
        self.asked.try_recv().ok()
    }

    /// run a small read in the background. it marks no host busy and never asks for a
    /// password, so it cannot hold up what you are doing (the tray clock uses it).
    pub fn quiet<T: Send + 'static>(
        &self,
        wake: &Wake,
        work: impl FnOnce() -> Result<T> + Send + 'static,
    ) -> Job<T> {
        let (done, result) = mpsc::channel();
        let wake = wake.clone();
        background(move || {
            let _ = done.send(work().map_err(|err| err.to_string()));
            wake.wake();
        });
        Job {
            result,
            finished: self.finished.clone(),
        }
    }

    /// run `work` against `host` in the background; `None` when that host
    /// already has a task. failures are also written to the console.
    pub fn spawn<T: Send + 'static>(
        &self,
        wake: &Wake,
        host: &str,
        work: impl FnOnce(AskPassword) -> Result<T> + Send + 'static,
    ) -> Option<Job<T>> {
        if !self.busy.lock().ok()?.insert(host.to_string()) {
            return None;
        }
        let guard = Busy {
            hosts: self.busy.clone(),
            host: host.to_string(),
        };
        let (done, result) = mpsc::channel();
        let (requests, wake) = (self.requests.clone(), wake.clone());
        background(move || {
            let mut ask = asker(requests, wake.clone());
            let outcome = work(&mut ask).map_err(|err| err.to_string());
            if let Err(message) = &outcome {
                emit(Kind::Err, message.clone());
            }
            // free the host before the result shows up, so the ui sees both at once
            drop(guard);
            let _ = done.send(outcome);
            wake.wake();
        });
        Some(Job {
            result,
            finished: self.finished.clone(),
        })
    }
}

/// the browser's tasks: the work is run in passes over http (see `browser`), and ends the same
/// way a thread's does, with the result on the job's channel and a wake.
#[cfg(target_arch = "wasm32")]
impl Tasks {
    /// like [`Tasks::spawn`], for a gateway whose ops go over http.
    pub fn spawn_web<T: 'static>(
        &self,
        wake: &Wake,
        host: &str,
        http: crate::gateway::http::Http,
        work: impl Fn(&dyn crate::gateway::Gateway, AskPassword) -> Result<T> + 'static,
    ) -> Option<Job<T>> {
        if !self.busy.lock().ok()?.insert(host.to_string()) {
            return None;
        }
        let guard = Busy {
            hosts: self.busy.clone(),
            host: host.to_string(),
        };
        let (done, result) = mpsc::channel();
        let (requests, wake) = (self.requests.clone(), wake.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let outcome = crate::browser::run(&http, Some(&requests), &wake, work).await;
            let outcome = outcome.map_err(|err| err.to_string());
            if let Err(message) = &outcome {
                emit(Kind::Err, message.clone());
            }
            drop(guard);
            let _ = done.send(outcome);
            wake.wake();
        });
        Some(Job {
            result,
            finished: self.finished.clone(),
        })
    }

    /// like [`Tasks::quiet`], for a gateway whose ops go over http.
    pub fn quiet_web<T: 'static>(
        &self,
        wake: &Wake,
        http: crate::gateway::http::Http,
        work: impl Fn(&dyn crate::gateway::Gateway) -> Result<T> + 'static,
    ) -> Job<T> {
        let (done, result) = mpsc::channel();
        let wake = wake.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let outcome = crate::browser::run(&http, None, &wake, |gateway, _| work(gateway)).await;
            let _ = done.send(outcome.map_err(|err| err.to_string()));
            wake.wake();
        });
        Job {
            result,
            finished: self.finished.clone(),
        }
    }
}

/// start `work` off the ui thread: a plain thread on the desktop.
#[cfg(not(target_arch = "wasm32"))]
fn background(work: impl FnOnce() + Send + 'static) {
    std::thread::spawn(work);
}

/// in the browser there are no threads: the work runs on the page's event loop once the
/// current frame is done. a task that talks to the server sends its op with
/// `gateway::http::Http::run`, which does not block.
#[cfg(target_arch = "wasm32")]
fn background(work: impl FnOnce() + Send + 'static) {
    wasm_bindgen_futures::spawn_local(async move { work() });
}

/// a password prompt that asks through the ui.
#[cfg(not(target_arch = "wasm32"))]
fn asker(requests: Sender<PasswordRequest>, wake: Wake) -> impl FnMut(&str) -> Result<String> {
    move |what| {
        let (reply, answer) = mpsc::channel();
        requests
            .send(PasswordRequest {
                prompt: what.to_string(),
                reply,
            })
            .map_err(|_| anyhow!("the ui is gone"))?;
        wake.wake();
        // core's prompt takes a plain string, so the only plain copy is made here
        answer
            .recv()
            .ok()
            .flatten()
            .map(|password| password.to_string())
            .ok_or_else(|| anyhow!("cancelled"))
    }
}

/// in the browser a task runs on the page's only thread, so waiting for the dialog would wait
/// forever. a prompt fails at once instead; the web ui asks for the password itself when the
/// server answers an op with `Reply::NeedsPassword` (see `gateway::http::Http::run`).
#[cfg(target_arch = "wasm32")]
fn asker(_requests: Sender<PasswordRequest>, _wake: Wake) -> impl FnMut(&str) -> Result<String> {
    |what: &str| Err(anyhow!("{PROMPT_IN_BROWSER} ({what})"))
}

/// why a task in the browser cannot ask for a password in the middle of its work.
pub const PROMPT_IN_BROWSER: &str =
    "the browser cannot stop a task to ask for a password; send the op again with it";

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn wait<T>(job: &Job<T>) -> Result<T, String> {
        let start = Instant::now();
        loop {
            if let Some(result) = job.poll() {
                return result;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "task never finished"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_task_returns_its_value_and_frees_the_host() {
        let (tasks, wake) = (Tasks::default(), Wake::none());
        let job = tasks.spawn(&wake, "a@b", |_| Ok(41 + 1)).unwrap();
        assert_eq!(wait(&job), Ok(42));
        assert!(!tasks.busy("a@b"));
    }

    #[test]
    fn the_count_of_finished_tasks_goes_up_as_outcomes_are_taken() {
        let (tasks, wake) = (Tasks::default(), Wake::none());
        assert_eq!(tasks.finished(), 0);
        let job = tasks.quiet(&wake, || Ok(1));
        assert_eq!(wait(&job), Ok(1));
        assert_eq!(tasks.finished(), 1);
    }

    #[test]
    fn only_one_task_runs_per_host() {
        let (tasks, wake) = (Tasks::default(), Wake::none());
        let (release, gate) = mpsc::channel::<()>();
        let first = tasks
            .spawn(&wake, "a@b", move |_| Ok(gate.recv().is_ok()))
            .unwrap();
        assert!(tasks.busy("a@b"));
        assert!(tasks.spawn(&wake, "a@b", |_| Ok(())).is_none());
        assert!(
            tasks.spawn(&wake, "c@d", |_| Ok(())).is_some(),
            "other hosts are free"
        );
        assert!(first.poll().is_none(), "still working");
        release.send(()).unwrap();
        assert_eq!(wait(&first), Ok(true));
        assert!(!tasks.busy("a@b"));
    }

    #[test]
    fn failures_come_back_as_messages() {
        let (tasks, wake) = (Tasks::default(), Wake::none());
        let job = tasks
            .spawn::<()>(&wake, "a@b", |_| Err(anyhow!("boom")))
            .unwrap();
        assert_eq!(wait(&job), Err("boom".to_string()));
        assert!(!tasks.busy("a@b"));
    }

    #[test]
    fn a_panicking_task_still_frees_the_host() {
        let (tasks, wake) = (Tasks::default(), Wake::none());
        let job = tasks
            .spawn::<()>(&wake, "a@b", |_| panic!("task panic on purpose"))
            .unwrap();
        assert!(wait(&job).is_err());
        assert!(!tasks.busy("a@b"));
    }

    fn answer_when_asked(tasks: &Tasks, reply: Option<&str>) -> String {
        let start = Instant::now();
        loop {
            if let Some(request) = tasks.next_request() {
                request
                    .reply
                    .send(reply.map(|r| Zeroizing::new(r.to_string())))
                    .unwrap();
                return request.prompt;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "no password asked"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_task_can_ask_for_a_password() {
        let (tasks, wake) = (Tasks::default(), Wake::none());
        let job = tasks
            .spawn(&wake, "a@b", |ask| ask("sudo password for me"))
            .unwrap();
        assert_eq!(
            answer_when_asked(&tasks, Some("hunter2")),
            "sudo password for me"
        );
        assert_eq!(wait(&job), Ok("hunter2".to_string()));
    }

    #[test]
    fn cancelling_the_password_cancels_the_task() {
        let (tasks, wake) = (Tasks::default(), Wake::none());
        let job = tasks.spawn(&wake, "a@b", |ask| ask("pw")).unwrap();
        answer_when_asked(&tasks, None);
        assert_eq!(wait(&job), Err("cancelled".to_string()));
    }

    #[test]
    fn no_request_means_none() {
        assert!(Tasks::default().next_request().is_none());
    }
}
