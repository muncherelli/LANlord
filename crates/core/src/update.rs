//! knowing whether a newer lanlord exists, and rebuilding and relaunching a development build.
//! the git and http calls go through a `Runner`, so tests use a fake one.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
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

/// the update notice's content when LANLORD_FAKE_UPDATE=1, to look at it without a network.
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
    if std::env::var("LANLORD_FAKE_UPDATE").as_deref() == Ok("1") {
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
        args: args.iter().map(|a| a.to_string()).collect(),
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

/// run `step`, sending every line it prints to the console as it comes.
pub fn run_step(step: &Step) -> Result<()> {
    emit(
        Kind::Info,
        format!("$ {} {}", step.program, step.args.join(" ")),
    );
    let mut child = Command::new(&step.program)
        .args(&step.args)
        .current_dir(&step.dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("could not run {}", step.program))?;
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
        readers.push(std::thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                emit(Kind::Info, line);
            }
        }));
    }
    let status = child.wait()?;
    for reader in readers {
        let _ = reader.join();
    }
    if !status.success() {
        bail!("{} failed ({status})", step.program);
    }
    Ok(())
}

/// run the whole rebuild, stopping at the first failure.
pub fn rebuild(steps: &[Step]) -> Result<()> {
    for step in steps {
        run_step(step)?;
    }
    emit(Kind::Info, "rebuilt; starting the new build");
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

/// start the rebuilt build; this process is expected to quit right after.
pub fn relaunch(exe: &Path, host: Option<&str>) -> Result<()> {
    relaunch_command(exe, host)
        .spawn()
        .with_context(|| format!("could not start {}", exe.display()))?;
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
        assert!(relaunch(Path::new("/no/such/program"), None).is_err());
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
