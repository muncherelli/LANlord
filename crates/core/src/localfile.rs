//! the small line-based files a client keeps (the recent servers, the last sign-in, the
//! preferences): each starts with a version line, and a file from a newer lanlord is refused and
//! never written over. see [`lanlord_model::format`] and docs/stored-formats.md.

use std::fs;
use std::io;
use std::path::Path;

use lanlord_model::format::{self, Step, TooNew};

fn refused(path: &Path, error: anyhow::Error) -> io::Error {
    let why = match error.downcast_ref::<TooNew>() {
        Some(too_new) => too_new.to_string(),
        None => format!("{error:#}"),
    };
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{}: {why}", path.display()),
    )
}

/// the file's text, brought up to date; `None` when there is no file. a newer file is an
/// `InvalidData` error that says so.
pub fn read(path: &Path, steps: &[Step<String>]) -> io::Result<Option<String>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    format::load_text(&text, steps)
        .map(Some)
        .map_err(|error| refused(path, error))
}

/// refuse (with the same error as [`read`]) before anything is written over a newer file.
pub fn guard(path: &Path, steps: &[Step<String>]) -> io::Result<()> {
    read(path, steps).map(|_| ())
}

/// store `text` as the file at `path` in one step: it is written beside the file, synced, and
/// renamed over it, so a crash leaves the old file or the new one, never half of one. the file
/// is private to the user.
pub fn write(path: &Path, text: &str) -> io::Result<()> {
    use std::io::Write;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let stored = options.open(&temp).and_then(|mut file| {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp, path)
    });
    if stored.is_err() {
        let _ = fs::remove_file(&temp);
    }
    stored
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lanlord-localfile-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("file")
    }

    #[test]
    fn a_newer_file_is_refused_with_the_clear_message() {
        let path = temp("newer");
        fs::write(&path, "# lanlord format 9, written by LANlord 7.4\nme@gw\n").unwrap();
        let error = read(&path, &[]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error
            .to_string()
            .ends_with("written by LANlord 7.4 (format 9); upgrade LANlord to use it"));
        assert!(guard(&path, &[]).is_err());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_missing_file_is_none_and_an_old_one_is_read() {
        let path = temp("old");
        assert_eq!(read(&path, &[]).unwrap(), None);
        fs::write(&path, "me@gw\n").unwrap();
        assert_eq!(read(&path, &[]).unwrap().as_deref(), Some("me@gw\n"));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
    #[test]
    fn a_write_replaces_the_file_and_leaves_nothing_beside_it() {
        let path = temp("write").join("file");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        write(&path, "one\n").unwrap();
        write(&path, "two\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two\n");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
