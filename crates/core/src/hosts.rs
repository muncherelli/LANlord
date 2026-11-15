//! the list of gateways the front ends offer, one `user@host` per line.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// ~/.config/lanlord/hosts
pub fn config_path() -> Option<PathBuf> {
    crate::paths::config_dir().map(|d| d.join("hosts"))
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

fn private_file() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    options.create(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
}

/// append `host` unless it is already listed.
pub fn add(path: &Path, host: &str) -> io::Result<()> {
    if load(path).iter().any(|h| h == host) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = private_file().append(true).open(path)?;
    writeln!(file, "{host}")
}

/// drop `host` from the file, keeping comments and the other hosts.
pub fn remove(path: &Path, host: &str) -> io::Result<()> {
    let Ok(text) = fs::read_to_string(path) else {
        return Ok(());
    };
    let kept: Vec<&str> = text.lines().filter(|l| l.trim() != host).collect();
    let mut body = kept.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    private_file()
        .write(true)
        .truncate(true)
        .open(path)?
        .write_all(body.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lanlord-core-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("lanlord").join("hosts")
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
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
        cleanup(&path);
    }

    #[test]
    fn load_skips_comments_and_blank_lines() {
        let path = temp("comments");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "# gateways\n\n  me@gw  \n").unwrap();
        assert_eq!(load(&path), ["me@gw"]);
        cleanup(&path);
    }

    #[test]
    fn remove_keeps_comments_and_other_hosts() {
        let path = temp("remove");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "# gateways\nme@gw\nme@other\n").unwrap();
        remove(&path, "me@gw").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "# gateways\nme@other\n");
        remove(&path, "me@other").unwrap();
        assert!(load(&path).is_empty());
        remove(&path, "me@nothing").unwrap();
        cleanup(&path);
    }

    #[test]
    fn removing_from_a_missing_file_is_fine() {
        remove(&temp("nofile"), "me@gw").unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp("mode");
        add(&path, "me@gw").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0);
        cleanup(&path);
    }

    #[test]
    fn the_default_path_is_under_the_config_dir() {
        if let Some(path) = config_path() {
            assert!(path.ends_with("lanlord/hosts"));
        }
    }
}
