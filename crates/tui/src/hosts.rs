//! the list of gateways the tui offers, one `user@host` per line.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// ~/.config/lanlord/hosts, honouring XDG_CONFIG_HOME and APPDATA.
pub fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(base.join("lanlord").join("hosts"))
}

/// read the hosts, skipping blank lines and comments.
pub fn load(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect()
}

/// append `host` unless it is already listed.
pub fn add(path: &Path, host: &str) -> io::Result<()> {
    if load(path).iter().any(|h| h == host) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{host}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lanlord-tui-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("lanlord").join("hosts")
    }

    #[test]
    fn missing_file_is_an_empty_list() {
        assert!(load(&temp("missing")).is_empty());
    }

    #[test]
    fn add_creates_the_file_and_skips_duplicates() {
        let path = temp("add");
        add(&path, "me@gw").unwrap();
        add(&path, "me@gw").unwrap();
        add(&path, "me@other").unwrap();
        assert_eq!(load(&path), ["me@gw", "me@other"]);
        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn load_skips_comments_and_blank_lines() {
        let path = temp("comments");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "# gateways\n\n  me@gw  \n").unwrap();
        assert_eq!(load(&path), ["me@gw"]);
        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }
}
