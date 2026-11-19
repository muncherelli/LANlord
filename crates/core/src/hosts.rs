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

/// the file next to the hosts that holds the target last signed in to.
fn last_path(path: &Path) -> PathBuf {
    path.with_file_name("last")
}

/// the target last signed in to (`user@host`, or the this-server target), if one is kept.
pub fn load_last(path: &Path) -> Option<String> {
    let text = fs::read_to_string(last_path(path)).ok()?;
    let target = text.trim();
    (!target.is_empty()).then(|| target.to_string())
}

/// keep `target` as the last sign-in; it is not a list entry.
pub fn save_last(path: &Path, target: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    // a temp file next to it, then a rename: a crash never leaves half a line, and the file is
    // private from the first byte
    let last = last_path(path);
    let temp = last.with_file_name(format!(".last.{}", std::process::id()));
    let _ = fs::remove_file(&temp);
    let written = private_file()
        .write(true)
        .truncate(true)
        .open(&temp)
        .and_then(|mut file| file.write_all(format!("{target}\n").as_bytes()))
        .and_then(|()| fs::rename(&temp, &last));
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written
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
    fn the_last_sign_in_is_kept_apart_from_the_list() {
        let path = temp("last");
        assert_eq!(load_last(&path), None);
        save_last(&path, "me@gw").unwrap();
        save_last(&path, "me@this-server.localhost").unwrap();
        assert_eq!(
            load_last(&path).as_deref(),
            Some("me@this-server.localhost")
        );
        assert!(load(&path).is_empty());
        cleanup(&path);
    }

    #[cfg(unix)]
    #[test]
    fn the_last_sign_in_is_replaced_whole_privately_and_leaves_nothing_behind() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp("last-private");
        save_last(&path, "me@gw").unwrap();
        save_last(&path, "me@other").unwrap();
        let last = last_path(&path);
        assert_eq!(fs::read_to_string(&last).unwrap(), "me@other\n");
        assert_eq!(
            fs::metadata(&last).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let names: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().all(|n| !n.starts_with(".last.")), "{names:?}");
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
