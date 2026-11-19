//! the list of gateways the front ends offer, one `user@host` per line.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use lanlord_model::format::{render_text, Step};

use crate::localfile;

/// the steps that bring an older `hosts` or `last` file up to date: none yet. each file starts
/// with a version line; one without is version 1. a file from a newer lanlord reads as empty and
/// is never written over.
const STEPS: &[Step<String>] = &[];

/// ~/.config/lanlord/hosts
pub fn config_path() -> Option<PathBuf> {
    crate::paths::config_dir().map(|d| d.join("hosts"))
}

/// read the hosts, skipping blank lines and comments.
pub fn load(path: &Path) -> Vec<String> {
    localfile::read(path, STEPS)
        .ok()
        .flatten()
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
    // a file from a newer lanlord stops here, before anything is written
    let existing = localfile::read(path, STEPS)?;
    if existing
        .as_deref()
        .is_some_and(|text| text.lines().map(str::trim).any(|line| line == host))
    {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = private_file().append(true).open(path)?;
    if existing.is_none() {
        write!(file, "{}", render_text("", STEPS))?;
    }
    writeln!(file, "{host}")
}

/// the file next to the hosts that holds the target last signed in to.
fn last_path(path: &Path) -> PathBuf {
    path.with_file_name("last")
}

/// the target last signed in to (`user@host`, or the this-server target), if one is kept.
pub fn load_last(path: &Path) -> Option<String> {
    let text = localfile::read(&last_path(path), STEPS).ok()??;
    let target = text.trim();
    (!target.is_empty()).then(|| target.to_string())
}

/// keep `target` as the last sign-in; it is not a list entry.
pub fn save_last(path: &Path, target: &str) -> io::Result<()> {
    localfile::guard(&last_path(path), STEPS)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    localfile::write(
        &last_path(path),
        &render_text(&format!("{target}\n"), STEPS),
    )
}

/// drop `host` from the file, keeping comments and the other hosts.
pub fn remove(path: &Path, host: &str) -> io::Result<()> {
    let Some(text) = localfile::read(path, STEPS)? else {
        return Ok(());
    };
    let kept: Vec<&str> = text.lines().filter(|l| l.trim() != host).collect();
    let mut body = kept.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    let body = render_text(&body, STEPS);
    localfile::write(path, &body)
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
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!(
                "{}# gateways\nme@other\n",
                lanlord_model::format::text_header(1)
            )
        );
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
    #[test]
    fn a_hosts_file_from_a_newer_lanlord_is_refused_and_left_alone() {
        let path = temp("newer");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let newer = "# lanlord format 9, written by LANlord 7.4\nme@gw\n";
        fs::write(&path, newer).unwrap();
        for result in [add(&path, "me@other"), remove(&path, "me@gw")] {
            let error = result.unwrap_err().to_string();
            assert!(
                error.ends_with("written by LANlord 7.4 (format 9); upgrade LANlord to use it"),
                "{error}"
            );
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), newer);
        cleanup(&path);
    }

    #[test]
    fn a_last_file_from_a_newer_lanlord_is_refused_and_left_alone() {
        let path = temp("newer-last");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let last = path.parent().unwrap().join("last");
        let newer = "# lanlord format 9, written by LANlord 7.4\nme@gw\n";
        fs::write(&last, newer).unwrap();
        assert!(save_last(&last, "me@other").is_err());
        assert_eq!(fs::read_to_string(&last).unwrap(), newer);
        cleanup(&path);
    }
}
