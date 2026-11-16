//! background tasks: work runs on a plain thread so the window never blocks.
//! a task can ask for a sudo password; the ui shows a dialog and answers
//! through a channel, the same way the tui does.

use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use eframe::egui::Context;
use lanlord_core::bootstrap::AskPassword;
use lanlord_core::log::{emit, Kind};

/// a task asking the user for a password; the answer, or `None` when
/// cancelled, goes back through `reply`.
pub struct PasswordRequest {
    pub prompt: String,
    pub reply: Sender<Option<String>>,
}

/// the result of a task that is still running or has just finished.
pub struct Job<T> {
    result: Receiver<Result<T, String>>,
}

impl<T> Job<T> {
    /// the outcome once the task is done; the error is a message to show.
    pub fn poll(&self) -> Option<Result<T, String>> {
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("the task stopped unexpectedly".into())),
        }
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
}

impl Default for Tasks {
    fn default() -> Self {
        let (requests, asked) = mpsc::channel();
        Self {
            busy: Arc::default(),
            requests,
            asked,
        }
    }
}

impl Tasks {
    /// is a task running against `host`?
    pub fn busy(&self, host: &str) -> bool {
        self.busy.lock().is_ok_and(|hosts| hosts.contains(host))
    }

    /// the next password a task is waiting for, if any.
    pub fn next_request(&self) -> Option<PasswordRequest> {
        self.asked.try_recv().ok()
    }

    /// run a small read on its own thread. it marks no host busy and never asks for a
    /// password, so it cannot hold up what you are doing (the tray clock uses it).
    pub fn quiet<T: Send + 'static>(
        &self,
        ctx: &Context,
        work: impl FnOnce() -> Result<T> + Send + 'static,
    ) -> Job<T> {
        let (done, result) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = done.send(work().map_err(|err| err.to_string()));
            ctx.request_repaint();
        });
        Job { result }
    }

    /// run `work` against `host` on its own thread; `None` when that host
    /// already has a task. failures are also written to the console.
    pub fn spawn<T: Send + 'static>(
        &self,
        ctx: &Context,
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
        let (requests, ctx) = (self.requests.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut ask = asker(requests, ctx.clone());
            let outcome = work(&mut ask).map_err(|err| err.to_string());
            if let Err(message) = &outcome {
                emit(Kind::Err, message.clone());
            }
            // free the host before the result shows up, so the ui sees both at once
            drop(guard);
            let _ = done.send(outcome);
            ctx.request_repaint();
        });
        Some(Job { result })
    }
}

/// a password prompt that asks through the ui.
fn asker(requests: Sender<PasswordRequest>, ctx: Context) -> impl FnMut(&str) -> Result<String> {
    move |what| {
        let (reply, answer) = mpsc::channel();
        requests
            .send(PasswordRequest {
                prompt: what.to_string(),
                reply,
            })
            .map_err(|_| anyhow!("the ui is gone"))?;
        ctx.request_repaint();
        answer
            .recv()
            .ok()
            .flatten()
            .ok_or_else(|| anyhow!("cancelled"))
    }
}

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
        let (tasks, ctx) = (Tasks::default(), Context::default());
        let job = tasks.spawn(&ctx, "a@b", |_| Ok(41 + 1)).unwrap();
        assert_eq!(wait(&job), Ok(42));
        assert!(!tasks.busy("a@b"));
    }

    #[test]
    fn only_one_task_runs_per_host() {
        let (tasks, ctx) = (Tasks::default(), Context::default());
        let (release, gate) = mpsc::channel::<()>();
        let first = tasks
            .spawn(&ctx, "a@b", move |_| Ok(gate.recv().is_ok()))
            .unwrap();
        assert!(tasks.busy("a@b"));
        assert!(tasks.spawn(&ctx, "a@b", |_| Ok(())).is_none());
        assert!(
            tasks.spawn(&ctx, "c@d", |_| Ok(())).is_some(),
            "other hosts are free"
        );
        assert!(first.poll().is_none(), "still working");
        release.send(()).unwrap();
        assert_eq!(wait(&first), Ok(true));
        assert!(!tasks.busy("a@b"));
    }

    #[test]
    fn failures_come_back_as_messages() {
        let (tasks, ctx) = (Tasks::default(), Context::default());
        let job = tasks
            .spawn::<()>(&ctx, "a@b", |_| Err(anyhow!("boom")))
            .unwrap();
        assert_eq!(wait(&job), Err("boom".to_string()));
        assert!(!tasks.busy("a@b"));
    }

    #[test]
    fn a_panicking_task_still_frees_the_host() {
        let (tasks, ctx) = (Tasks::default(), Context::default());
        let job = tasks
            .spawn::<()>(&ctx, "a@b", |_| panic!("task panic on purpose"))
            .unwrap();
        assert!(wait(&job).is_err());
        assert!(!tasks.busy("a@b"));
    }

    fn answer_when_asked(tasks: &Tasks, reply: Option<&str>) -> String {
        let start = Instant::now();
        loop {
            if let Some(request) = tasks.next_request() {
                request.reply.send(reply.map(String::from)).unwrap();
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
        let (tasks, ctx) = (Tasks::default(), Context::default());
        let job = tasks
            .spawn(&ctx, "a@b", |ask| ask("sudo password for me"))
            .unwrap();
        assert_eq!(
            answer_when_asked(&tasks, Some("hunter2")),
            "sudo password for me"
        );
        assert_eq!(wait(&job), Ok("hunter2".to_string()));
    }

    #[test]
    fn cancelling_the_password_cancels_the_task() {
        let (tasks, ctx) = (Tasks::default(), Context::default());
        let job = tasks.spawn(&ctx, "a@b", |ask| ask("pw")).unwrap();
        answer_when_asked(&tasks, None);
        assert_eq!(wait(&job), Err("cancelled".to_string()));
    }

    #[test]
    fn no_request_means_none() {
        assert!(Tasks::default().next_request().is_none());
    }
}
