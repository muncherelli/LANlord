//! a directory with a mode.

use anyhow::Result;
use lanlord_model::engine::{Action, ChangeView};

use super::{Change, Observed, Resource};
use crate::engine::host::Host;
use crate::engine::journal::{Journal, Undo};
use crate::engine::Events;

pub struct Dir {
    pub path: String,
    pub mode: u32,
}

impl Dir {
    pub fn new(path: &str, mode: u32) -> Self {
        Self {
            path: path.into(),
            mode,
        }
    }
}

impl Resource for Dir {
    fn id(&self) -> String {
        format!("dir {}", self.path)
    }

    fn check(&self, host: &dyn Host) -> Result<Observed> {
        let stat = host.stat(&self.path)?;
        Ok(Observed {
            // a file where a directory belongs is not a directory: it is replaced by the apply's
            // error, never silently
            exists: stat.is_some_and(|s| s.is_dir),
            mode: stat.map(|s| s.mode),
            ..Observed::default()
        })
    }

    fn diff(&self, observed: &Observed, _: &[String]) -> Option<Change> {
        if observed.exists && observed.mode == Some(self.mode) {
            return None;
        }
        let (action, summary) = if observed.exists {
            (
                Action::Update,
                format!("{} will get mode {:04o}", self.path, self.mode),
            )
        } else {
            (Action::Create, format!("{} will be created", self.path))
        };
        Some(Change {
            view: ChangeView {
                resource: self.id(),
                action,
                summary,
                diff: None,
                sensitive: false,
            },
            observed: observed.clone(),
        })
    }

    fn apply(&self, host: &dyn Host, _: &Change, _: &mut Journal, _: &mut Events) -> Result<Undo> {
        host.mkdir(&self.path, self.mode)?;
        // a directory is left in place on undo: it holds nothing yet, and may hold samba's data
        Ok(Undo::Nothing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::host::FakeHost;

    #[test]
    fn a_directory_is_made_and_then_left_alone() {
        let host = FakeHost::new();
        let dir = Dir::new("/opt/x/lib", 0o751);
        let change = dir.diff(&dir.check(&host).unwrap(), &[]).unwrap();
        assert_eq!(change.view.action, Action::Create);
        let mut journal = Journal::new("/j".into());
        dir.apply(&host, &change, &mut journal, &mut |_| {})
            .unwrap();
        assert!(dir.diff(&dir.check(&host).unwrap(), &[]).is_none());
    }

    #[test]
    fn a_wrong_mode_is_fixed() {
        let host = FakeHost::new();
        host.mkdir("/opt/x/lib", 0o755).unwrap();
        let dir = Dir::new("/opt/x/lib", 0o751);
        assert_eq!(
            dir.diff(&dir.check(&host).unwrap(), &[])
                .unwrap()
                .view
                .action,
            Action::Update
        );
    }
}
