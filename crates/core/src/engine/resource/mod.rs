//! the things the engine changes. each one looks (`check`), says what differs (`diff`) and
//! changes it (`apply`), and hands back how to undo that.

use std::collections::BTreeMap;

use anyhow::Result;
use lanlord_model::engine::ChangeView;

use super::host::Host;
use super::journal::{Journal, Undo};
use super::Events;

pub mod container;
pub mod dir;
pub mod file;

pub use container::Container;
pub use dir::Dir;
pub use file::File;

/// what `check` saw.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Observed {
    pub exists: bool,
    pub content: Option<Vec<u8>>,
    pub mode: Option<u32>,
    /// anything else a resource noted, by name.
    pub facts: BTreeMap<String, String>,
}

/// a difference to remove: how it shows to a person, and what was seen when it was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub view: ChangeView,
    pub observed: Observed,
}

/// the module cannot be done by the engine; the playbooks do it instead. the message says why.
#[derive(Debug)]
pub struct Unsupported(pub String);

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Unsupported {}

pub trait Resource {
    /// what it is, for the plan, the events and the journal: "file /opt/lanlord/nt4/config/smb.conf".
    fn id(&self) -> String;
    /// look at the machine. nothing is changed.
    fn check(&self, host: &dyn Host) -> Result<Observed>;
    /// what differs from what is wanted, or `None`. `changed` holds the ids of the earlier
    /// resources that have a change, for one that follows another (a container follows its
    /// configuration file).
    fn diff(&self, observed: &Observed, changed: &[String]) -> Option<Change>;
    /// make the change. the journal keeps what an undo needs.
    fn apply(
        &self,
        host: &dyn Host,
        change: &Change,
        journal: &mut Journal,
        events: &mut Events,
    ) -> Result<Undo>;
}

/// a unified diff of two texts, small enough to show. `None` for a file that is not text.
pub fn unified(old: Option<&[u8]>, new: &[u8], name: &str) -> Option<String> {
    let new = std::str::from_utf8(new).ok()?;
    let old = match old {
        Some(bytes) => std::str::from_utf8(bytes).ok()?,
        None => "",
    };
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    // longest common subsequence; files here are a few hundred lines at most
    let (n, m) = (old_lines.len(), new_lines.len());
    if n * m > 4_000_000 {
        return Some(format!("--- {name}\n+++ {name}\n(too large to show)\n"));
    }
    let mut table = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if old_lines[i] == new_lines[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let mut out = format!("--- {name}\n+++ {name}\n");
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && old_lines[i] == new_lines[j] {
            i += 1;
            j += 1;
        } else if i < n && (j == m || table[i + 1][j] >= table[i][j + 1]) {
            out.push_str(&format!("-{}\n", old_lines[i]));
            i += 1;
        } else {
            out.push_str(&format!("+{}\n", new_lines[j]));
            j += 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diff_shows_only_what_changed() {
        let diff = unified(Some(b"a\nb\nc\n"), b"a\nx\nc\n", "f").unwrap();
        assert_eq!(diff, "--- f\n+++ f\n-b\n+x\n");
        assert_eq!(unified(None, b"a\n", "f").unwrap(), "--- f\n+++ f\n+a\n");
        assert!(unified(None, &[0xff, 0xfe], "f").is_none());
    }
}
