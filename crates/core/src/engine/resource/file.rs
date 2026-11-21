//! a file with exact content and mode.

use anyhow::Result;
use lanlord_model::engine::{Action, ChangeView};

use super::{unified, Change, Observed, Resource};
use crate::engine::host::Host;
use crate::engine::journal::{Journal, Undo};
use crate::engine::Events;

pub struct File {
    pub path: String,
    pub content: Vec<u8>,
    pub mode: u32,
    /// the content comes from a secret: the plan says the file changes, never how.
    pub sensitive: bool,
}

impl File {
    pub fn new(path: &str, content: impl Into<Vec<u8>>, mode: u32) -> Self {
        Self {
            path: path.into(),
            content: content.into(),
            mode,
            sensitive: false,
        }
    }
}

impl Resource for File {
    fn id(&self) -> String {
        format!("file {}", self.path)
    }

    fn check(&self, host: &dyn Host) -> Result<Observed> {
        let content = host.read(&self.path)?;
        let mode = host.stat(&self.path)?.map(|stat| stat.mode);
        Ok(Observed {
            exists: content.is_some(),
            content,
            mode,
            ..Observed::default()
        })
    }

    fn diff(&self, observed: &Observed, _: &[String]) -> Option<Change> {
        let same_content = observed.content.as_deref() == Some(self.content.as_slice());
        let same_mode = observed.mode == Some(self.mode);
        if same_content && same_mode {
            return None;
        }
        let (action, summary) = if !observed.exists {
            (Action::Create, format!("{} will be created", self.path))
        } else if same_content {
            (
                Action::Update,
                format!("{} will get mode {:04o}", self.path, self.mode),
            )
        } else {
            (Action::Update, format!("{} will change", self.path))
        };
        let diff = if self.sensitive || same_content {
            None
        } else {
            unified(observed.content.as_deref(), &self.content, &self.path)
        };
        Some(Change {
            view: ChangeView {
                resource: self.id(),
                action,
                summary,
                diff,
                sensitive: self.sensitive,
            },
            observed: observed.clone(),
        })
    }

    fn apply(
        &self,
        host: &dyn Host,
        change: &Change,
        journal: &mut Journal,
        _: &mut Events,
    ) -> Result<Undo> {
        let saved = match &change.observed.content {
            Some(old) => Some(journal.save(host, old)?),
            None => None,
        };
        host.write(&self.path, &self.content, self.mode)?;
        Ok(Undo::File {
            path: self.path.clone(),
            saved,
            mode: change.observed.mode.unwrap_or(self.mode),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::host::FakeHost;

    fn nobody(_: lanlord_model::apply::ApplyEvent) {}

    #[test]
    fn a_missing_file_is_created_and_undone_by_removing_it() {
        let host = FakeHost::new();
        let file = File::new("/x/conf", "hello\n", 0o640);
        let seen = file.check(&host).unwrap();
        let change = file.diff(&seen, &[]).unwrap();
        assert_eq!(change.view.action, Action::Create);
        let mut journal = Journal::new("/j".into());
        let undo = file
            .apply(&host, &change, &mut journal, &mut nobody)
            .unwrap();
        assert_eq!(host.text("/x/conf").as_deref(), Some("hello\n"));
        assert_eq!(host.stat("/x/conf").unwrap().unwrap().mode, 0o640);
        journal.record("f", undo);
        journal.rollback(&host);
        assert!(host.text("/x/conf").is_none());
    }

    #[test]
    fn the_same_content_and_mode_is_no_change() {
        let host = FakeHost::new();
        host.put("/x/conf", b"hello\n", 0o640);
        let file = File::new("/x/conf", "hello\n", 0o640);
        assert!(file.diff(&file.check(&host).unwrap(), &[]).is_none());
    }

    #[test]
    fn a_changed_file_shows_a_diff_unless_it_is_sensitive() {
        let host = FakeHost::new();
        host.put("/x/conf", b"a\n", 0o640);
        let mut file = File::new("/x/conf", "b\n", 0o640);
        let seen = file.check(&host).unwrap();
        let shown = file.diff(&seen, &[]).unwrap();
        assert_eq!(
            shown.view.diff.as_deref(),
            Some("--- /x/conf\n+++ /x/conf\n-a\n+b\n")
        );
        file.sensitive = true;
        let hidden = file.diff(&seen, &[]).unwrap();
        assert!(hidden.view.diff.is_none() && hidden.view.sensitive);
    }

    #[test]
    fn a_wrong_mode_alone_is_a_change() {
        let host = FakeHost::new();
        host.put("/x/conf", b"a\n", 0o600);
        let file = File::new("/x/conf", "a\n", 0o640);
        let change = file.diff(&file.check(&host).unwrap(), &[]).unwrap();
        assert!(change.view.diff.is_none());
        assert!(change.view.summary.contains("0640"));
    }
}
