//! what the server can do for the components: the doctor's report, read once per server and
//! kept for every window that shows it (My Server's Compatibility tab, the Components wizard and
//! the taskbar alert). nothing reads it until something needs it, and a Re-check reads it again.

use std::sync::{Arc, Mutex, MutexGuard};

use lanlord_model::doctor::Report;

use crate::ask::never;
use crate::remote::Remote;
use crate::services::Services;

/// the cached report of the signed-in server; clones share it.
#[derive(Clone, Default)]
pub struct Compat(Arc<Mutex<Remote<Report>>>);

impl Compat {
    fn remote(&self) -> MutexGuard<'_, Remote<Report>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// pick up a finished read; the report of `host` once it is there.
    pub fn report(&self, host: &str) -> Option<Report> {
        let mut remote = self.remote();
        remote.poll();
        remote
            .belongs_to(host)
            .then(|| remote.data.clone())
            .flatten()
    }

    /// is a read going on?
    pub fn busy(&self) -> bool {
        let mut remote = self.remote();
        remote.poll();
        remote.busy()
    }

    /// the line about the last read: true when it is an error.
    pub fn note(&self) -> Option<(bool, String)> {
        self.remote().note.clone()
    }

    /// read the report of `host` in the background unless it is there already. it never
    /// asks for a password and holds nothing up: a server that needs one is read when somebody
    /// opens the Compatibility tab.
    pub fn background(&self, services: &Services, host: &str) {
        let mut remote = self.remote();
        remote.poll();
        if !remote.wants_load(host, false) {
            return;
        }
        let target = host.to_string();
        let job = services.quiet(move |gateway| gateway.doctor(&target, &mut never));
        remote.track(host, Some(job), "checking this server...", "", true);
    }

    /// read the report of `host` again, asking for the sudo password if the probe needs it.
    pub fn recheck(&self, services: &Services, host: &str) {
        let mut remote = self.remote();
        remote.poll();
        if remote.busy() {
            return;
        }
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| gateway.doctor(&target, ask));
        remote.track(host, job, "checking this server...", "checked", false);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::gateway::Fake;
    use crate::wake::Wake;

    const UNPRIVILEGED: &str =
        "virt_container=lxc\nvirt_vm=none\nuid_map=0 100000 65536\nnesting=no\nkeyctl=no\n";

    fn wait(compat: &Compat, host: &str) -> Option<Report> {
        let start = Instant::now();
        while compat.busy() || compat.report(host).is_none() {
            if start.elapsed() > Duration::from_secs(2) {
                return None;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        compat.report(host)
    }

    #[test]
    fn the_report_is_read_once_and_kept_for_every_window() {
        let fake = Arc::new(Fake::default());
        *fake.probe.lock().unwrap() = UNPRIVILEGED.into();
        let services = Services::new(fake.clone(), Wake::none());
        let compat = Compat::default();
        assert!(compat.report("me@gw").is_none());
        compat.background(&services, "me@gw");
        let report = wait(&compat, "me@gw").expect("the report");
        assert!(!lanlord_model::compat::blockers(&report, "dns").is_empty());
        compat.background(&services, "me@gw");
        assert!(compat.clone().report("me@gw").is_some(), "clones share it");
        assert_eq!(fake.calls(), ["doctor me@gw"], "read once");
        assert!(compat.report("other@gw").is_none(), "per server");
    }

    #[test]
    fn a_background_read_never_asks_and_a_recheck_does() {
        let fake = Arc::new(Fake::default());
        *fake.password_prompt.lock().unwrap() = Some("[sudo] password:".into());
        let services = Services::new(fake.clone(), Wake::none());
        let compat = Compat::default();
        compat.background(&services, "me@gw");
        let start = Instant::now();
        while compat.busy() {
            assert!(start.elapsed() < Duration::from_secs(2));
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(compat.report("me@gw").is_none());
        assert!(compat.note().is_some_and(|(error, _)| error));
        // not retried every frame
        compat.background(&services, "me@gw");
        assert_eq!(fake.calls().len(), 1);
    }

    #[test]
    fn a_recheck_reads_again() {
        let fake = Arc::new(Fake::default());
        let services = Services::new(fake.clone(), Wake::none());
        let compat = Compat::default();
        compat.recheck(&services, "me@gw");
        assert!(wait(&compat, "me@gw").is_some());
        *fake.probe.lock().unwrap() = UNPRIVILEGED.into();
        compat.recheck(&services, "me@gw");
        let start = Instant::now();
        loop {
            let report = compat.report("me@gw");
            if report.is_some_and(|r| !lanlord_model::compat::blockers(&r, "dns").is_empty()) {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(2));
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(fake.calls().len(), 2);
    }
}
