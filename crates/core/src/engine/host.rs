//! what the engine does to a machine: run a program, and read and write files. the real one runs
//! argv directly (never through a shell) and does its own file i/o; the fake one answers from a
//! script and keeps files in memory, so every plan and apply is tested without a machine.

use std::time::Duration;

use anyhow::Result;

/// how a program ended. a program that ran and failed is an `Output` with a code, not an error;
/// only a program that could not be started is an error.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(stdout: &str) -> Self {
        Self {
            code: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    pub fn failed(code: i32, stderr: &str) -> Self {
        Self {
            code,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }

    pub fn success(&self) -> bool {
        self.code == 0
    }
}

/// what a path is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stat {
    pub is_dir: bool,
    pub mode: u32,
}

pub trait Host {
    /// run `argv` with `stdin` on its standard input, and wait.
    fn run(&self, argv: &[&str], stdin: Option<&[u8]>) -> Result<Output>;
    /// the bytes of a file, or `None` when there is none.
    fn read(&self, path: &str) -> Result<Option<Vec<u8>>>;
    /// write a file whole and set its mode; a reader never sees half of it.
    fn write(&self, path: &str, bytes: &[u8], mode: u32) -> Result<()>;
    /// make a directory and its parents, and set the mode of the last one.
    fn mkdir(&self, path: &str, mode: u32) -> Result<()>;
    /// remove a file, or a directory and what is in it; no error when it is gone already.
    fn remove(&self, path: &str) -> Result<()>;
    fn stat(&self, path: &str) -> Result<Option<Stat>>;
    fn sleep(&self, how_long: Duration);
}

/// this machine.
pub struct RealHost;

#[cfg(unix)]
impl Host for RealHost {
    fn run(&self, argv: &[&str], stdin: Option<&[u8]>) -> Result<Output> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("no program to run"))?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| anyhow::anyhow!("could not run {program}: {error}"))?;
        if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
            // a child that exits without reading closes the pipe; that is its answer, not ours
            let _ = pipe.write_all(bytes);
        }
        let out = child.wait_with_output()?;
        Ok(Output {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(anyhow::anyhow!("could not read {path}: {error}")),
        }
    }

    fn write(&self, path: &str, bytes: &[u8], mode: u32) -> Result<()> {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let target = std::path::Path::new(path);
        let dir = target.parent().unwrap_or(std::path::Path::new("."));
        let name = target
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("{path} is not a file"))?;
        // unique per call: two writers of one file in one process must not share a temp file
        static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let temp = dir.join(format!(
            ".{}.lanlord-{}-{}",
            name.to_string_lossy(),
            std::process::id(),
            WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&temp);
        let written = (|| -> std::io::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(mode)
                .open(&temp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            // the mode asked for, whatever the umask took off
            std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(mode))?;
            std::fs::rename(&temp, target)
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        written.map_err(|error| anyhow::anyhow!("could not write {path}: {error}"))
    }

    fn mkdir(&self, path: &str, mode: u32) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(path)
            .and_then(|()| std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)))
            .map_err(|error| anyhow::anyhow!("could not make {path}: {error}"))
    }

    fn remove(&self, path: &str) -> Result<()> {
        let removed = match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
            Ok(_) => std::fs::remove_file(path),
            Err(_) => Ok(()),
        };
        removed.map_err(|error| anyhow::anyhow!("could not remove {path}: {error}"))
    }

    fn stat(&self, path: &str) -> Result<Option<Stat>> {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(meta) => Ok(Some(Stat {
                is_dir: meta.is_dir(),
                mode: meta.permissions().mode() & 0o7777,
            })),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(anyhow::anyhow!("could not look at {path}: {error}")),
        }
    }

    fn sleep(&self, how_long: Duration) {
        std::thread::sleep(how_long);
    }
}

#[cfg(not(unix))]
impl Host for RealHost {
    fn run(&self, _: &[&str], _: Option<&[u8]>) -> Result<Output> {
        anyhow::bail!("the engine runs on the gateway, a unix machine")
    }
    fn read(&self, _: &str) -> Result<Option<Vec<u8>>> {
        anyhow::bail!("the engine runs on the gateway, a unix machine")
    }
    fn write(&self, _: &str, _: &[u8], _: u32) -> Result<()> {
        anyhow::bail!("the engine runs on the gateway, a unix machine")
    }
    fn mkdir(&self, _: &str, _: u32) -> Result<()> {
        anyhow::bail!("the engine runs on the gateway, a unix machine")
    }
    fn remove(&self, _: &str) -> Result<()> {
        anyhow::bail!("the engine runs on the gateway, a unix machine")
    }
    fn stat(&self, _: &str) -> Result<Option<Stat>> {
        anyhow::bail!("the engine runs on the gateway, a unix machine")
    }
    fn sleep(&self, how_long: Duration) {
        std::thread::sleep(how_long);
    }
}

