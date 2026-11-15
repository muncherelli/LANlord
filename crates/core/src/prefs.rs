//! the client's own preferences, one `key=value` per line. nothing secret ever goes in here.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prefs {
    /// look for a newer lanlord in the background.
    pub check_updates: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            check_updates: true,
        }
    }
}

/// the preferences file inside the config directory.
pub fn path() -> Option<PathBuf> {
    crate::paths::config_dir().map(|d| d.join("prefs"))
}

/// read the preferences; a missing or odd file gives the defaults.
pub fn load(path: &Path) -> Prefs {
    let mut prefs = Prefs::default();
    for line in fs::read_to_string(path).unwrap_or_default().lines() {
        match line.split_once('=') {
            Some((key, value)) if key.trim() == "check_updates" => {
                prefs.check_updates = value.trim() != "false";
            }
            _ => {}
        }
    }
    prefs
}

pub fn save(path: &Path, prefs: &Prefs) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
        .open(path)?
        .write_all(format!("check_updates={}\n", prefs.check_updates).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_round_trip_and_default_on() {
        let dir = std::env::temp_dir().join(format!("lanlord-prefs-{}", std::process::id()));
        let file = dir.join("prefs");
        assert!(load(&file).check_updates, "no file means the defaults");
        save(
            &file,
            &Prefs {
                check_updates: false,
            },
        )
        .unwrap();
        assert!(!load(&file).check_updates);
        save(&file, &Prefs::default()).unwrap();
        assert!(load(&file).check_updates);
        fs::write(&file, "other=1\ncheck_updates = false\n").unwrap();
        assert!(!load(&file).check_updates);
        fs::write(&file, "nonsense").unwrap();
        assert!(load(&file).check_updates);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_path_is_inside_the_config_directory() {
        if let Some(path) = path() {
            assert!(path.ends_with("lanlord/prefs"));
        }
    }
}
