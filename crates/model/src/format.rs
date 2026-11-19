//! versions of the files lanlord stores, and the steps that bring an older file up to date.
//!
//! every stored file says which format it is in, and which lanlord wrote it. a file with no
//! version is version 1, the format before versions existed. a list of steps, each turning
//! version N into N+1, runs when a file is loaded, so an old file is always readable in memory;
//! `lanlord upgrade` stores the result once. nothing turns a newer file into an older one: a
//! file from a newer lanlord is refused ([`TooNew`]) and never overwritten. see
//! docs/stored-formats.md for how to change a format.
//!
//! this is plain data and text: it is safe to use in the browser build.

use std::fmt;

use anyhow::{Context as _, Result};

/// the yaml key (top level) that holds the format version of a stored settings file.
pub const FORMAT_KEY: &str = "lanlord_format";
/// the yaml key (top level) that holds the lanlord, as `major.minor`, that wrote the file.
pub const WRITTEN_BY_KEY: &str = "lanlord_version";

/// the lanlord that is running, as `major.minor`.
pub fn written_by() -> &'static str {
    concat!(
        env!("CARGO_PKG_VERSION_MAJOR"),
        ".",
        env!("CARGO_PKG_VERSION_MINOR")
    )
}

/// a file written in a format newer than this lanlord knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TooNew {
    pub found: u32,
    /// the lanlord (`major.minor`) that wrote it, when the file says.
    pub by: Option<String>,
}

impl fmt::Display for TooNew {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let by = match &self.by {
            Some(by) => format!("LANlord {by}"),
            None => "a newer LANlord".to_string(),
        };
        write!(
            f,
            "written by {by} (format {}); upgrade LANlord to use it",
            self.found
        )
    }
}

impl std::error::Error for TooNew {}

/// refuse a file whose format is past the `known` one.
pub fn check(found: u32, by: Option<&str>, known: u32) -> Result<(), TooNew> {
    if found > known {
        return Err(TooNew {
            found,
            by: by.map(str::to_string),
        });
    }
    Ok(())
}

/// whether `stamp` looks like the time that names an upgrade's backups, `20260101T120000Z`; such
/// text is safe in a file name.
pub fn backup_stamp(stamp: &str) -> bool {
    stamp.len() == 16
        && stamp.chars().enumerate().all(|(i, c)| match i {
            8 => c == 'T',
            15 => c == 'Z',
            _ => c.is_ascii_digit(),
        })
}

/// what a step may need to know about the machine beyond the file itself. a load that has no
/// such knowledge (a front end, the web server) leaves it empty, and the steps that need it
/// wait for the upgrade, which fills it in on the gateway.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Context {
    /// the gateway's administrators.
    pub admins: Option<Vec<String>>,
}

/// one step: it turns the format at its place in the list into the next one.
pub struct Step<T> {
    pub name: &'static str,
    pub run: fn(&mut T, &Context) -> Result<()>,
    /// whether the context holds what the step needs. a step that is not ready stops the run
    /// there: the file is stored as far as it got, and the next load starts at that step.
    pub ready: fn(&Context) -> bool,
}

/// a step that needs nothing beyond the file.
pub fn always(_: &Context) -> bool {
    true
}

/// the newest format a list of steps leads to: version 1 plus one per step.
pub const fn current<T>(steps: &[Step<T>]) -> u32 {
    steps.len() as u32 + 1
}

/// run the steps that take `value` from version `from` on. it returns the version it got to:
/// the newest one, unless a step was not ready (see [`Step::ready`]).
pub fn run_steps<T>(value: &mut T, from: u32, steps: &[Step<T>], context: &Context) -> Result<u32> {
    let mut reached = from.max(1);
    for (index, step) in steps
        .iter()
        .enumerate()
        .skip(from.saturating_sub(1) as usize)
    {
        if !(step.ready)(context) {
            break;
        }
        (step.run)(value, context)
            .with_context(|| format!("format {} to {} ({})", index + 1, index + 2, step.name))?;
        reached = index as u32 + 2;
    }
    Ok(reached)
}

/// the line that starts a line-based file (hosts, preferences) in format `version`. older
/// readers see a comment.
pub fn text_header(version: u32) -> String {
    format!(
        "# lanlord format {version}, written by LANlord {}\n",
        written_by()
    )
}

