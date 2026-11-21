//! the native apply engine: a module is a list of resources, each looked at, compared with what
//! is wanted and changed only when it differs. runs inside the root process that already holds the
//! gateway key (the operations service, or `sudo lanlord operate`), so it needs no ssh and no
//! playbook run. a module the engine cannot do reports `Unsupported` and the playbooks do it.

use anyhow::Result;
use lanlord_model::apply::{ApplyEvent, Stats, TaskStatus};
use lanlord_model::engine::ChangeView;
use zeroize::Zeroize;

pub mod collect;
pub mod host;
pub mod journal;
pub mod modules;
pub mod resource;
pub mod state;

use host::Host;
use journal::{Journal, Undo};
use resource::{Change, Resource};

pub use resource::Unsupported;

/// where the engine keeps its state and, during a run, the earlier bytes of the files it changes.
pub const STATE_DIR: &str = "/var/lib/lanlord-operations/engine";

/// where events go as the engine works.
pub type Events<'a> = dyn FnMut(ApplyEvent) + 'a;

/// a value that must never reach a log, an event, a plan or an error. it is shown as nothing, and
/// handed out only to be written to a child's standard input.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// `text` without any of the secrets in it.
pub fn scrub(text: &str, secrets: &[Secret]) -> String {
    let mut out = text.to_string();
    for secret in secrets {
        if !secret.0.is_empty() {
            out = out.replace(&secret.0, "***");
        }
    }
    out
}

/// what a run did, in the numbers the progress dialog sums up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub ok: u32,
    pub changed: u32,
    pub failed: u32,
}

impl Tally {
    pub fn stats(self) -> Stats {
        Stats {
            ok: self.ok,
            changed: self.changed,
            failed: self.failed,
            ..Stats::default()
        }
    }

    pub fn add(&mut self, other: Tally) {
        self.ok += other.ok;
        self.changed += other.changed;
        self.failed += other.failed;
    }
}

/// run one named step, with its events. `work` says whether it changed anything and what to show.
pub fn task(
    events: &mut Events,
    tally: &mut Tally,
    secrets: &[Secret],
    name: &str,
    work: impl FnOnce(&mut Events) -> Result<(bool, String)>,
) -> Result<()> {
    events(ApplyEvent::TaskStart { name: name.into() });
    match work(events) {
        Ok((changed, msg)) => {
            if changed {
                tally.changed += 1;
            } else {
                tally.ok += 1;
            }
            events(ApplyEvent::TaskDone {
                name: name.into(),
                status: TaskStatus::Ok,
                changed,
                msg: scrub(&msg, secrets),
            });
            Ok(())
        }
        Err(error) => {
            tally.failed += 1;
            let text = scrub(&format!("{error:#}"), secrets);
            events(ApplyEvent::TaskDone {
                name: name.into(),
                status: TaskStatus::Failed,
                changed: false,
                msg: text.clone(),
            });
            Err(anyhow::anyhow!(text))
        }
    }
}

/// look at every resource and say what differs, in order. nothing is changed.
pub fn look(host: &dyn Host, resources: &[Box<dyn Resource>]) -> Result<Vec<Option<Change>>> {
    let mut changes = Vec::new();
    let mut changed: Vec<String> = Vec::new();
    for resource in resources {
        let observed = resource.check(host)?;
        let change = resource.diff(&observed, &changed);
        if change.is_some() {
            changed.push(resource.id());
        }
        changes.push(change);
    }
    Ok(changes)
}

pub fn views(changes: &[Option<Change>]) -> Vec<ChangeView> {
    changes.iter().flatten().map(|c| c.view.clone()).collect()
}

/// change what differs, in order. on a failure the finished steps are put back as far as they can
/// be (see the journal) and the error says what was kept.
pub fn change(
    host: &dyn Host,
    resources: &[Box<dyn Resource>],
    changes: &[Option<Change>],
    journal: &mut Journal,
    events: &mut Events,
    tally: &mut Tally,
    secrets: &[Secret],
) -> Result<()> {
    let todo = changes.iter().flatten().count() as u32;
    if todo > 0 {
        events(ApplyEvent::Total { n: todo });
    }
    for (resource, change) in resources.iter().zip(changes) {
        let Some(change) = change else { continue };
        let id = resource.id();
        let done = task(events, tally, secrets, &id, |events| {
            let undo = resource.apply(host, change, journal, events)?;
            let note = match &undo {
                Undo::Irreversible(_) => change.view.summary.clone(),
                _ => String::new(),
            };
            journal.record(&id, undo);
            Ok((true, note))
        });
        if let Err(error) = done {
            let report = journal.rollback(host);
            for step in &report.undone {
                events(ApplyEvent::Log {
                    text: format!("put back: {step}"),
                });
            }
            let kept = match report.kept {
                Some(why) => format!(" (not put back: {why})"),
                None => String::new(),
            };
            return Err(anyhow::anyhow!("{error}{kept}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::host::FakeHost;
    use crate::engine::resource::{Dir, File};

    #[test]
    fn a_secret_is_never_shown() {
        let secret = Secret::new("hunter2");
        assert!(!format!("{secret:?}").contains("hunter2"));
        assert_eq!(
            scrub("bad password hunter2 given", &[secret]),
            "bad password *** given"
        );
    }

    #[test]
    fn resources_are_changed_in_order_and_a_failure_puts_the_earlier_ones_back() {
        struct Fails;
        impl Resource for Fails {
            fn id(&self) -> String {
                "fails".into()
            }
            fn check(&self, _: &dyn Host) -> Result<resource::Observed> {
                Ok(Default::default())
            }
            fn diff(&self, o: &resource::Observed, _: &[String]) -> Option<Change> {
                Some(Change {
                    view: ChangeView {
                        resource: "fails".into(),
                        action: lanlord_model::engine::Action::Run,
                        summary: "x".into(),
                        diff: None,
                        sensitive: false,
                    },
                    observed: o.clone(),
                })
            }
            fn apply(
                &self,
                _: &dyn Host,
                _: &Change,
                _: &mut Journal,
                _: &mut Events,
            ) -> Result<Undo> {
                anyhow::bail!("it broke with hunter2")
            }
        }
        let host = FakeHost::new();
        host.put("/a", b"old\n", 0o644);
        let resources: Vec<Box<dyn Resource>> = vec![
            Box::new(Dir::new("/d", 0o750)),
            Box::new(File::new("/a", "new\n", 0o644)),
            Box::new(Fails),
        ];
        let changes = look(&host, &resources).unwrap();
        assert_eq!(views(&changes).len(), 3);
        let mut seen = Vec::new();
        let mut tally = Tally::default();
        let error = change(
            &host,
            &resources,
            &changes,
            &mut Journal::new("/j".into()),
            &mut |e| seen.push(e),
            &mut tally,
            &[Secret::new("hunter2")],
        )
        .unwrap_err()
        .to_string();
        assert_eq!(host.text("/a").as_deref(), Some("old\n"));
        assert!(
            !error.contains("hunter2") && error.contains("***"),
            "{error}"
        );
        assert!(!format!("{seen:?}").contains("hunter2"));
        assert_eq!((tally.changed, tally.failed), (2, 1));
    }
}
