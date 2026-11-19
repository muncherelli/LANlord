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
}