/// the version a line-based file says it has and the rest of it, header left out. no header is
/// version 1. `Err` when `known` is older than the file.
pub fn read_text(text: &str, known: u32) -> Result<(u32, &str), TooNew> {
    let first = text.lines().next().unwrap_or_default();
    let Some(rest) = first.strip_prefix("# lanlord format ") else {
        return Ok((1, text));
    };
    let (number, by) = match rest.split_once(", written by LANlord ") {
        Some((number, by)) => (number, Some(by.trim())),
        None => (rest, None),
    };
    let Ok(found) = number.trim().parse::<u32>() else {
        return Ok((1, text));
    };
    check(found, by, known)?;
    let body = text.get(first.len()..).unwrap_or_default();
    let body = body
        .strip_prefix("\r\n")
        .or_else(|| body.strip_prefix('\n'));
    Ok((found, body.unwrap_or_default()))
}

/// `text` of a settings file without its two version lines (they start at the first column).
/// the playbooks get the settings without them.
pub fn without_stamp(text: &str) -> String {
    text.lines()
        .filter(|line| {
            !line.starts_with(&format!("{FORMAT_KEY}:"))
                && !line.starts_with(&format!("{WRITTEN_BY_KEY}:"))
        })
        .map(|line| format!("{line}\n"))
        .collect()
}

/// a line-based file's text, brought up to date: the header comes off and the `steps` run on
/// the rest, from the version the file says (1 when it says nothing). a newer file is refused
/// with [`TooNew`].
pub fn load_text(text: &str, steps: &[Step<String>]) -> Result<String> {
    let (version, body) = read_text(text, current(steps))?;
    let mut body = body.to_string();
    run_steps(&mut body, version, steps, &Context::default())?;
    Ok(body)
}

/// the text of a line-based file to store: the header of the current format, then `body`.
pub fn render_text(body: &str, steps: &[Step<String>]) -> String {
    format!("{}{body}", text_header(current(steps)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refusal_names_the_writer_and_the_format() {
        let error = TooNew {
            found: 7,
            by: Some("9.2".into()),
        };
        assert_eq!(
            error.to_string(),
            "written by LANlord 9.2 (format 7); upgrade LANlord to use it"
        );
        let unknown = TooNew { found: 7, by: None };
        assert!(unknown
            .to_string()
            .starts_with("written by a newer LANlord (format 7)"));
    }

    #[test]
    fn a_text_file_without_a_header_is_version_one() {
        assert_eq!(read_text("me@gw\n", 1), Ok((1, "me@gw\n")));
        assert_eq!(read_text("", 1), Ok((1, "")));
    }

    #[test]
    fn a_text_header_comes_off_and_a_newer_one_is_refused() {
        let text = format!("{}me@gw\n", text_header(1));
        assert_eq!(read_text(&text, 1), Ok((1, "me@gw\n")));
        let newer = "# lanlord format 4, written by LANlord 9.2\nme@gw\n";
        assert_eq!(
            read_text(newer, 1),
            Err(TooNew {
                found: 4,
                by: Some("9.2".into())
            })
        );
        assert_eq!(read_text(newer, 4), Ok((4, "me@gw\n")));
    }

    fn bump(list: &mut Vec<u32>, _: &Context) -> Result<()> {
        list.push(list.len() as u32 + 1);
        Ok(())
    }

    #[test]
    fn steps_run_from_the_files_version_on() {
        let steps = [
            Step {
                name: "one",
                run: bump,
                ready: always,
            },
            Step {
                name: "two",
                run: bump,
                ready: always,
            },
        ];
        assert_eq!(current(&steps), 3);
        let mut seen = Vec::new();
        assert_eq!(
            run_steps(&mut seen, 1, &steps, &Context::default()).unwrap(),
            3
        );
        assert_eq!(seen.len(), 2);
        let mut seen = Vec::new();
        run_steps(&mut seen, 2, &steps, &Context::default()).unwrap();
        assert_eq!(seen.len(), 1);
        let mut seen = Vec::new();
        run_steps(&mut seen, 3, &steps, &Context::default()).unwrap();
        assert!(seen.is_empty());
    }
    fn never(_: &Context) -> bool {
        false
    }

    #[test]
    fn a_step_that_is_not_ready_stops_the_run_where_it_is() {
        let steps = [
            Step {
                name: "one",
                run: bump,
                ready: always,
            },
            Step {
                name: "two",
                run: bump,
                ready: never,
            },
        ];
        let mut seen = Vec::new();
        assert_eq!(
            run_steps(&mut seen, 1, &steps, &Context::default()).unwrap(),
            2
        );
        assert_eq!(seen.len(), 1);
    }
}
