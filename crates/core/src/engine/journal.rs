//! what a run keeps so a failed module can be put back: for each finished step an `Undo`, and the
//! earlier bytes of a file under `<state>/journal/<run>/`. a module is transactional as far as its
//! steps can be undone: on failure the finished steps are undone in reverse order, back to the
//! last step that cannot be (a replaced container, a created account), which stays as it is.

use anyhow::Result;

use super::host::Host;

/// how to take one finished step back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Undo {
    Nothing,
    /// put a file back: `saved` is the journal copy of the earlier bytes, `None` when the file
    /// did not exist before.
    File {
        path: String,
        saved: Option<String>,
        mode: u32,
    },
    /// this cannot be undone; the text says what stays.
    Irreversible(String),
}

#[derive(Debug)]
pub struct Journal {
    dir: String,
    next: usize,
    done: Vec<(String, Undo)>,
}

/// what a rollback did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Rollback {
    /// steps put back, newest first.
    pub undone: Vec<String>,
    /// the step that stopped the rollback, and what stays because of it.
    pub kept: Option<String>,
}

impl Journal {
    pub fn new(dir: String) -> Self {
        Self {
            dir,
            next: 0,
            done: Vec::new(),
        }
    }

    /// keep `bytes` (the earlier content of a file) and say where.
    pub fn save(&mut self, host: &dyn Host, bytes: &[u8]) -> Result<String> {
        host.mkdir(&self.dir, 0o700)?;
        self.next += 1;
        let path = format!("{}/{}", self.dir, self.next);
        host.write(&path, bytes, 0o600)?;
        Ok(path)
    }

    pub fn record(&mut self, step: &str, undo: Undo) {
        if undo != Undo::Nothing {
            self.done.push((step.to_string(), undo));
        }
    }

    /// whether something that cannot be undone has been done.
    pub fn has_irreversible(&self) -> bool {
        self.done
            .iter()
            .any(|(_, undo)| matches!(undo, Undo::Irreversible(_)))
    }

    /// undo the finished steps, newest first, until one cannot be undone.
    pub fn rollback(&mut self, host: &dyn Host) -> Rollback {
        let mut report = Rollback::default();
        while let Some((step, undo)) = self.done.pop() {
            match undo {
                Undo::Nothing => {}
                Undo::Irreversible(what) => {
                    report.kept = Some(format!("{step}: {what}"));
                    break;
                }
                Undo::File { path, saved, mode } => {
                    let put_back = match &saved {
                        Some(copy) => host
                            .read(copy)
                            .and_then(|bytes| {
                                bytes.ok_or_else(|| anyhow::anyhow!("the saved copy is gone"))
                            })
                            .and_then(|bytes| host.write(&path, &bytes, mode)),
                        None => host.remove(&path),
                    };
                    match put_back {
                        Ok(()) => report.undone.push(step),
                        Err(error) => {
                            report.kept = Some(format!("{step}: could not be put back ({error})"));
                            break;
                        }
                    }
                }
            }
        }
        report
    }

    /// the run went well: the copies are not needed.
    pub fn finish(&self, host: &dyn Host) {
        let _ = host.remove(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::host::FakeHost;

    #[test]
    fn undo_goes_back_in_reverse_and_stops_at_what_cannot_be_undone() {
        let host = FakeHost::new();
        host.put("/a", b"new-a", 0o644).put("/b", b"new-b", 0o644);
        let mut journal = Journal::new("/j/run1".into());
        let old_a = journal.save(&host, b"old-a").unwrap();
        journal.record(
            "a",
            Undo::File {
                path: "/a".into(),
                saved: Some(old_a),
                mode: 0o640,
            },
        );
        journal.record("container", Undo::Irreversible("replaced".into()));
        let old_b = journal.save(&host, b"old-b").unwrap();
        journal.record(
            "b",
            Undo::File {
                path: "/b".into(),
                saved: Some(old_b),
                mode: 0o640,
            },
        );
        let report = journal.rollback(&host);
        assert_eq!(report.undone, ["b"]);
        assert_eq!(report.kept.as_deref(), Some("container: replaced"));
        assert_eq!(host.text("/b").as_deref(), Some("old-b"));
        // the step before the container is consistent with it, so it stays
        assert_eq!(host.text("/a").as_deref(), Some("new-a"));
    }

    #[test]
    fn a_file_that_did_not_exist_is_removed() {
        let host = FakeHost::new();
        host.put("/new", b"x", 0o644);
        let mut journal = Journal::new("/j/run2".into());
        journal.record(
            "new",
            Undo::File {
                path: "/new".into(),
                saved: None,
                mode: 0o644,
            },
        );
        assert_eq!(journal.rollback(&host).undone, ["new"]);
        assert!(host.text("/new").is_none());
    }

    #[test]
    fn finishing_drops_the_copies() {
        let host = FakeHost::new();
        let mut journal = Journal::new("/j/run3".into());
        journal.save(&host, b"old").unwrap();
        assert!(host.text("/j/run3/1").is_some());
        journal.finish(&host);
        assert!(host.text("/j/run3/1").is_none());
    }
}
