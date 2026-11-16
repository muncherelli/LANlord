//! knowing whether a newer lanlord exists, and rebuilding and relaunching a development build.
//! the git and http calls go through a `Runner`, so tests use a fake one.

use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::log::{emit, Kind};

const REPO: &str = "muncherelli/LANlord";
/// how long a git or http call may take before it is given up on.
const CALL_LIMIT: Duration = Duration::from_secs(30);
/// how many lines of "what is new" the update box lists.
const MAX_LINES: usize = 12;
/// how often the check runs in the background.
pub const CHECK_EVERY: Duration = Duration::from_secs(3600);

/// runs an external program and returns what it printed.
pub trait Runner: Send + Sync {
    fn run(&self, program: &str, args: &[&str], dir: Option<&Path>) -> Result<String>;
}

/// the real thing: a child process that is killed when it takes too long.
pub struct System;

impl Runner for System {
    fn run(&self, program: &str, args: &[&str], dir: Option<&Path>) -> Result<String> {
        run_limited(program, args, dir, CALL_LIMIT)
    }
}

fn run_limited(
    program: &str,
    args: &[&str],
    dir: Option<&Path>,
    limit: Duration,
) -> Result<String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0");
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("could not run {program}"))?;
    let mut stdout = child.stdout.take().context("no output pipe")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        let _ = tx.send(text);
    });
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            let text = rx.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
            if !status.success() {
                bail!("{program} failed ({status})");
            }
            return Ok(text);
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{program} took too long");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// where a newer version would come from, and so how to install it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// new commits on trunk in the checkout this build was made from.
    Git,
    /// a new release; the page to open.
    Release { url: String },
}

/// what is new, for the notice and its box.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    pub source: Source,
    pub headline: String,
    pub lines: Vec<String>,
}

/// the checkout a development build can be rebuilt from: LANLORD_DEV, else the place this
/// build was made (debug builds only), when it is still a git checkout.
pub fn dev_repo() -> Option<PathBuf> {
    dev_repo_for(std::env::var_os("LANLORD_DEV").map(PathBuf::from))
}

fn dev_repo_for(chosen: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(path) = chosen {
        return path.join(".git").exists().then_some(path);
    }
    if !cfg!(debug_assertions) {
        return None;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2)?;
    root.join(".git").exists().then(|| root.to_path_buf())
}

/// is a newer trunk waiting in `repo`? fetches first.
pub fn check_dev(runner: &dyn Runner, repo: &Path) -> Result<Option<Update>> {
    runner.run("git", &["fetch", "--quiet", "origin", "trunk"], Some(repo))?;
    let count = runner.run(
        "git",
        &["rev-list", "--count", "HEAD..origin/trunk"],
        Some(repo),
    )?;
    let behind: usize = count.trim().parse().context("unexpected git output")?;
    if behind == 0 {
        return Ok(None);
    }
    let log = runner.run(
        "git",
        &["log", "--oneline", "HEAD..origin/trunk"],
        Some(repo),
    )?;
    let lines = log.lines().take(MAX_LINES).map(str::to_string).collect();
    let plural = if behind == 1 { "" } else { "s" };
    Ok(Some(Update {
        source: Source::Git,
        headline: format!("{behind} new change{plural} on trunk"),
        lines,
    }))
}

/// is there a release newer than `current`? asks github anonymously.
pub fn check_release(runner: &dyn Runner, current: &str) -> Result<Option<Update>> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let reply = runner.run(
        "curl",
        &[
            "-fsS",
            "--max-time",
            "20",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: lanlord",
            &url,
        ],
        None,
    )?;
    let tag = json_string(&reply, "tag_name").context("no release found")?;
    if !newer(current, &tag) {
        return Ok(None);
    }
    let page = json_string(&reply, "html_url")
        .unwrap_or_else(|| format!("https://github.com/{REPO}/releases/latest"));
    let body = json_string(&reply, "body").unwrap_or_default();
    let lines = body
        .lines()
        .map(|l| l.trim().trim_start_matches(['-', '*', ' ']).to_string())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .take(MAX_LINES)
        .collect();
    Ok(Some(Update {
        source: Source::Release { url: page },
        headline: format!("LANlord {} is available", tag.trim_start_matches('v')),
        lines,
    }))
}

/// is `latest` a higher version than `current`? both look like 1.2.3, with an optional v.
pub fn newer(current: &str, latest: &str) -> bool {
    fn parts(version: &str) -> Vec<u64> {
        version
            .trim()
            .trim_start_matches('v')
            .split(['-', '+'])
            .next()
            .unwrap_or("")
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    }
    let (mut a, mut b) = (parts(current), parts(latest));
    let len = a.len().max(b.len());
    a.resize(len, 0);
    b.resize(len, 0);
    b > a
}

