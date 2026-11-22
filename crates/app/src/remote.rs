//! what a window knows about the gateway it shows: the data loaded from
//! it, the background job fetching or changing it, and the line of text
//! telling you how that went.

use crate::tasks::Job;

struct Pending<T> {
    host: String,
    job: Job<T>,
    done: String,
    load: bool,
}

pub struct Remote<T> {
    pub data: Option<T>,
    /// the host `data` came from.
    source: Option<String>,
    /// a host whose load failed, so it is not retried every frame.
    failed: Option<String>,
    pending: Option<Pending<T>>,
    /// a line for the window: true when it is an error.
    pub note: Option<(bool, String)>,
}

impl<T> Default for Remote<T> {
    fn default() -> Self {
        Self {
            data: None,
            source: None,
            failed: None,
            pending: None,
            note: None,
        }
    }
}

impl<T> Remote<T> {
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// is `data` the data of `host`?
    pub fn belongs_to(&self, host: &str) -> bool {
        self.source.as_deref() == Some(host)
    }

    /// the host `data` came from.
    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    /// pretend `data` came from `host`, for tests that skip the load job.
    #[cfg(test)]
    pub fn set_source(&mut self, host: &str) {
        self.source = Some(host.into());
    }

    /// should the window fetch `host` now? yes when it shows another host's
    /// data, nothing is going on and you have no edits to lose.
    pub fn wants_load(&self, host: &str, dirty: bool) -> bool {
        !self.belongs_to(host)
            && self.failed.as_deref() != Some(host)
            && self.pending.is_none()
            && !dirty
    }

    pub fn say(&mut self, error: bool, text: impl Into<String>) {
        self.note = Some((error, text.into()));
    }

    /// follow `job`, started against `host`; its result replaces `data` and
    /// `done` is shown. `None` means the host was busy.
    pub fn track(
        &mut self,
        host: &str,
        job: Option<Job<T>>,
        doing: &str,
        done: &str,
        load: bool,
    ) -> bool {
        match job {
            Some(job) => {
                if load {
                    self.failed = None;
                }
                self.say(false, doing);
                self.pending = Some(Pending {
                    host: host.into(),
                    job,
                    done: done.into(),
                    load,
                });
                true
            }
            None => {
                self.say(true, "the gateway is busy");
                false
            }
        }
    }

    /// pick up a finished job; true when new data arrived.
    pub fn poll(&mut self) -> bool {
        let Some(pending) = self.pending.take() else {
            return false;
        };
        match pending.job.poll() {
            None => {
                self.pending = Some(pending);
                false
            }
            Some(Ok(data)) => {
                self.data = Some(data);
                self.source = Some(pending.host);
                self.failed = None;
                self.say(false, pending.done);
                true
            }
            Some(Err(error)) => {
                if pending.load {
                    self.failed = Some(pending.host);
                }
                self.say(true, error);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::Fake;
    use crate::services::Services;
    use crate::wake::Wake;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn finish(remote: &mut Remote<u32>) -> bool {
        let start = Instant::now();
        loop {
            if remote.poll() || !remote.busy() {
                return remote.data.is_some();
            }
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn a_load_fills_the_data_and_a_failure_is_not_retried() {
        let services = Services::new(Arc::new(Fake::new()), Wake::none());
        let mut remote = Remote::<u32>::default();
        assert!(remote.wants_load("a", false) && !remote.wants_load("a", true));
        let job = services.spawn("a", |_, _| Ok(7));
        assert!(remote.track("a", job, "loading", "loaded", true));
        assert!(!remote.wants_load("a", false), "busy");
        assert!(finish(&mut remote));
        assert_eq!((remote.data, remote.belongs_to("a")), (Some(7), true));
        assert_eq!(remote.note, Some((false, "loaded".into())));
        assert!(remote.wants_load("b", false));

        let job = services.spawn("b", |_, _| anyhow::bail!("down"));
        remote.track("b", job, "loading", "loaded", true);
        finish(&mut remote);
        assert_eq!(remote.note, Some((true, "down".into())));
        assert!(
            !remote.wants_load("b", false),
            "failed hosts are not retried"
        );
        // a failed save does not block loading
        let job = services.spawn("a", |_, _| anyhow::bail!("no"));
        remote.track("a", job, "saving", "saved", false);
        finish(&mut remote);
        assert!(remote.belongs_to("a") && !remote.wants_load("a", false));
    }

    #[test]
    fn a_busy_host_is_reported() {
        let services = Services::new(Arc::new(Fake::new()), Wake::none());
        let (hold, release) = std::sync::mpsc::channel::<()>();
        let release = std::sync::Mutex::new(release);
        let first = services.spawn("a", move |_, _| {
            let _ = release.lock().unwrap().recv();
            Ok(1u32)
        });
        let mut remote = Remote::<u32>::default();
        let second = services.spawn("a", |_, _| Ok(2u32));
        assert!(!remote.track("a", second, "x", "y", false));
        assert_eq!(remote.note, Some((true, "the gateway is busy".into())));
        drop(hold);
        drop(first);
    }
}