#[cfg(test)]
pub use fake::FakeHost;

#[cfg(test)]
mod fake {
    use std::cell::RefCell;
    use std::collections::{BTreeMap, VecDeque};

    use super::*;

    /// a machine that exists only in memory. programs answer from a script: the first rule whose
    /// text is the start of the argv line gives its next answer (the last answer repeats); a program
    /// nothing matches succeeds and says nothing. every call is kept for the test to read.
    #[derive(Default)]
    pub struct FakeHost {
        pub files: RefCell<BTreeMap<String, (Vec<u8>, u32)>>,
        pub dirs: RefCell<BTreeMap<String, u32>>,
        rules: RefCell<Vec<(String, VecDeque<Output>)>>,
        /// every program run, as one line, in order.
        pub calls: RefCell<Vec<String>>,
        /// the standard input given to each call that had one: (argv line, input).
        pub inputs: RefCell<Vec<(String, Vec<u8>)>>,
        pub slept: RefCell<Duration>,
    }

    impl FakeHost {
        pub fn new() -> Self {
            Self::default()
        }

        /// answer programs that start with `prefix` with `answers`, one per call.
        pub fn when(&self, prefix: &str, answers: Vec<Output>) -> &Self {
            // a later rule wins over an earlier one
            self.rules
                .borrow_mut()
                .insert(0, (prefix.to_string(), answers.into()));
            self
        }

        pub fn put(&self, path: &str, bytes: &[u8], mode: u32) -> &Self {
            self.files
                .borrow_mut()
                .insert(path.into(), (bytes.to_vec(), mode));
            self
        }

        pub fn text(&self, path: &str) -> Option<String> {
            self.files
                .borrow()
                .get(path)
                .map(|(bytes, _)| String::from_utf8_lossy(bytes).into_owned())
        }

        pub fn ran(&self, prefix: &str) -> usize {
            self.calls
                .borrow()
                .iter()
                .filter(|line| line.starts_with(prefix))
                .count()
        }

        pub fn log(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl Host for FakeHost {
        fn run(&self, argv: &[&str], stdin: Option<&[u8]>) -> Result<Output> {
            let line = argv.join(" ");
            self.calls.borrow_mut().push(line.clone());
            if let Some(bytes) = stdin {
                self.inputs
                    .borrow_mut()
                    .push((line.clone(), bytes.to_vec()));
            }
            let mut rules = self.rules.borrow_mut();
            for (prefix, answers) in rules.iter_mut() {
                let matches = line == *prefix || line.starts_with(&format!("{prefix} "));
                if matches {
                    let answer = if answers.len() > 1 {
                        answers.pop_front()
                    } else {
                        answers.front().cloned()
                    };
                    return Ok(answer.unwrap_or_default());
                }
            }
            Ok(Output::default())
        }

        fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self
                .files
                .borrow()
                .get(path)
                .map(|(bytes, _)| bytes.clone()))
        }

        fn write(&self, path: &str, bytes: &[u8], mode: u32) -> Result<()> {
            self.put(path, bytes, mode);
            Ok(())
        }

        fn mkdir(&self, path: &str, mode: u32) -> Result<()> {
            self.dirs.borrow_mut().insert(path.into(), mode);
            Ok(())
        }

        fn remove(&self, path: &str) -> Result<()> {
            let below = format!("{path}/");
            self.files
                .borrow_mut()
                .retain(|name, _| name != path && !name.starts_with(&below));
            self.dirs
                .borrow_mut()
                .retain(|name, _| name != path && !name.starts_with(&below));
            Ok(())
        }

        fn stat(&self, path: &str) -> Result<Option<Stat>> {
            if let Some((_, mode)) = self.files.borrow().get(path) {
                return Ok(Some(Stat {
                    is_dir: false,
                    mode: *mode,
                }));
            }
            Ok(self.dirs.borrow().get(path).map(|mode| Stat {
                is_dir: true,
                mode: *mode,
            }))
        }

        fn sleep(&self, how_long: Duration) {
            *self.slept.borrow_mut() += how_long;
        }
    }
}