/// the string value of `"key": "..."` in a json document, with its escapes undone.
pub fn json_string(text: &str, key: &str) -> Option<String> {
    let marker = format!("\"{key}\"");
    let after = &text[text.find(&marker)? + marker.len()..];
    let after = after.trim_start().strip_prefix(':')?.trim_start();
    let mut chars = after.strip_prefix('"')?.chars();
    let mut out = String::new();
    loop {
        match chars.next()? {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                'r' => {}
                't' => out.push('\t'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.push(
                        u32::from_str_radix(&hex, 16)
                            .ok()
                            .and_then(char::from_u32)
                            .unwrap_or('?'),
                    );
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
}

/// the update notice's content when LANLORD_FAKE_UPDATE is set (to 1, or to the name of a state to
/// look at), to see it without a network.
fn fake() -> Update {
    Update {
        source: Source::Git,
        headline: "2 new changes on trunk".into(),
        lines: vec![
            "a1b2c3d feat(ui): a pretend change".into(),
            "e4f5a6b fix(core): another pretend change".into(),
        ],
    }
}

/// look for an update; `None` when there is none, when checking is off or when anything fails.
pub fn check(runner: &dyn Runner, enabled: bool) -> Option<Update> {
    let off = std::env::var("LANLORD_NO_UPDATE_CHECK").as_deref() == Ok("1");
    if !enabled || off {
        return None;
    }
    if std::env::var("LANLORD_FAKE_UPDATE").is_ok_and(|v| !v.is_empty()) {
        return Some(fake());
    }
    match dev_repo() {
        Some(repo) => check_dev(runner, &repo),
        None => check_release(runner, env!("CARGO_PKG_VERSION")),
    }
    .ok()
    .flatten()
}

/// one command of a rebuild.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub program: String,
    pub args: Vec<String>,
    pub dir: PathBuf,
}

/// the `.app` folder `exe` runs from, on macos.
fn bundle_of(exe: &Path) -> Option<&Path> {
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
}

/// the commands that bring the build at `exe` up to date from `repo`: an optional fast-forward
/// pull, then cargo, or the app build script for an installed .app.
pub fn rebuild_steps(repo: &Path, exe: &Path, pull: bool) -> Vec<Step> {
    let step = |program: &str, args: &[&str]| Step {
        program: program.into(),
        args: args.iter().map(ToString::to_string).collect(),
        dir: repo.to_path_buf(),
    };
    let mut steps = Vec::new();
    if pull {
        steps.push(step("git", &["pull", "--ff-only", "origin", "trunk"]));
    }
    match bundle_of(exe) {
        Some(app) => {
            let script = repo.join("tools").join("build-app.sh");
            steps.push(step(&script.to_string_lossy(), &[&app.to_string_lossy()]));
        }
        None if exe.components().any(|c| c.as_os_str() == "release") => {
            steps.push(step("cargo", &["build", "--release", "-p", "lanlord"]));
        }
        None => steps.push(step("cargo", &["build", "-p", "lanlord"])),
    }
    steps
}

/// how many of the last lines a failed step shows in its error.
const TAIL_LINES: usize = 12;

/// the search path with `~/.cargo/bin` added, because an app opened from the Finder or a
/// launcher does not inherit the shell's path.
fn search_path() -> OsString {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        dirs.push(PathBuf::from(home).join(".cargo").join("bin"));
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

/// the program called `name`, looked up in `path`; a name with a separator is taken as it is.
fn find_program(name: &str, path: &OsStr) -> Option<PathBuf> {
    if name.contains('/') || name.contains('\\') {
        return Path::new(name).is_file().then(|| PathBuf::from(name));
    }
    std::env::split_paths(path)
        .flat_map(|dir| {
            let plain = dir.join(name);
            [plain.clone(), plain.with_extension("exe")]
        })
        .find(|candidate| candidate.is_file())
}

/// a flag a background job checks so it can be stopped early.
#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// something to call whenever a job makes progress, such as a line of output.
pub type Beat = Arc<dyn Fn() + Send + Sync>;

/// why a step did not finish.
#[derive(Debug)]
pub enum StepFault {
    Cancelled,
    Failed { message: String, tail: Vec<String> },
}

/// start `command` in its own process group, so that stopping it stops everything it started.
fn own_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(not(unix))]
    let _ = command;
}

/// signal a whole process group through the shell's own `kill`, which understands `--` and a
/// negative group id the same way everywhere; never a group below 2, which could be everything.
#[cfg(unix)]
fn signal_group(signal: &str, leader: u32) {
    if leader < 2 {
        return;
    }
    let _ = Command::new("sh")
        .args(["-c", "kill -s \"$1\" -- \"$2\" 2>/dev/null", "sh"])
        .arg(signal)
        .arg(format!("-{leader}"))
        .status();
}

/// stop a step and whatever it started: politely first, then for good.
fn stop(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        signal_group("TERM", child.id());
        for _ in 0..20 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        signal_group("KILL", child.id());
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// run `step`, sending every line it prints to the console and to `beat` as it comes. it stops
/// when `cancel` is set; a failure carries the last lines it printed.
pub fn run_step_with(step: &Step, cancel: &Cancel, beat: &Beat) -> Result<(), StepFault> {
    emit(
        Kind::Info,
        format!("$ {} {}", step.program, step.args.join(" ")),
    );
    let fail = |message: String| StepFault::Failed {
        message,
        tail: Vec::new(),
    };
    let mut command = Command::new(&step.program);
    command
        .args(&step.args)
        .current_dir(&step.dir)
        .env("PATH", search_path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    own_group(&mut command);
    let mut child = command
        .spawn()
        .map_err(|e| fail(format!("could not run {}: {e}", step.program)))?;
    let tail: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut readers = Vec::new();
    let out = child
        .stdout
        .take()
        .map(|p| Box::new(p) as Box<dyn Read + Send>);
    let err = child
        .stderr
        .take()
        .map(|p| Box::new(p) as Box<dyn Read + Send>);
    for pipe in [out, err].into_iter().flatten() {
        let tail = Arc::clone(&tail);
        let beat = Arc::clone(beat);
        readers.push(std::thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                emit(Kind::Info, line.clone());
                beat();
                let mut kept = tail
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                kept.push(line);
                if kept.len() > TAIL_LINES {
                    kept.remove(0);
                }
            }
        }));
    }
    let status = loop {
        if cancel.is_cancelled() {
            stop(&mut child);
            // the readers end by themselves once the pipes close; never wait for them here
            drop(readers);
            return Err(StepFault::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(e) => return Err(fail(format!("could not watch {}: {e}", step.program))),
        }
    };
    for reader in readers {
        let _ = reader.join();
    }
    if status.success() {
        return Ok(());
    }
    let kept = tail
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Err(StepFault::Failed {
        message: format!("{} failed ({status})", step.program),
        tail: kept.clone(),
    })
}

/// run `step` to the end, sending every line it prints to the console; a failure carries the
/// last lines it printed.
pub fn run_step(step: &Step) -> Result<()> {
    let beat: Beat = Arc::new(|| {});
    match run_step_with(step, &Cancel::default(), &beat) {
        Ok(()) => Ok(()),
        Err(StepFault::Cancelled) => bail!("{} was stopped", step.program),
        Err(StepFault::Failed { message, tail }) => bail!("{message}{}", indent(&tail)),
    }
}

/// does this output look like the network being down, rather than something wrong with the
/// checkout or the build?
pub fn is_network_error(text: &str) -> bool {
    const SIGNS: [&str; 9] = [
        "Could not resolve host",
        "Failed to connect",
        "Network is unreachable",
        "Connection timed out",
        "Operation timed out",
        "Connection refused",
        "Temporary failure in name resolution",
        "unable to access",
        "Could not read from remote repository",
    ];
    SIGNS.iter().any(|sign| text.contains(sign))
}

/// a git operation that was cut off leaves marks in the checkout; say so rather than carry on.
pub fn checkout_problem(repo: &Path) -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(repo)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let dir = if Path::new(&dir).is_absolute() {
        PathBuf::from(dir)
    } else {
        repo.join(dir)
    };
    [
        (
            "index.lock",
            "a git command did not finish; if none is running, remove this file",
        ),
        ("MERGE_HEAD", "a merge is half done"),
        ("rebase-merge", "a rebase is half done"),
        ("rebase-apply", "a rebase is half done"),
        ("CHERRY_PICK_HEAD", "a cherry-pick is half done"),
    ]
    .iter()
    .find(|(name, _)| dir.join(name).exists())
    .map(|(name, what)| {
        format!(
            "the checkout was left half-finished: {what} ({})",
            dir.join(name).display()
        )
    })
}

