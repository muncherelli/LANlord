//! the client's own preferences, one `key=value` per line. nothing secret ever goes in here.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use lanlord_model::format::{render_text, Step};

use crate::localfile;

/// the steps that bring an older preferences file up to date: none yet. the file starts with a
/// version line; one without is version 1. a file from a newer lanlord reads as the defaults
/// and is never written over.
const STEPS: &[Step<String>] = &[];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prefs {
    /// look for a newer lanlord in the background.
    pub check_updates: bool,
    /// the admin accounts that switched the Welcome screen off at logon; everyone else, and
    /// every account on its first logon, gets it.
    pub welcome_off: BTreeSet<String>,
}

impl Prefs {
    /// whether `user` sees the Welcome screen after logon.
    pub fn welcome(&self, user: &str) -> bool {
        !self.welcome_off.contains(user)
    }

    /// switch the Welcome screen at logon on or off for `user`.
    pub fn set_welcome(&mut self, user: &str, on: bool) {
        if on {
            self.welcome_off.remove(user);
        } else {
            self.welcome_off.insert(user.to_string());
        }
    }
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            check_updates: true,
            welcome_off: BTreeSet::new(),
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
    let text = localfile::read(path, STEPS)
        .ok()
        .flatten()
        .unwrap_or_default();
    for line in text.lines() {
        match line.split_once('=') {
            Some((key, value)) if key.trim() == "check_updates" => {
                prefs.check_updates = value.trim() != "false";
            }
            Some((key, value)) if key.trim() == "welcome_off" && !value.trim().is_empty() => {
                prefs.welcome_off.insert(value.trim().to_string());
            }
            _ => {}
        }
    }
    prefs
}

pub fn save(path: &Path, prefs: &Prefs) -> io::Result<()> {
    localfile::guard(path, STEPS)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut text = format!("check_updates={}\n", prefs.check_updates);
    for user in &prefs.welcome_off {
        text.push_str(&format!("welcome_off={user}\n"));
    }
    options
        .open(path)?
        .write_all(render_text(&text, STEPS).as_bytes())
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
                ..Prefs::default()
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
    fn the_welcome_screen_is_remembered_per_account() {
        let dir = std::env::temp_dir().join(format!("lanlord-welcome-{}", std::process::id()));
        let file = dir.join("prefs");
        assert!(
            load(&file).welcome("dana"),
            "on by default, on the first logon"
        );
        let mut prefs = load(&file);
        prefs.set_welcome("dana", false);
        save(&file, &prefs).unwrap();
        let back = load(&file);
        assert!(!back.welcome("dana"));
        assert!(back.welcome("lee"));
        assert!(back.check_updates);
        let mut back = back;
        back.set_welcome("dana", true);
        save(&file, &back).unwrap();
        assert!(load(&file).welcome("dana"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_path_is_inside_the_config_directory() {
        if let Some(path) = path() {
            assert!(path.ends_with("lanlord/prefs"));
        }
    }
    #[test]
    fn preferences_from_a_newer_lanlord_are_refused_and_left_alone() {
        let dir = std::env::temp_dir().join(format!("lanlord-prefs-newer-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("prefs");
        let newer = "# lanlord format 9, written by LANlord 7.4\ncheck_updates = false\n";
        fs::write(&file, newer).unwrap();
        let error = save(&file, &Prefs::default()).unwrap_err().to_string();
        assert!(
            error.ends_with("written by LANlord 7.4 (format 9); upgrade LANlord to use it"),
            "{error}"
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), newer);
        let _ = fs::remove_dir_all(&dir);
    }
}