/// an app build that was cut off leaves its half-built copy beside the installed app.
pub fn app_leftover(exe: &Path) -> Option<String> {
    let app = bundle_of(exe)?;
    let name = app.file_name()?.to_string_lossy().into_owned();
    let prefix = format!("{name}.tmp.");
    std::fs::read_dir(app.parent()?)
        .ok()?
        .filter_map(Result::ok)
        .find(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .map(|entry| {
            format!(
                "a half-built copy was left at {}; the installed app was not touched",
                entry.path().display()
            )
        })
}

/// `lines` as an indented block after a newline, or nothing when there are none.
pub(crate) fn indent(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    format!("\n  {}", lines.join("\n  "))
}

/// run the whole rebuild, stopping at the first failure.
pub fn rebuild(steps: &[Step]) -> Result<()> {
    for step in steps {
        run_step(step)?;
    }
    emit(Kind::Info, "rebuilt; starting the new build");
    Ok(())
}

/// check, before anything is changed, that a rebuild can run: the checkout is still there and
/// every program the build needs can be found.
pub fn preflight(repo: &Path, exe: &Path, pull: bool) -> Result<()> {
    preflight_with(repo, exe, pull, &search_path())
}

fn preflight_with(repo: &Path, exe: &Path, pull: bool, path: &OsStr) -> Result<()> {
    if !repo.is_dir() {
        bail!(
            "the checkout {} is gone, so this build cannot be rebuilt; nothing was changed",
            repo.display()
        );
    }
    for step in rebuild_steps(repo, exe, pull) {
        if find_program(&step.program, path).is_none() {
            let hint = match step.program.as_str() {
                "cargo" => " (install Rust with rustup, or start LANlord from a terminal where cargo works)",
                _ => "",
            };
            bail!("{} was not found{hint}; nothing was changed", step.program);
        }
    }
    Ok(())
}

/// check that the rebuilt build at `exe` is really there and can be started.
pub fn verify_built(exe: &Path) -> Result<()> {
    let meta = std::fs::metadata(exe)
        .with_context(|| format!("the build finished but {} is missing", exe.display()))?;
    if !meta.is_file() {
        bail!("the build finished but {} is not a program", exe.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 == 0 {
            bail!("the build finished but {} cannot be run", exe.display());
        }
    }
    Ok(())
}

/// the command that starts the rebuilt build at `exe`, signing in again to `host` on its own
/// when the agent allows it.
pub fn relaunch_command(exe: &Path, host: Option<&str>) -> Command {
    let mut command = Command::new(exe);
    command
        .arg("gui")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(host) = host {
        command.env("LANLORD_LAST_HOST", host);
        command.env("LANLORD_RELAUNCHED", "1");
    }
    command
}

/// how long a started build has to stay alive before the old one may quit, and how often to look.
#[derive(Clone, Copy, Debug)]
pub struct Grace {
    pub total: Duration,
    pub poll: Duration,
}

impl Default for Grace {
    fn default() -> Self {
        Self {
            total: Duration::from_secs(2),
            poll: Duration::from_millis(50),
        }
    }
}

/// start the child in its own process group (or detached process on windows), so closing this
/// window does not take it down.
fn detach(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
        command.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    #[cfg(not(any(unix, windows)))]
    let _ = command;
}

/// the last lines of the new build's error output, as an indented block.
fn log_tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<String> = text
        .lines()
        .rev()
        .take(TAIL_LINES)
        .map(str::to_string)
        .collect();
    indent(&lines.into_iter().rev().collect::<Vec<_>>())
}

/// start the rebuilt build and watch it for `grace`: `Ok` only when it is still running after
/// that, so the old one may quit; an early exit says how it ended and what it printed. the
/// process is started beside this one, never in place of it.
pub fn relaunch(exe: &Path, host: Option<&str>, grace: Grace) -> Result<()> {
    static SERIAL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let log = std::env::temp_dir().join(format!(
        "lanlord-relaunch-{}-{serial}.log",
        std::process::id()
    ));
    let file = std::fs::File::create(&log).context("could not make a log for the new build")?;
    let mut command = relaunch_command(exe, host);
    command.stderr(Stdio::from(file));
    detach(&mut command);
    let mut child = command
        .spawn()
        .with_context(|| format!("could not start {}", exe.display()))?;
    let started = Instant::now();
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                break Err(anyhow::anyhow!(
                    "the new build exited at once ({status}){}",
                    log_tail(&log)
                ));
            }
            Ok(None) if started.elapsed() >= grace.total => break Ok(()),
            Ok(None) => std::thread::sleep(grace.poll),
            Err(error) => break Err(anyhow::anyhow!("could not watch the new build: {error}")),
        }
    };
    // the build keeps its handle on the file; only the name goes
    let _ = std::fs::remove_file(&log);
    result
}

/// the whole rebuild and relaunch: check first, build, check the result, start it and watch it.
/// the caller quits only when this says `Ok`.
pub fn install_build(
    repo: &Path,
    exe: &Path,
    host: Option<&str>,
    pull: bool,
    grace: Grace,
) -> Result<()> {
    preflight(repo, exe, pull)?;
    rebuild(&rebuild_steps(repo, exe, pull))?;
    verify_built(exe)?;
    relaunch(exe, host, grace)?;
    emit(Kind::Info, "the new build is running; closing this window");
    Ok(())
}

/// the program that opens a web page on this system.
fn opener() -> (&'static str, &'static [&'static str]) {
    if cfg!(target_os = "macos") {
        ("open", &[])
    } else if cfg!(windows) {
        ("cmd", &["/c", "start", ""])
    } else {
        ("xdg-open", &[])
    }
}

/// open a release page in the browser; only https github pages of this project are opened.
pub fn open_release(url: &str) -> Result<()> {
    if !url.starts_with(&format!("https://github.com/{REPO}/")) {
        bail!("not a release page of this project: {url}");
    }
    let (program, args) = opener();
    Command::new(program)
        .args(args)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("could not open {url}"))?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// answers calls from a list of (command words, reply) pairs.
    struct Fake {
        replies: Vec<(&'static str, Result<&'static str, &'static str>)>,
        seen: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(replies: Vec<(&'static str, Result<&'static str, &'static str>)>) -> Self {
            Self {
                replies,
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    impl Runner for Fake {
        fn run(&self, program: &str, args: &[&str], _dir: Option<&Path>) -> Result<String> {
            let call = format!("{program} {}", args.join(" "));
            self.seen.lock().unwrap().push(call.clone());
            for (prefix, reply) in &self.replies {
                if call.starts_with(prefix) {
                    return reply.map(str::to_string).map_err(|e| anyhow::anyhow!(e));
                }
            }
            bail!("unexpected call: {call}")
        }
    }

    #[test]
    fn versions_compare_by_number_not_by_text() {
        assert!(newer("0.1.0", "v0.1.1"));
        assert!(newer("0.9.0", "0.10.0"));
        assert!(newer("1.0", "1.0.1"));
        assert!(!newer("0.1.1", "0.1.1"));
        assert!(!newer("0.2.0", "v0.1.9"));
        assert!(!newer("1.0.0", "1.0.0-rc1"));
        assert!(!newer("1.0.0", "garbage"));
    }

    #[test]
    fn json_strings_come_out_with_their_escapes_undone() {
        let doc = r#"{"tag_name": "v0.2.0", "body": "- one\r\n- \"two\" é\n", "n": 3}"#;
        assert_eq!(json_string(doc, "tag_name").as_deref(), Some("v0.2.0"));
        assert_eq!(
            json_string(doc, "body").as_deref(),
            Some("- one\n- \"two\" é\n")
        );
        assert_eq!(json_string(doc, "n"), None);
        assert_eq!(json_string(doc, "missing"), None);
        assert_eq!(json_string(r#"{"a": "open"#, "a"), None);
    }

    #[test]
    fn a_dev_build_behind_trunk_lists_the_new_commits() {
        let fake = Fake::new(vec![
            ("git fetch", Ok("")),
            ("git rev-list", Ok("2\n")),
            ("git log", Ok("abc feat: one\ndef fix: two\n")),
        ]);
        let update = check_dev(&fake, Path::new("/repo")).unwrap().unwrap();
        assert_eq!(update.source, Source::Git);
        assert_eq!(update.headline, "2 new changes on trunk");
        assert_eq!(update.lines, ["abc feat: one", "def fix: two"]);
    }

    #[test]
    fn a_dev_build_up_to_date_or_failing_says_nothing() {
        let current = Fake::new(vec![("git fetch", Ok("")), ("git rev-list", Ok("0\n"))]);
        assert_eq!(check_dev(&current, Path::new("/repo")).unwrap(), None);
        let offline = Fake::new(vec![("git fetch", Err("no network"))]);
        assert!(check_dev(&offline, Path::new("/repo")).is_err());
        let odd = Fake::new(vec![("git fetch", Ok("")), ("git rev-list", Ok("x"))]);
        assert!(check_dev(&odd, Path::new("/repo")).is_err());
    }

    #[test]
    fn one_commit_is_singular_and_long_logs_are_cut() {
        let one = Fake::new(vec![
            ("git fetch", Ok("")),
            ("git rev-list", Ok("1")),
            ("git log", Ok("abc x")),
        ]);
        let update = check_dev(&one, Path::new("/r")).unwrap().unwrap();
        assert_eq!(update.headline, "1 new change on trunk");
        let many = Fake::new(vec![
            ("git fetch", Ok("")),
            ("git rev-list", Ok("30")),
            (
                "git log",
                Ok("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\no\n"),
            ),
        ]);
        let update = check_dev(&many, Path::new("/r")).unwrap().unwrap();
        assert_eq!(update.lines.len(), MAX_LINES);
    }

    #[test]
    fn a_newer_release_is_found_and_an_older_one_is_not() {
        let reply = r##"{"tag_name":"v0.3.0","html_url":"https://github.com/muncherelli/LANlord/releases/tag/v0.3.0","body":"# What's new\n- reset a machine\n* software share\n\n"}"##;
        let fake = Fake::new(vec![("curl", Ok(reply))]);
        let update = check_release(&fake, "0.2.0").unwrap().unwrap();
        assert_eq!(update.headline, "LANlord 0.3.0 is available");
        assert_eq!(update.lines, ["reset a machine", "software share"]);
        assert_eq!(
            update.source,
            Source::Release {
                url: "https://github.com/muncherelli/LANlord/releases/tag/v0.3.0".into()
            }
        );
        assert!(check_release(&fake, "0.3.0").unwrap().is_none());
        let broken = Fake::new(vec![("curl", Ok("{}"))]);
        assert!(check_release(&broken, "0.1.0").is_err());
        let minimal = Fake::new(vec![("curl", Ok(r#"{"tag_name":"v9.0.0"}"#))]);
        let update = check_release(&minimal, "0.1.0").unwrap().unwrap();
        assert!(
            matches!(update.source, Source::Release { url } if url.ends_with("/releases/latest"))
        );
    }

    #[test]
    fn checking_respects_the_switches() {
        let fake = Fake::new(Vec::new());
        // off in the preferences: no calls at all
        assert!(check(&fake, false).is_none());
        assert!(fake.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn a_rebuild_is_cargo_for_a_dev_build_and_the_script_for_an_app() {
        let repo = Path::new("/repo");
        let debug = rebuild_steps(repo, Path::new("/repo/target/debug/lanlord"), false);
        assert_eq!(debug.len(), 1);
        assert_eq!(debug[0].args, ["build", "-p", "lanlord"]);
        let release = rebuild_steps(repo, Path::new("/repo/target/release/lanlord"), true);
        assert_eq!(release.len(), 2);
        assert_eq!(release[0].program, "git");
        assert_eq!(release[0].args, ["pull", "--ff-only", "origin", "trunk"]);
        assert_eq!(release[1].args, ["build", "--release", "-p", "lanlord"]);
        let app = rebuild_steps(
            repo,
            Path::new("/Users/me/Applications/LANlord.app/Contents/MacOS/lanlord-bin"),
            false,
        );
        assert_eq!(app[0].program, "/repo/tools/build-app.sh");
        assert_eq!(app[0].args, ["/Users/me/Applications/LANlord.app"]);
        assert!(app.iter().all(|s| s.dir == repo));
    }

    #[test]
    fn running_steps_stops_at_the_first_failure() {
        let dir = std::env::temp_dir();
        let sh = |script: &str| Step {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            dir: dir.clone(),
        };
        run_step(&sh("echo hello; echo oops >&2")).unwrap();
        let err = run_step(&sh("exit 3")).unwrap_err();
        assert!(err.to_string().contains("failed"), "{err}");
        let marker = dir.join(format!("lanlord-update-{}", std::process::id()));
        let steps = [sh("exit 1"), sh(&format!("touch {}", marker.display()))];
        assert!(rebuild(&steps).is_err());
        assert!(!marker.exists(), "the second step must not run");
        assert!(rebuild(&[sh("true")]).is_ok());
        let missing = Step {
            program: "/no/such/program".into(),
            args: Vec::new(),
            dir,
        };
        assert!(run_step(&missing).is_err());
    }

    #[test]
    fn the_system_runner_returns_output_and_gives_up_on_slow_calls() {
        let ok = System.run("echo", &["hi"], None).unwrap();
        assert_eq!(ok.trim(), "hi");
        assert!(System.run("false", &[], None).is_err());
        assert!(System
            .run("/no/such/program", &[], Some(Path::new("/")))
            .is_err());
        let slow = run_limited("sleep", &["5"], None, Duration::from_millis(100));
        assert!(slow.unwrap_err().to_string().contains("too long"));
    }

    #[test]
    fn the_relaunch_command_carries_the_last_server() {
        let command = relaunch_command(Path::new("/x/lanlord"), Some("me@gw"));
        let envs: Vec<_> = command
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(envs.contains(&("LANLORD_LAST_HOST".into(), Some("me@gw".into()))));
        assert!(envs.contains(&("LANLORD_RELAUNCHED".into(), Some("1".into()))));
        assert_eq!(command.get_args().next().unwrap(), "gui");
        assert_eq!(
            relaunch_command(Path::new("/x"), None).get_envs().count(),
            0
        );
        assert!(relaunch(Path::new("/no/such/program"), None, quick()).is_err());
    }

    /// a short grace period, so the tests do not wait for two seconds.
    fn quick() -> Grace {
        Grace {
            total: Duration::from_millis(250),
            poll: Duration::from_millis(10),
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lanlord-core-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// write an executable script. tests run in parallel and start processes, and a process
    /// forked while this one holds the script open for writing would make running it fail with
    /// "text file busy"; so the script is written beside its place and `install` copies it in,
    /// and this process never has the program itself open for writing.
    fn script(path: &Path, body: &str) {
        let parent = path.parent().unwrap();
        std::fs::create_dir_all(parent).unwrap();
        let source = parent.join(format!(
            ".{}.src",
            path.file_name().unwrap().to_string_lossy()
        ));
        std::fs::write(&source, format!("#!/bin/sh\n{body}\n")).unwrap();
        let installed = std::process::Command::new("install")
            .args(["-m", "0755"])
            .arg(&source)
            .arg(path)
            .status()
            .unwrap();
        assert!(installed.success(), "install {}", path.display());
        let _ = std::fs::remove_file(source);
    }

    #[test]
    fn a_missing_checkout_is_caught_before_anything_runs() {
        let dir = scratch("gone");
        let exe = dir.join("target/debug/lanlord");
        let err = preflight_with(&dir.join("not-here"), &exe, false, OsStr::new("")).unwrap_err();
        assert!(err.to_string().contains("is gone"), "{err}");
        assert!(err.to_string().contains("nothing was changed"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_cargo_or_script_is_caught_with_a_hint() {
        let dir = scratch("tools");
        let exe = dir.join("target/debug/lanlord");
        // no cargo on an empty path
        let err = preflight_with(&dir, &exe, false, OsStr::new("")).unwrap_err();
        assert!(err.to_string().contains("cargo was not found"), "{err}");
        assert!(err.to_string().contains("rustup"), "{err}");
        // with a cargo on the path the check passes, and a pull needs git too
        let bin = dir.join("bin");
        script(&bin.join("cargo"), "exit 0");
        assert!(preflight_with(&dir, &exe, false, bin.as_os_str()).is_ok());
        let err = preflight_with(&dir, &exe, true, bin.as_os_str()).unwrap_err();
        assert!(err.to_string().contains("git was not found"), "{err}");
        // an installed app needs its build script
        let app = dir.join("Fake.app/Contents/MacOS/lanlord-bin");
        let err = preflight_with(&dir, &app, false, bin.as_os_str()).unwrap_err();
        assert!(
            err.to_string().contains("build-app.sh was not found"),
            "{err}"
        );
        script(&dir.join("tools/build-app.sh"), "exit 0");
        assert!(preflight_with(&dir, &app, false, bin.as_os_str()).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_build_reports_the_last_lines_it_printed() {
        let dir = scratch("build");
        let step = Step {
            program: "sh".into(),
            args: vec!["-c".into(), "echo compiling; echo boom >&2; exit 2".into()],
            dir: dir.clone(),
        };
        let err = rebuild(&[step]).unwrap_err().to_string();
        assert!(
            err.contains("failed") && err.contains("exit status: 2"),
            "{err}"
        );
        assert!(err.contains("boom") && err.contains("compiling"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_build_that_is_not_there_afterwards_is_an_error() {
        let dir = scratch("built");
        let missing = verify_built(&dir.join("lanlord")).unwrap_err();
        assert!(missing.to_string().contains("is missing"), "{missing}");
        let folder = verify_built(&dir).unwrap_err();
        assert!(folder.to_string().contains("not a program"), "{folder}");
        let plain = dir.join("plain");
        std::fs::write(&plain, "x").unwrap();
        let err = verify_built(&plain).unwrap_err();
        assert!(err.to_string().contains("cannot be run"), "{err}");
        let ok = dir.join("ok");
        script(&ok, "exit 0");
        assert!(verify_built(&ok).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_build_that_cannot_be_started_is_an_error() {
        let dir = scratch("spawn");
        // a folder, a file that is not executable and a name that does not exist
        for exe in [dir.clone(), dir.join("plain"), dir.join("nope")] {
            if exe.ends_with("plain") {
                std::fs::write(&exe, "x").unwrap();
            }
            let err = relaunch(&exe, None, quick()).unwrap_err().to_string();
            assert!(err.contains("could not start"), "{err}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_build_that_exits_at_once_keeps_the_old_one_open_and_says_why() {
        let dir = scratch("exits");
        let exe = dir.join("lanlord");
        script(&exe, "echo cannot open display >&2; exit 3");
        let err = relaunch(&exe, None, quick()).unwrap_err().to_string();
        assert!(err.contains("exited at once"), "{err}");
        assert!(err.contains("exit status: 3"), "{err}");
        assert!(err.contains("cannot open display"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_build_that_stays_alive_is_a_success_and_runs_in_its_own_group() {
        let dir = scratch("alive");
        let exe = dir.join("lanlord");
        let pgid = dir.join("pgid");
        script(
            &exe,
            &format!("ps -o pgid= -p $$ > '{}'\nsleep 1", pgid.display()),
        );
        relaunch(&exe, Some("me@gw"), quick()).unwrap();
        let theirs = std::fs::read_to_string(&pgid).unwrap();
        let ours = Command::new("ps")
            .args(["-o", "pgid=", "-p", &std::process::id().to_string()])
            .output()
            .unwrap();
        assert_ne!(theirs.trim(), String::from_utf8_lossy(&ours.stdout).trim());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn install_build_stops_at_the_first_problem_and_succeeds_when_all_is_well() {
        let dir = scratch("install");
        let app = dir.join("Fake.app/Contents/MacOS/lanlord-bin");
        // no checkout at all
        let gone = install_build(&dir.join("x"), &app, None, false, quick()).unwrap_err();
        assert!(gone.to_string().contains("is gone"), "{gone}");
        // a build script that works but leaves no program behind
        script(&dir.join("tools/build-app.sh"), "exit 0");
        let missing = install_build(&dir, &app, None, false, quick()).unwrap_err();
        assert!(missing.to_string().contains("is missing"), "{missing}");
        // a program that stays up
        script(&app, "sleep 1");
        assert!(install_build(&dir, &app, None, false, quick()).is_ok());
        // a program that dies at once
        script(&app, "exit 1");
        let died = install_build(&dir, &app, None, false, quick()).unwrap_err();
        assert!(died.to_string().contains("exited at once"), "{died}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn programs_are_found_on_a_path_or_by_name_with_a_separator() {
        let dir = scratch("find");
        let tool = dir.join("bin/tool");
        script(&tool, "exit 0");
        let path = dir.join("bin");
        assert_eq!(find_program("tool", path.as_os_str()), Some(tool.clone()));
        assert_eq!(find_program("other", path.as_os_str()), None);
        assert_eq!(
            find_program(tool.to_str().unwrap(), OsStr::new("")),
            Some(tool)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_step_can_be_stopped_and_takes_what_it_started_with_it() {
        let dir = scratch("stop");
        let pid = dir.join("pid");
        let step = Step {
            program: "sh".into(),
            args: vec![
                "-c".into(),
                format!("sleep 5 & echo $! > '{}'; wait", pid.display()),
            ],
            dir: dir.clone(),
        };
        let cancel = Cancel::default();
        let beat: Beat = Arc::new(|| {});
        let stopper = cancel.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            stopper.cancel();
        });
        let started = Instant::now();
        let result = run_step_with(&step, &cancel, &beat);
        handle.join().unwrap();
        assert!(matches!(result, Err(StepFault::Cancelled)), "{result:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "it stopped promptly"
        );
        // the sleep it started is gone too
        let child = std::fs::read_to_string(&pid).unwrap();
        let mut gone = false;
        for _ in 0..40 {
            let alive = Command::new("kill")
                .args(["-0", child.trim()])
                .stderr(Stdio::null())
                .status()
                .unwrap();
            if !alive.success() {
                gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(gone, "the process group was stopped");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn every_line_a_step_prints_is_a_beat() {
        let dir = scratch("beats");
        let step = Step {
            program: "sh".into(),
            args: vec!["-c".into(), "echo one; echo two >&2; echo three".into()],
            dir: dir.clone(),
        };
        let count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter = Arc::clone(&count);
        let beat: Beat = Arc::new(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        run_step_with(&step, &Cancel::default(), &beat).unwrap();
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 3);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn network_trouble_is_told_apart_from_other_failures() {
        for text in [
            "fatal: unable to access 'https://github.com/x/': Could not resolve host: github.com",
            "ssh: connect to host github.com port 22: Network is unreachable",
            "fatal: Could not read from remote repository.",
        ] {
            assert!(is_network_error(text), "{text}");
        }
        assert!(!is_network_error("error: could not compile `lanlord`"));
        assert!(!is_network_error(
            "fatal: Not possible to fast-forward, aborting."
        ));
    }

    #[test]
    fn an_interrupted_pull_or_build_is_noticed() {
        let dir = scratch("half");
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap()
        };
        if !git(&["init", "-q"]).status.success() {
            return; // no git here, nothing to check
        }
        assert_eq!(checkout_problem(&dir), None);
        std::fs::write(dir.join(".git/index.lock"), "").unwrap();
        let problem = checkout_problem(&dir).unwrap();
        assert!(
            problem.contains("index.lock") && problem.contains("half-finished"),
            "{problem}"
        );
        std::fs::remove_file(dir.join(".git/index.lock")).unwrap();
        std::fs::write(dir.join(".git/MERGE_HEAD"), "").unwrap();
        assert!(checkout_problem(&dir).unwrap().contains("merge"));
        // not a checkout at all: nothing to say
        let plain = scratch("plain");
        assert_eq!(checkout_problem(&plain), None);
        // a half-built app next to the installed one
        let exe = dir.join("Apps/LANlord.app/Contents/MacOS/lanlord-bin");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        assert_eq!(app_leftover(&exe), None);
        std::fs::create_dir_all(dir.join("Apps/LANlord.app.tmp.4242")).unwrap();
        let left = app_leftover(&exe).unwrap();
        assert!(
            left.contains("half-built") && left.contains("not touched"),
            "{left}"
        );
        assert_eq!(app_leftover(&dir.join("target/debug/lanlord")), None);
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(plain);
    }

    #[test]
    fn only_release_pages_of_this_project_are_opened() {
        assert!(open_release("https://example.com/x").is_err());
        assert!(open_release("file:///etc/passwd").is_err());
        assert!(open_release("https://github.com/muncherelli/LANlord-evil/x").is_err());
    }

    #[test]
    fn dev_mode_needs_a_git_checkout() {
        // the repo this test runs in is a checkout (or a worktree); a bogus override is not
        assert!(dev_repo_for(Some(PathBuf::from("/no/such/place"))).is_none());
        let here = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap();
        assert_eq!(
            dev_repo_for(Some(here.to_path_buf())),
            Some(here.to_path_buf())
        );
        assert!(dev_repo_for(None).is_some());
        let fake = fake();
        assert_eq!(fake.source, Source::Git);
    }
}
