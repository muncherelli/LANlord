//! runs a command on a domain machine through `lanlord-exec` in the nt4 container: the request
//! (with the exec account's password) goes in on stdin, and the run comes back as event lines.

use anyhow::{anyhow, bail, Result};
use base64::Engine;
use zeroize::Zeroize;

use crate::bootstrap::{root_argv, AskPassword};
use crate::config;
use crate::engine::host::Host;
use crate::engine::modules::nt4::{container_name, exec_targets, remember_exec};
use crate::leases;
use crate::log::{emit, Kind};
use crate::settings;
use crate::ssh::Session;
use crate::users::cached;

pub use lanlord_model::nt4_exec::*;

/// the sealed secret holding its password: `exec_password` in the `nt4-domain` section.
pub const SECRET: &str = "nt4-domain.exec_password";
/// each stream of a run keeps this many bytes; the rest is dropped and `truncated` says so.
pub const OUTPUT_CAP: usize = 1024 * 1024;
/// the message when the account has no password yet.
pub const NOT_SET_UP: &str = "the exec account is not set up yet; run apply";

/// the module whose apply makes the account.
pub const MODULE: &str = "nt4-domain";

const MARKER: &str = "LANLORD_EVENT ";
/// what the script says when the name does not resolve; the lease fallback answers it.
const NO_ANSWER: &str = "does not answer to its name";

/// one line the script printed after the marker.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// new bytes of the command's output; `err` is stderr.
    Output {
        err: bool,
        data: Vec<u8>,
    },
    Exit(i32),
    TimedOut,
    Error(String),
}

/// the event on `line`, or `None` for any other line (docker's own messages) and for one that
/// is not well formed.
pub fn parse_event(line: &str) -> Option<Event> {
    let json = line.trim_end().strip_prefix(MARKER)?;
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    if let Some(stream) = value.get("stream").and_then(|s| s.as_str()) {
        let data = value.get("data")?.as_str()?;
        let data = base64::engine::general_purpose::STANDARD
            .decode(data)
            .ok()?;
        return match stream {
            "out" => Some(Event::Output { err: false, data }),
            "err" => Some(Event::Output { err: true, data }),
            _ => None,
        };
    }
    if let Some(code) = value.get("exit").and_then(|c| c.as_i64()) {
        // a code outside i32 is clamped, so the run still reports a (non-zero) result
        let code = i32::try_from(code).unwrap_or(if code < 0 { i32::MIN } else { i32::MAX });
        return Some(Event::Exit(code));
    }
    if value.get("timed_out").and_then(|t| t.as_bool()) == Some(true) {
        return Some(Event::TimedOut);
    }
    value
        .get("error")
        .and_then(|e| e.as_str())
        .map(|e| Event::Error(e.to_string()))
}

/// what a run did. the output is raw (the machine's oem code page); `stdout_text` and
/// `stderr_text` decode it for display.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// `None` when the run timed out.
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub truncated: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl Outcome {
    pub fn stdout_text(&self) -> String {
        decode_cp437(&redact_secrets(&self.stdout))
    }

    pub fn stderr_text(&self) -> String {
        decode_cp437(&redact_secrets(&self.stderr))
    }
}

/// gathers the events of one run.
#[derive(Default)]
pub struct Collector {
    outcome: Outcome,
    error: Option<String>,
    finished: bool,
    /// the last lines that were not events (docker's own messages), for an error.
    notes: Vec<String>,
}

/// how many non-event lines, and how many bytes of them, an error may quote.
const NOTE_LINES: usize = 20;
const NOTE_BYTES: usize = 4096;

impl Collector {
    /// take one output line; returns the stream and the bytes that were kept, for live display.
    pub fn feed(&mut self, line: &str) -> Option<(bool, Vec<u8>)> {
        let Some(rest) = line.strip_prefix(MARKER) else {
            self.note(line);
            return None;
        };
        // both streams are full: skip the decoding of what would be dropped anyway
        if self.is_full() && rest.starts_with(r#"{"stream""#) {
            self.outcome.truncated = true;
            return None;
        }
        let Some(event) = parse_event(line) else {
            // an event that cannot be read (cut short, say) may have held output
            self.outcome.truncated = true;
            return None;
        };
        match event {
            Event::Output { err, data } => {
                let kept = if err {
                    &mut self.outcome.stderr
                } else {
                    &mut self.outcome.stdout
                };
                let room = OUTPUT_CAP.saturating_sub(kept.len());
                if data.len() > room {
                    self.outcome.truncated = true;
                }
                let take = &data[..data.len().min(room)];
                kept.extend_from_slice(take);
                (!take.is_empty()).then(|| (err, take.to_vec()))
            }
            Event::Exit(code) => {
                self.outcome.exit_code = Some(code);
                self.finished = true;
                None
            }
            Event::TimedOut => {
                self.outcome.timed_out = true;
                self.finished = true;
                None
            }
            Event::Error(message) => {
                self.error = Some(message);
                None
            }
        }
    }

    /// true when both streams hold as much as they may.
    pub fn is_full(&self) -> bool {
        self.outcome.stdout.len() >= OUTPUT_CAP && self.outcome.stderr.len() >= OUTPUT_CAP
    }

    fn note(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        self.notes.push(line.chars().take(512).collect());
        while self.notes.len() > NOTE_LINES
            || self.notes.iter().map(|n| n.len() + 1).sum::<usize>() > NOTE_BYTES
        {
            self.notes.remove(0);
        }
    }

    /// the outcome; an error event, or a stream that ended with no final event, is an error.
    pub fn finish(self, docker_exit: i32) -> Result<Outcome> {
        if let Some(message) = self.error {
            bail!("{message}");
        }
        if !self.finished {
            let said = if self.notes.is_empty() {
                String::new()
            } else {
                format!(": {}", self.notes.join(" | "))
            };
            bail!("the run ended without a result (docker exited with {docker_exit}){said}");
        }
        Ok(self.outcome)
    }
}

/// the arguments of the run; the password is never among them, it goes on stdin.
pub fn argv(
    container: &str,
    machine: &str,
    address: Option<&str>,
    timeout_secs: u32,
) -> Vec<String> {
    let mut argv: Vec<String> = [
        "docker",
        "exec",
        "-i",
        container,
        "lanlord-exec",
        "run",
        "--machine",
        machine,
    ]
    .map(String::from)
    .into();
    if let Some(address) = address {
        argv.extend(["--address".into(), address.into()]);
    }
    argv.extend(["--timeout".into(), timeout_secs.to_string()]);
    argv
}

/// the caller's name as the script takes it: other characters become `_`, at most 64 are kept,
/// and an empty name is `cli`.
fn audit_name(caller: &str) -> String {
    let name: String = caller
        .chars()
        .take(64)
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._@-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.is_empty() {
        "cli".into()
    } else {
        name
    }
}

/// the json request for stdin. it is written straight into one buffer that is big enough for
/// the worst case, so no copy of the password is left behind by a regrowing buffer.
fn request(password: &str, command: &str, caller: &str) -> Vec<u8> {
    #[derive(serde::Serialize)]
    struct Request<'a> {
        user: &'a str,
        password: &'a str,
        command: &'a str,
        caller: &'a str,
    }
    // an escaped character takes at most six bytes
    let mut body = Vec::with_capacity(64 + 6 * (password.len() + command.len() + caller.len()));
    // writing to a vec cannot fail
    let _ = serde_json::to_writer(
        &mut body,
        &Request {
            user: USER,
            password,
            command,
            caller,
        },
    );
    body
}

/// a new random password for the account: 24 characters of the alphabet that is easy to type
/// and that NT4 and its tools take as they are (no quotes, percent signs, backslashes or
/// spaces), from the operating system's random source.
pub fn new_password() -> String {
    crate::passwords::generate(PASSWORD_LEN)
}

/// the arguments that set the account's password in `container`; the password goes on stdin.
pub fn account_argv(container: &str) -> Vec<String> {
    ["docker", "exec", "-i", container, "lanlord-user", "exec"]
        .map(String::from)
        .into()
}

/// make the account's password in the sealed secrets when the settings have a domain and none is
/// there yet. it is stored before the apply that uses it. returns whether one was made.
pub(crate) fn ensure_password(
    session: &Session,
    host: &str,
    identity: &age::x25519::Identity,
    ask: AskPassword,
) -> Result<bool> {
    let mut ask = cached(ask);
    let Some(mut whole) = settings::whole_with(session, identity, &mut ask)? else {
        return Ok(false);
    };
    let wanted = wants_account(&whole);
    let set = whole
        .get("nt4-domain")
        .and_then(|section| section.get("exec_password"))
        .and_then(|text| text.as_str())
        .is_some_and(|text| !text.is_empty());
    wipe(&mut whole);
    if !wanted || set {
        return Ok(false);
    }
    let mut password = new_password();
    let sealed = settings::seal_secret(session, host, identity, SECRET, &password, &mut ask);
    password.zeroize();
    sealed?;
    Ok(true)
}

/// make a new password, seal it and set it on the account, and do nothing else. it is sealed
/// first, so a failed set leaves the secret ahead of the account and the next apply (or another
/// rotate) brings the account to it, never a password nobody has.
///
/// it sets the account on each domain controller of the list and no other server. on the
/// gateway the engine's record of the password is updated too, so the next plan or apply does not
/// send it again.
pub(crate) fn rotate(
    session: &Session,
    host: &str,
    identity: &age::x25519::Identity,
    ask: AskPassword,
) -> Result<()> {
    let engine = crate::engine::host::RealHost;
    let state = session
        .is_local()
        .then_some((&engine as &dyn Host, crate::engine::STATE_DIR));
    rotate_with(session, host, identity, ask, state)
}

fn rotate_with(
    session: &Session,
    host: &str,
    identity: &age::x25519::Identity,
    ask: AskPassword,
    state: Option<(&dyn Host, &str)>,
) -> Result<()> {
    let mut ask = cached(ask);
    let Some(mut whole) = settings::whole_with(session, identity, &mut ask)? else {
        bail!("no settings on {host}; run lanlord config push first");
    };
    let targets = exec_targets(&whole);
    wipe(&mut whole);
    if targets.is_empty() {
        bail!("{host} has no domain controller, so there is no exec account to rotate");
    }
    let mut password = new_password();
    let done = settings::seal_secret(session, host, identity, SECRET, &password, &mut ask)
        .and_then(|()| {
            targets.iter().try_for_each(|target| {
                set_account(session, &mut ask, &target.container, &password)?;
                match state {
                    Some((engine, dir)) => remember_exec(engine, dir, &target.key, &password),
                    None => Ok(()),
                }
            })
        });
    password.zeroize();
    done
}

/// set the account's password in `container`, over stdin.
fn set_account(session: &Session, ask: AskPassword, container: &str, password: &str) -> Result<()> {
    let admin = config::admin(session)?;
    let command = account_argv(container);
    let refs: Vec<&str> = command.iter().map(String::as_str).collect();
    let (remote, first) = root_argv(session, &admin, &refs, ask)?;
    let mut input = first.into_bytes();
    input.extend_from_slice(password.as_bytes());
    input.push(b'\n');
    let result = session.capture_with_stdin(&remote, &input);
    input.zeroize();
    result
        .map(|_| ())
        .map_err(|_| anyhow!("could not set the exec account's password"))
}

/// the containers of the domain controllers in the settings, one per domain. settings that
/// name none (or no settings yet) mean the first server's container, as before the list.
fn controllers(session: &Session, ask: AskPassword) -> Result<Vec<String>> {
    let mut containers: Vec<String> = match config::pull_plain_with(session, ask)? {
        Some(plain) => exec_targets(&lanlord_model::settings::parse_value(&plain)?)
            .into_iter()
            .map(|target| target.container)
            .collect(),
        None => Vec::new(),
    };
    if containers.is_empty() {
        containers.push(container_name(crate::engine::modules::nt4::LEGACY));
    }
    Ok(containers)
}

/// whether `name` is a container name the engine could have made: `lanlord-nt4`, or
/// `lanlord-nt4-` and a server id of letters, digits and inner hyphens.
fn valid_container(name: &str) -> bool {
    match name.strip_prefix("lanlord-nt4") {
        Some("") => true,
        Some(rest) => rest.strip_prefix('-').is_some_and(|id| {
            id.starts_with(|c: char| c.is_ascii_alphanumeric())
                && id.ends_with(|c: char| c.is_ascii_alphanumeric())
                && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        }),
        None => false,
    }
}

/// the machine accounts of each domain controller's container, without the `$`. a container
/// whose name is not valid, or whose list fails (not applied yet, stopped), is skipped with a
/// warning naming it; it is an error only when no container can be listed or when root access
/// fails.
fn machines_by_container(
    session: &Session,
    ask: AskPassword,
) -> Result<Vec<(String, Vec<String>)>> {
    let admin = config::admin(session)?;
    let mut found = Vec::new();
    let mut last_error = None;
    for container in controllers(session, ask)? {
        if !valid_container(&container) {
            emit(
                Kind::Info,
                format!("warning: skipped {container}: not a container name of the nt4 module"),
            );
            last_error = Some(anyhow!("{container} is not a valid container name"));
            continue;
        }
        // no root access fails the same way for every container: stop at the first error
        let (remote, first) = root_argv(
            session,
            &admin,
            &["docker", "exec", &container, "lanlord-user", "list"],
            ask,
        )?;
        let listed = session.capture_with_stdin(&remote, first.as_bytes());
        match listed {
            Ok(listed) => found.push((container, machine_names(&listed))),
            Err(error) => {
                emit(
                    Kind::Info,
                    format!("warning: could not list the machines of {container}: {error}"),
                );
                last_error = Some(error);
            }
        }
    }
    match (found.is_empty(), last_error) {
        (true, Some(error)) => Err(error),
        _ => Ok(found),
    }
}

/// the container whose domain has the account `machine`; when none has it, the first
/// controller's, so the account tool says what is wrong. a name that is an account in more
/// than one domain is an error, never the first of them.
pub(crate) fn container_of(session: &Session, ask: AskPassword, machine: &str) -> Result<String> {
    let found = machines_by_container(session, ask)?;
    if let Some(container) = holder(&found, machine)? {
        return Ok(container.to_string());
    }
    found
        .first()
        .map(|(container, _)| container.clone())
        .ok_or_else(|| anyhow!("{machine} is not a machine of the domain"))
}

/// the one container that has the account `machine`, if any; two is an error.
fn holder<'a>(found: &'a [(String, Vec<String>)], machine: &str) -> Result<Option<&'a str>> {
    let holders: Vec<&str> = found
        .iter()
        .filter(|(_, names)| names.iter().any(|name| name.eq_ignore_ascii_case(machine)))
        .map(|(container, _)| container.as_str())
        .collect();
    match holders[..] {
        [] => Ok(None),
        [one] => Ok(Some(one)),
        _ => bail!(
            "{machine} is a machine in more than one domain ({}); rename or remove one of the accounts",
            holders.join(", ")
        ),
    }
}

/// the names of the machine accounts of every domain, without the `$`; a name in two domains is
/// listed once, and running on it or resetting it is refused.
pub fn machines(session: &Session, ask: AskPassword) -> Result<Vec<String>> {
    let mut names: Vec<String> = Vec::new();
    for name in machines_by_container(session, ask)?
        .into_iter()
        .flat_map(|(_, names)| names)
    {
        if !names.iter().any(|known| known.eq_ignore_ascii_case(&name)) {
            names.push(name);
        }
    }
    Ok(names)
}

/// the machine accounts in `pdbedit -L` output (`name$:uid:full name`).
fn machine_names(listed: &str) -> Vec<String> {
    listed
        .lines()
        .filter_map(|line| line.split(':').next())
        .filter_map(|name| name.strip_suffix('$'))
        .filter(|name| !name.is_empty())
        .map(String::from)
        .collect()
}

/// the exec account's password from the sealed secrets.
fn password(
    session: &Session,
    identity: &age::x25519::Identity,
    ask: AskPassword,
) -> Result<String> {
    let mut sealed =
        config::pull_with(session, identity, ask)?.ok_or_else(|| anyhow!(NOT_SET_UP))?;
    let parsed = lanlord_model::settings::parse_value(&sealed);
    sealed.zeroize();
    let mut value = parsed?;
    // read by name: the secret key list is the model's, and this does not depend on it
    let found = value
        .get("nt4-domain")
        .and_then(|section| section.get("exec_password"))
        .and_then(|text| text.as_str())
        .filter(|text| !text.is_empty())
        .map(String::from);
    wipe(&mut value);
    found.ok_or_else(|| anyhow!(NOT_SET_UP))
}

/// overwrite every text in `value`, best effort, before it is dropped.
fn wipe(value: &mut serde_yaml_ng::Value) {
    use serde_yaml_ng::Value;
    match value {
        Value::String(text) => text.zeroize(),
        Value::Sequence(items) => items.iter_mut().for_each(wipe),
        Value::Mapping(map) => map.iter_mut().for_each(|(_, v)| wipe(v)),
        Value::Tagged(tagged) => wipe(&mut tagged.value),
        _ => {}
    }
}

/// what to run: the machine, the command line, the timeout in seconds and the caller's name for
/// the audit log (`cli` when there is none).
pub type Job<'a> = (&'a str, &'a str, u32, &'a str);

/// run a job, handing the output to `on_chunk` (true for stderr) as it comes. the machine must
/// be an account of the domain. a name that does not resolve is tried again at the address of
/// its dhcp lease.
pub fn run(
    session: &Session,
    identity: &age::x25519::Identity,
    ask: AskPassword,
    (machine, command, timeout_secs, caller): Job,
    on_chunk: &mut dyn FnMut(bool, &[u8]),
) -> Result<Outcome> {
    let mut ask = cached(ask);
    let known = machines_by_container(session, &mut ask)?;
    let Some(container) = holder(&known, machine)? else {
        bail!("{machine} is not a machine of the domain");
    };
    let machine = known
        .iter()
        .flat_map(|(_, names)| names)
        .find(|name| name.eq_ignore_ascii_case(machine))
        .expect("the holder lists the machine");
    let mut secret = password(session, identity, &mut ask)?;
    let mut body = request(&secret, command, &audit_name(caller));
    secret.zeroize();
    let run = |address: Option<&str>, ask: AskPassword, on_chunk: &mut dyn FnMut(bool, &[u8])| {
        run_request(
            session,
            ask,
            (container, machine),
            &body,
            timeout_secs,
            address,
            on_chunk,
        )
    };
    let mut result = run(None, &mut ask, on_chunk);
    if result
        .as_ref()
        .is_err_and(|error| error.to_string().contains(NO_ANSWER))
    {
        if let Some(address) = lease_address(session, &mut ask, machine) {
            result = run(Some(&address), &mut ask, on_chunk);
        }
    }
    body.zeroize();
    result
}

/// the address a dhcp lease gives `machine`, if any.
fn lease_address(session: &Session, ask: AskPassword, machine: &str) -> Option<String> {
    leases::read_with(session, ask)
        .ok()?
        .into_iter()
        .find(|lease| lease.name.eq_ignore_ascii_case(machine))
        .map(|lease| lease.address)
}

fn run_request(
    session: &Session,
    ask: AskPassword,
    (container, machine): (&str, &str),
    body: &[u8],
    timeout_secs: u32,
    address: Option<&str>,
    on_chunk: &mut dyn FnMut(bool, &[u8]),
) -> Result<Outcome> {
    let admin = config::admin(session)?;
    let command = argv(container, machine, address, timeout_secs);
    let refs: Vec<&str> = command.iter().map(String::as_str).collect();
    let (remote, first) = root_argv(session, &admin, &refs, ask)?;
    let mut input = first.into_bytes();
    input.extend_from_slice(body);
    let mut collector = Collector::default();
    // a secret property is hidden in the live output too; a line is held until it is complete
    let mut live = [LiveRedactor::default(), LiveRedactor::default()];
    let code = session.run_with_stdin_filtered(&remote, &input, &mut |line| {
        if let Some((err, bytes)) = collector.feed(line) {
            let shown = live[usize::from(err)].push(&bytes);
            if !shown.is_empty() {
                on_chunk(err, &shown);
            }
        }
        // output events are never kept in the debug log: they hold the raw output, secrets
        // included, as base64
        !line.starts_with(concat!("LANLORD_EVENT ", r#"{"stream""#))
    });
    input.zeroize();
    for (err, redactor) in live.iter_mut().enumerate() {
        let rest = redactor.finish();
        if !rest.is_empty() {
            on_chunk(err == 1, &rest);
        }
    }
    collector.finish(code?)
}

#[cfg(all(test, unix))]
pub(crate) mod tests {
    use super::*;
    use crate::testgw::no_password;

    pub(crate) fn event(json: &str) -> String {
        format!("{MARKER}{json}")
    }

    #[test]
    fn events_are_parsed() {
        assert_eq!(
            parse_event(&event(r#"{"stream":"out","data":"aGk="}"#)),
            Some(Event::Output {
                err: false,
                data: b"hi".to_vec()
            })
        );
        assert_eq!(
            parse_event(&event(r#"{"stream":"err","data":"gQ=="}"#)),
            Some(Event::Output {
                err: true,
                data: vec![0x81]
            })
        );
        assert_eq!(parse_event(&event(r#"{"exit":3}"#)), Some(Event::Exit(3)));
        assert_eq!(
            parse_event(&event(r#"{"timed_out":true}"#)),
            Some(Event::TimedOut)
        );
        assert_eq!(
            parse_event(&event(r#"{"error":"nope"}"#)),
            Some(Event::Error("nope".into()))
        );
    }

    #[test]
    fn malformed_lines_are_ignored() {
        for line in [
            "",
            "Error response from daemon",
            "LANLORD_EVENT",
            "LANLORD_EVENT not json",
            r#"LANLORD_EVENT {"stream":"out","data":"!!!"}"#,
            r#"LANLORD_EVENT {"stream":"out"}"#,
            r#"LANLORD_EVENT {"stream":"other","data":"aGk="}"#,
            r#"LANLORD_EVENT {"exit":"x"}"#,
            r#"LANLORD_EVENT {"timed_out":false}"#,
            r#"LANLORD_EVENT []"#,
        ] {
            assert_eq!(parse_event(line), None, "{line}");
        }
    }

    #[test]
    fn a_run_collects_output_and_the_exit_code() {
        let mut collector = Collector::default();
        assert_eq!(
            collector.feed(&event(r#"{"stream":"out","data":"gQ=="}"#)),
            Some((false, vec![0x81]))
        );
        assert_eq!(collector.feed("noise"), None);
        collector.feed(&event(r#"{"stream":"err","data":"ZQ=="}"#));
        collector.feed(&event(r#"{"exit":3}"#));
        let outcome = collector.finish(0).unwrap();
        assert_eq!(outcome.exit_code, Some(3));
        // cp437 0x81 is u with diaeresis
        assert_eq!(outcome.stdout_text(), "\u{fc}");
        assert_eq!(outcome.stderr_text(), "e");
        assert!(!outcome.timed_out && !outcome.truncated);
    }

    #[test]
    fn an_event_that_cannot_be_read_marks_the_output_as_cut() {
        let mut collector = Collector::default();
        assert_eq!(collector.feed("noise"), None);
        collector.feed(&event(r#"{"exit":0}"#));
        assert!(!collector.finish(0).unwrap().truncated);
        let mut collector = Collector::default();
        // cut in the middle of its json
        assert_eq!(collector.feed(r#"LANLORD_EVENT {"stream":"out","da"#), None);
        collector.feed(&event(r#"{"exit":0}"#));
        assert!(collector.finish(0).unwrap().truncated);
    }

    #[test]
    fn a_timeout_has_no_exit_code() {
        let mut collector = Collector::default();
        collector.feed(&event(r#"{"timed_out":true}"#));
        let outcome = collector.finish(0).unwrap();
        assert!(outcome.timed_out);
        assert_eq!(outcome.exit_code, None);
    }

    #[test]
    fn no_final_event_is_an_error() {
        let mut collector = Collector::default();
        collector.feed(&event(r#"{"stream":"out","data":"aGk="}"#));
        let error = collector.finish(1).unwrap_err().to_string();
        assert!(error.contains("without a result"), "{error}");
        assert!(error.contains('1'), "{error}");
    }

    #[test]
    fn an_error_event_is_the_error() {
        let mut collector = Collector::default();
        collector.feed(&event(r#"{"error":"another command is running"}"#));
        assert_eq!(
            collector.finish(1).unwrap_err().to_string(),
            "another command is running"
        );
    }

    #[test]
    fn output_is_capped_per_stream() {
        let mut collector = Collector::default();
        let chunk = base64::engine::general_purpose::STANDARD.encode(vec![b'x'; OUTPUT_CAP - 1]);
        let line = event(&format!(r#"{{"stream":"out","data":"{chunk}"}}"#));
        assert_eq!(collector.feed(&line).unwrap().1.len(), OUTPUT_CAP - 1);
        // only one byte of room is left
        assert_eq!(collector.feed(&line).unwrap().1.len(), 1);
        assert_eq!(collector.feed(&line), None);
        // stderr has a cap of its own
        assert!(collector
            .feed(&event(r#"{"stream":"err","data":"aGk="}"#))
            .is_some());
        collector.feed(&event(r#"{"exit":0}"#));
        let outcome = collector.finish(0).unwrap();
        assert_eq!(outcome.stdout.len(), OUTPUT_CAP);
        assert!(outcome.truncated);
    }

    #[test]
    fn the_password_is_never_an_argument() {
        let secret = "s3cretpw";
        for address in [None, Some("192.0.2.7")] {
            let argv = argv("lanlord-nt4", "WS1", address, 30);
            assert!(argv.iter().all(|a| !a.contains(secret)), "{argv:?}");
            assert_eq!(
                argv[..6],
                ["docker", "exec", "-i", "lanlord-nt4", "lanlord-exec", "run"]
            );
            assert_eq!(argv[argv.len() - 2..], ["--timeout", "30"]);
        }
        let body = String::from_utf8(request(secret, "ver", "ann")).unwrap();
        assert!(body.contains(secret) && body.contains(USER));
        assert!(body.contains(r#""caller":"ann""#), "{body}");
    }

    #[test]
    fn the_audit_name_is_made_plain() {
        assert_eq!(audit_name("ann.lee@demo-1"), "ann.lee@demo-1");
        assert_eq!(audit_name(""), "cli");
        assert_eq!(audit_name("a b\"c\nd"), "a_b_c_d");
        assert_eq!(audit_name(&"a".repeat(80)), "a".repeat(64));
    }

    const DOMAIN: &str =
        "nt4-servers:\n  - {name: a, role: pdc, domain: LAB, netbios_name: ONE, network: lab}\ndomains:\n  LAB: {}\n";
    const WORKGROUP: &str =
        "nt4-servers:\n  - {name: a, role: workgroup, workgroup: WG, netbios_name: ONE, network: lab}\n";

    fn sealed_password(gw: &crate::testgw::Gateway, id: &age::x25519::Identity) -> Option<String> {
        let sealed = gw.sealed(id)?;
        let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&sealed).unwrap();
        value["nt4-domain"]["exec_password"]
            .as_str()
            .map(String::from)
    }

    #[test]
    fn a_new_password_is_24_characters_nt4_takes_and_never_repeats() {
        let seen: std::collections::HashSet<String> = (0..50).map(|_| new_password()).collect();
        assert_eq!(seen.len(), 50);
        for password in &seen {
            assert_eq!(password.len(), 24);
            assert!(password
                .chars()
                .all(|c| crate::passwords::ALPHABET.contains(c)));
            for bad in ['"', '%', '\\', ' ', '\'', '$', '^', '&', '|', '<', '>'] {
                assert!(!password.contains(bad), "{password}");
            }
        }
    }

    #[test]
    fn the_account_argv_never_holds_a_password() {
        let argv = account_argv("lanlord-nt4");
        assert_eq!(argv.last().map(String::as_str), Some("exec"));
        assert!(argv.iter().all(|a| !a.contains(&new_password())));
        assert!(argv.contains(&"-i".to_string()));
    }

    #[test]
    fn a_domain_without_a_password_gets_one_sealed_and_not_in_the_plain_settings() {
        use crate::testgw::{identity, no_password, Gateway};
        let (gw, id) = (Gateway::new("exec-ensure"), identity("exec-ensure"));
        gw.seed_plain(DOMAIN);
        assert!(ensure_password(&gw.session, "gw", &id, &mut no_password).unwrap());
        let password = sealed_password(&gw, &id).expect("sealed");
        assert_eq!(password.len(), 24);
        assert!(!gw.plain().unwrap().contains(&password));
        assert!(!gw.plain().unwrap().contains("exec_password"));
        // it is the same one on the next apply: nothing is made twice
        assert!(!ensure_password(&gw.session, "gw", &id, &mut no_password).unwrap());
        assert_eq!(
            sealed_password(&gw, &id).as_deref(),
            Some(password.as_str())
        );
    }

    #[test]
    fn other_sealed_secrets_survive_the_new_one() {
        use crate::testgw::{identity, no_password, Gateway};
        let (gw, id) = (Gateway::new("exec-keep"), identity("exec-keep"));
        gw.seed_plain(DOMAIN);
        gw.seed_sealed(&id, "nt4-domain:\n  admin_password: s3cretadmin\n");
        assert!(ensure_password(&gw.session, "gw", &id, &mut no_password).unwrap());
        let sealed = gw.sealed(&id).unwrap();
        assert!(sealed.contains("s3cretadmin") && sealed.contains("exec_password"));
    }

    #[test]
    fn a_workgroup_or_empty_gateway_makes_no_account() {
        use crate::testgw::{identity, no_password, Gateway};
        let (gw, id) = (Gateway::new("exec-wg"), identity("exec-wg"));
        assert!(!ensure_password(&gw.session, "gw", &id, &mut no_password).unwrap());
        gw.seed_plain(WORKGROUP);
        assert!(!ensure_password(&gw.session, "gw", &id, &mut no_password).unwrap());
        assert!(gw.sealed(&id).is_none());
        let error = rotate(&gw.session, "gw", &id, &mut no_password)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no domain controller"), "{error}");
        assert!(gw.exec_calls().0.is_empty());
    }

    #[test]
    fn rotate_seals_a_new_password_and_sets_only_the_account() {
        use crate::testgw::{identity, no_password, Gateway};
        let (gw, id) = (Gateway::new("exec-rotate"), identity("exec-rotate"));
        gw.seed_plain(DOMAIN);
        ensure_password(&gw.session, "gw", &id, &mut no_password).unwrap();
        let old = sealed_password(&gw, &id).unwrap();
        rotate(&gw.session, "gw", &id, &mut no_password).unwrap();
        let new = sealed_password(&gw, &id).unwrap();
        assert_ne!(old, new);
        assert_eq!(new.len(), 24);
        let (argv, stdin) = gw.exec_calls();
        assert_eq!(argv.len(), 1, "{argv:?}");
        assert!(argv[0].contains("lanlord-user exec"));
        assert!(!argv[0].contains(&new) && !argv[0].contains(&old));
        assert_eq!(stdin, format!("{new}\n").into_bytes());
        // the plain settings were not touched
        assert_eq!(gw.plain().as_deref(), Some(DOMAIN));
    }

    const MIXED: &str = "nt4-servers:\n  - {name: nt4-domain, role: workgroup, workgroup: WG, netbios_name: ONE, network: lab}\n  - {name: lab, role: pdc, domain: LAB, netbios_name: TWO, network: lab}\n  - {name: files, role: fileserver, workgroup: WG, netbios_name: THREE, network: lab}\ndomains:\n  LAB: {}\n";

    #[test]
    fn rotate_sets_the_account_on_each_controller_and_remembers_it_for_the_engine() {
        use crate::engine::host::FakeHost;
        use crate::engine::{state::State, Secret};
        use crate::testgw::{identity, no_password, Gateway};
        let (gw, id) = (Gateway::new("exec-mixed"), identity("exec-mixed"));
        gw.seed_plain(MIXED);
        let engine = FakeHost::new();
        rotate_with(
            &gw.session,
            "gw",
            &id,
            &mut no_password,
            Some((&engine, "/s")),
        )
        .unwrap();
        let new = sealed_password(&gw, &id).unwrap();
        // only the controller's container, named by its id; nothing for the workgroup server
        // that holds the legacy id or the file server
        let (argv, _) = gw.exec_calls();
        assert_eq!(argv.len(), 1, "{argv:?}");
        assert!(argv[0].contains("docker exec -i lanlord-nt4-lab lanlord-user exec"));
        assert!(!argv[0].contains("lanlord-nt4 ") && !argv[0].contains("files"));
        // the engine knows this password is set, and no other
        let state = State::load(&engine, "/s", false).unwrap();
        assert!(state.is_current("lab/exec", &Secret::new(&new)));
        assert!(!state.is_current("lab/exec", &Secret::new("another")));
        assert!(!state.is_current("nt4-domain/exec", &Secret::new(&new)));
        assert!(!engine.text("/s/state.json").unwrap().contains(&new));
    }

    #[test]
    fn rotate_stops_at_a_failing_second_controller_and_remembers_only_the_first() {
        use crate::engine::host::FakeHost;
        use crate::engine::{state::State, Secret};
        use crate::testgw::{identity, no_password, Gateway};
        let two = "nt4-servers:\n  - {name: lab, role: pdc, domain: LAB, netbios_name: ONE, network: lab}\n  - {name: lab2, role: pdc, domain: LABTWO, netbios_name: TWO, network: lab}\ndomains:\n  LAB: {}\n  LABTWO: {}\n";
        let (gw, id) = (Gateway::new("exec-second"), identity("exec-second"));
        gw.seed_plain(two);
        gw.fail_exec_on("lanlord-nt4-lab2 ");
        let engine = FakeHost::new();
        let error = rotate_with(
            &gw.session,
            "gw",
            &id,
            &mut no_password,
            Some((&engine, "/s")),
        )
        .unwrap_err()
        .to_string();
        let new = sealed_password(&gw, &id).unwrap();
        // the error says what failed, not the password, and it does not name the controller
        assert!(error.contains("could not set"), "{error}");
        assert!(!error.contains(&new), "{error}");
        assert!(!error.contains("lab2"), "{error}");
        // the first controller was set and is remembered; the second is not
        let (argv, _) = gw.exec_calls();
        assert_eq!(argv.len(), 1, "{argv:?}");
        assert!(
            argv[0].contains("lanlord-nt4-lab lanlord-user exec"),
            "{argv:?}"
        );
        let state = State::load(&engine, "/s", false).unwrap();
        assert!(state.is_current("lab/exec", &Secret::new(&new)));
        assert!(!state.is_current("lab2/exec", &Secret::new(&new)));
        assert!(!engine.text("/s/state.json").unwrap().contains(&new));
    }

    #[test]
    fn rotate_without_settings_says_so() {
        use crate::testgw::{identity, no_password, Gateway};
        let (gw, id) = (Gateway::new("exec-none"), identity("exec-none"));
        let error = rotate(&gw.session, "gw", &id, &mut no_password)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no settings"), "{error}");
    }

    #[test]
    fn a_failed_set_names_no_password() {
        use crate::testgw::{identity, no_password, Gateway};
        let _ = (Gateway::new("exec-fail"), identity("exec-fail"));
        let session = Session::fake(
            "for a; do last=$a; done\ncase \"$last\" in\n  \"id -un\") echo tester ;;\n  \"sudo -n true\") exit 0 ;;\n  *\"lanlord-user exec\") cat >/dev/null; echo boom >&2; exit 1 ;;\nesac",
        );
        let password = new_password();
        let error = set_account(&session, &mut no_password, "lanlord-nt4", &password)
            .unwrap_err()
            .to_string();
        assert!(!error.contains(&password), "{error}");
        assert!(error.contains("could not set"), "{error}");
    }

    #[test]
    fn every_exec_path_uses_the_container_of_the_machines_domain() {
        use crate::testgw::{identity, no_password, Gateway};
        let (gw, id) = (Gateway::new("exec-two"), identity("exec-two"));
        gw.seed_plain(TWO_DOMAINS);
        gw.seed_accounts("lanlord-nt4", "ws1$:1001:\n");
        gw.seed_accounts("lanlord-nt4-lab", "ws2$:1001:\nlanlord-exec:1002:\n");
        gw.seed_sealed(&id, "nt4-domain:\n  exec_password: pw\n");
        let all = machines(&gw.session, &mut no_password).unwrap();
        assert_eq!(all, ["ws1", "ws2"]);
        run(
            &gw.session,
            &id,
            &mut no_password,
            ("WS2", "ver", 5, "cli"),
            &mut |_, _| {},
        )
        .ok();
        run(
            &gw.session,
            &id,
            &mut no_password,
            ("ws1", "ver", 5, "cli"),
            &mut |_, _| {},
        )
        .ok();
        let calls = gw.calls();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(calls[0].contains("exec -i lanlord-nt4-lab lanlord-exec run --machine ws2"));
        assert!(calls[1].contains("exec -i lanlord-nt4 lanlord-exec run --machine ws1"));
        let error = run(
            &gw.session,
            &id,
            &mut no_password,
            ("files", "ver", 5, "cli"),
            &mut |_, _| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains("not a machine"), "{error}");
        assert_eq!(gw.calls().len(), 2);
    }

    #[test]
    fn no_settings_or_no_controller_means_the_single_domain_container() {
        use crate::testgw::{no_password, Gateway};
        let gw = Gateway::new("exec-single");
        gw.seed_accounts("lanlord-nt4", "ws1$:1001:\n");
        assert_eq!(machines(&gw.session, &mut no_password).unwrap(), ["ws1"]);
        gw.seed_plain("domains:\n  ONE: {}\n");
        assert_eq!(machines(&gw.session, &mut no_password).unwrap(), ["ws1"]);
        assert_eq!(
            container_of(&gw.session, &mut no_password, "ws1").unwrap(),
            "lanlord-nt4"
        );
    }

    #[test]
    fn a_reset_goes_to_the_container_that_holds_the_account() {
        use crate::testgw::{no_password, Gateway};
        let gw = Gateway::new("exec-reset");
        gw.seed_plain(TWO_DOMAINS);
        gw.seed_accounts("lanlord-nt4", "ws1$:1001:\n");
        gw.seed_accounts("lanlord-nt4-lab", "ws2$:1001:\n");
        crate::nt4::reset_machine(&gw.session, "WS2", &mut no_password).unwrap();
        crate::nt4::reset_machine(&gw.session, "ws1", &mut no_password).unwrap();
        let calls = gw.calls();
        assert!(calls[0].contains("docker exec lanlord-nt4-lab lanlord-user machine ws2 reset"));
        assert!(calls[1].contains("docker exec lanlord-nt4 lanlord-user machine ws1 reset"));
    }

    #[test]
    fn a_container_that_cannot_be_listed_is_skipped_and_the_others_still_work() {
        use crate::testgw::{identity, no_password, Gateway};
        let (gw, id) = (Gateway::new("exec-badlist"), identity("exec-badlist"));
        gw.seed_plain(TWO_DOMAINS);
        gw.seed_accounts("lanlord-nt4-lab", "ws2$:1001:\n");
        gw.fail_list_on("lanlord-nt4 ");
        gw.seed_sealed(&id, "nt4-domain:\n  exec_password: pw\n");
        assert_eq!(machines(&gw.session, &mut no_password).unwrap(), ["ws2"]);
        crate::nt4::reset_machine(&gw.session, "ws2", &mut no_password).unwrap();
        assert!(gw.calls()[0].contains("docker exec lanlord-nt4-lab lanlord-user machine ws2"));
        // every list failing is an error
        gw.fail_list_on("lanlord-nt4");
        assert!(machines(&gw.session, &mut no_password).is_err());
    }

    #[test]
    fn a_machine_in_two_domains_is_refused_by_run_and_reset() {
        use crate::testgw::{identity, no_password, Gateway};
        let (gw, id) = (Gateway::new("exec-dup"), identity("exec-dup"));
        gw.seed_plain(TWO_DOMAINS);
        gw.seed_accounts("lanlord-nt4", "ws1$:1001:\n");
        gw.seed_accounts("lanlord-nt4-lab", "WS1$:1001:\n");
        gw.seed_sealed(&id, "nt4-domain:\n  exec_password: pw\n");
        assert_eq!(machines(&gw.session, &mut no_password).unwrap(), ["ws1"]);
        let run_error = run(
            &gw.session,
            &id,
            &mut no_password,
            ("ws1", "ver", 5, "cli"),
            &mut |_, _| {},
        )
        .unwrap_err()
        .to_string();
        assert!(
            run_error.contains("more than one domain (lanlord-nt4, lanlord-nt4-lab)"),
            "{run_error}"
        );
        let reset_error = crate::nt4::reset_machine(&gw.session, "ws1", &mut no_password)
            .unwrap_err()
            .to_string();
        assert!(
            reset_error.contains("more than one domain"),
            "{reset_error}"
        );
        assert!(gw.calls().is_empty());
    }

    #[test]
    fn only_the_engines_container_names_are_valid() {
        for good in [
            "lanlord-nt4",
            "lanlord-nt4-lab",
            "lanlord-nt4-a-b",
            "lanlord-nt4-7",
        ] {
            assert!(valid_container(good), "{good}");
        }
        for bad in [
            "",
            "lanlord-nt4-",
            "lanlord-nt4--a",
            "lanlord-nt4-a-",
            "lanlord-nt4x",
            "lanlord-nt4-a;b",
            "lanlord-nt4-a b",
            "other",
        ] {
            assert!(!valid_container(bad), "{bad}");
        }
    }

    #[test]
    fn domain_machines_are_listed_without_the_dollar() {
        let listed = "root:0:Administrator\nws1$:1001:\nlanlord-exec:1002:\nnt4srv$:1003:\n$:1:\n";
        assert_eq!(machine_names(listed), ["ws1", "nt4srv"]);
    }

    const TWO_DOMAINS: &str = "nt4-servers:\n  - {name: nt4-domain, role: pdc, domain: ONE, netbios_name: AAA, network: lab}\n  - {name: lab, role: pdc, domain: LAB, netbios_name: BBB, network: lab}\n  - {name: files, role: workgroup, workgroup: WG, netbios_name: CCC, network: lab}\ndomains:\n  ONE: {}\n  LAB: {}\n";

    pub(crate) const PASSWORD: &str = "s3cretpw";
    pub(crate) const LEASE: &str = "1 aa:bb:cc:dd:ee:ff 192.0.2.50 ws1 *";

    /// what a fake gateway answers: its sealed settings (base64, empty for none), its dhcp lease
    /// line (empty for none), and the lines the first and the second `lanlord-exec` call print.
    /// the second is the call with `--address`.
    pub(crate) struct Fake<'a> {
        pub sealed: &'a str,
        pub lease: &'a str,
        pub first: &'a str,
        pub second: &'a str,
    }

    /// a gateway whose docker lists `ws1$`. every lanlord-exec call appends its argv to
    /// `<record>.argv` and saves its stdin in `record`.
    pub(crate) fn gateway(record: &std::path::Path, fake: &Fake) -> Session {
        let script = r#"for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sudo -n true") exit 0 ;;
  *"lanlord-user list") printf 'ws1$:1001:\n' ;;
  "sh -c "*settings.yml*) exit 0 ;;
  "sh -c "*) printf 'present\n%s\n' '@LEASE@' ;;
  *"--address 192.0.2.50"*) printf '%s\n' "$last" >> '@REC@.argv'; cat > '@REC@'; printf '%s\n' '@SECOND@' ;;
  *lanlord-exec*) printf '%s\n' "$last" >> '@REC@.argv'; cat > '@REC@'; printf '%s\n' '@FIRST@' ;;
  *"sh -s") cat >/dev/null; printf '%s' '@SEALED@' ;;
  *) exit 1 ;;
esac"#;
        Session::fake(
            &script
                .replace("@REC@", &record.display().to_string())
                .replace("@LEASE@", fake.lease)
                .replace("@FIRST@", fake.first)
                .replace("@SECOND@", fake.second)
                .replace("@SEALED@", fake.sealed),
        )
    }

    /// the sealed settings of a gateway whose exec account has `password`, as the pull prints them.
    pub(crate) fn sealed(identity: &age::x25519::Identity, password: &str) -> String {
        let plain = format!("nt4-domain:\n  exec_password: {password}\n");
        let cipher = crate::crypto::encrypt(identity, plain.as_bytes()).unwrap();
        base64::engine::general_purpose::STANDARD.encode(cipher)
    }

    fn argv_calls(record: &std::path::Path) -> Vec<String> {
        let file = format!("{}.argv", record.display());
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let _ = std::fs::remove_file(file);
        text.lines().map(String::from).collect()
    }

    fn job() -> Job<'static> {
        ("ws1", "ver", 5, "cli")
    }

    #[test]
    fn a_missing_secret_says_to_run_apply() {
        let record = tempfile::NamedTempFile::new().unwrap();
        let fake = Fake {
            sealed: "",
            lease: "",
            first: "",
            second: "",
        };
        let session = gateway(record.path(), &fake);
        let error = run(
            &session,
            &age::x25519::Identity::generate(),
            &mut no_password,
            job(),
            &mut |_, _| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains(NOT_SET_UP), "{error}");
    }

    #[test]
    fn a_machine_outside_the_domain_is_refused() {
        let record = tempfile::NamedTempFile::new().unwrap();
        let fake = Fake {
            sealed: "",
            lease: "",
            first: "",
            second: "",
        };
        let session = gateway(record.path(), &fake);
        let error = run(
            &session,
            &age::x25519::Identity::generate(),
            &mut no_password,
            ("other", "ver", 5, "cli"),
            &mut |_, _| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains("not a machine"), "{error}");
    }

    #[test]
    fn the_sealed_secret_reaches_the_script_on_stdin_only() {
        let identity = age::x25519::Identity::generate();
        let sealed = sealed(&identity, PASSWORD);
        let first = format!(
            "{}\n{}",
            event(r#"{"stream":"out","data":"aGk="}"#),
            event(r#"{"exit":0}"#)
        );
        let record = tempfile::NamedTempFile::new().unwrap();
        let fake = Fake {
            sealed: &sealed,
            lease: "",
            first: &first,
            second: "",
        };
        let session = gateway(record.path(), &fake);
        let mut seen = Vec::new();
        let outcome = run(
            &session,
            &identity,
            &mut no_password,
            ("WS1", "ver", 9, "cli"),
            &mut |err, bytes| seen.push((err, bytes.to_vec())),
        )
        .unwrap();
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(seen, [(false, b"hi".to_vec())]);
        let calls = argv_calls(record.path());
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert!(calls[0].contains("--machine ws1"), "{calls:?}");
        assert!(calls[0].contains("--timeout 9"), "{calls:?}");
        assert!(!calls[0].contains(PASSWORD), "{calls:?}");
        let stdin = std::fs::read_to_string(record.path()).unwrap();
        assert!(stdin.contains(PASSWORD) && stdin.contains(r#""command":"ver""#));
    }

    fn stream(name: &str, text: &str) -> String {
        let data = base64::engine::general_purpose::STANDARD.encode(text);
        event(&format!(r#"{{"stream":"{name}","data":"{data}"}}"#))
    }

    #[test]
    fn secrets_are_hidden_in_the_text_and_in_the_live_output() {
        let mut collector = Collector::default();
        for line in [
            stream(
                "out",
                "KeyType : (STRING) \"a\"\r\nAnonymousUserPass : (STRING) \"s3",
            ),
            stream("out", "cret\"\r\nwamuserpass : (EXPANDSZ) \"s3cret\""),
            stream("err", "WAMUserPass : (STRING) \"s3cret\"\n"),
            event(r#"{"exit":0}"#),
        ] {
            collector.feed(&line);
        }
        let outcome = collector.finish(0).unwrap();
        assert_eq!(
            outcome.stdout_text(),
            "KeyType : (STRING) \"a\"\r\nAnonymousUserPass : (STRING) (hidden)\r\nwamuserpass : (EXPANDSZ) (hidden)"
        );
        assert_eq!(outcome.stderr_text(), "WAMUserPass : (STRING) (hidden)\n");
    }

    #[test]
    fn a_secret_is_hidden_live_and_flushed_when_the_run_ends_mid_line() {
        let identity = age::x25519::Identity::generate();
        let sealed = sealed(&identity, PASSWORD);
        let first = format!(
            "{}\n{}\n{}\n{}",
            stream(
                "out",
                "A : (STRING) \"x\"\nAnonymousUserPass : (STRING) \"s3"
            ),
            stream("out", "cret\""),
            stream("err", "oops"),
            event(r#"{"exit":0}"#)
        );
        let record = tempfile::NamedTempFile::new().unwrap();
        let fake = Fake {
            sealed: &sealed,
            lease: "",
            first: &first,
            second: "",
        };
        let session = gateway(record.path(), &fake);
        let mut seen = Vec::new();
        let outcome = run(
            &session,
            &identity,
            &mut no_password,
            job(),
            &mut |err, bytes| seen.push((err, bytes.to_vec())),
        )
        .unwrap();
        let shown = |stream: bool| {
            let bytes: Vec<u8> = seen
                .iter()
                .filter(|(err, _)| *err == stream)
                .flat_map(|(_, b)| b.clone())
                .collect();
            String::from_utf8(bytes).unwrap()
        };
        assert_eq!(
            shown(false),
            "A : (STRING) \"x\"\nAnonymousUserPass : (STRING) (hidden)"
        );
        assert_eq!(shown(true), "oops");
        assert!(!outcome.stdout_text().contains("s3cret"));
        assert_eq!(outcome.stderr_text(), "oops");
        argv_calls(record.path());
    }

    #[test]
    fn a_name_that_does_not_answer_is_retried_at_its_lease_address() {
        let identity = age::x25519::Identity::generate();
        let sealed = sealed(&identity, PASSWORD);
        let first = event(r#"{"error":"WS1 does not answer to its name"}"#);
        let second = event(r#"{"exit":7}"#);
        let record = tempfile::NamedTempFile::new().unwrap();
        let fake = Fake {
            sealed: &sealed,
            lease: LEASE,
            first: &first,
            second: &second,
        };
        let session = gateway(record.path(), &fake);
        let outcome = run(&session, &identity, &mut no_password, job(), &mut |_, _| {}).unwrap();
        assert_eq!(outcome.exit_code, Some(7));
        let calls = argv_calls(record.path());
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(!calls[0].contains("--address"), "{calls:?}");
        assert!(
            calls[1].contains("--address 192.0.2.50 --timeout 5"),
            "{calls:?}"
        );
        assert!(calls.iter().all(|c| !c.contains(PASSWORD)), "{calls:?}");
    }

    #[test]
    fn a_name_that_does_not_answer_and_has_no_lease_is_the_error() {
        let identity = age::x25519::Identity::generate();
        let sealed = sealed(&identity, PASSWORD);
        let first = event(r#"{"error":"WS1 does not answer to its name"}"#);
        let record = tempfile::NamedTempFile::new().unwrap();
        let fake = Fake {
            sealed: &sealed,
            lease: "",
            first: &first,
            second: "",
        };
        let session = gateway(record.path(), &fake);
        let error = run(&session, &identity, &mut no_password, job(), &mut |_, _| {}).unwrap_err();
        assert!(error.to_string().contains(NO_ANSWER), "{error}");
        assert_eq!(argv_calls(record.path()).len(), 1);
    }

    #[test]
    fn no_error_message_holds_the_password() {
        let identity = age::x25519::Identity::generate();
        let sealed = sealed(&identity, PASSWORD);
        let no_answer = event(r#"{"error":"WS1 does not answer to its name"}"#);
        let other = event(r#"{"error":"another command is already running on WS1"}"#);
        for (first, lease, sealed) in [
            ("", LEASE, sealed.as_str()),
            ("docker: no such container", "", sealed.as_str()),
            (no_answer.as_str(), "", sealed.as_str()),
            (other.as_str(), LEASE, sealed.as_str()),
            ("", "", ""),
        ] {
            let record = tempfile::NamedTempFile::new().unwrap();
            let fake = Fake {
                sealed,
                lease,
                first,
                second: first,
            };
            let session = gateway(record.path(), &fake);
            let error =
                run(&session, &identity, &mut no_password, job(), &mut |_, _| {}).unwrap_err();
            assert!(
                !format!("{error:#} {error:?}").contains(PASSWORD),
                "{error:?}"
            );
            argv_calls(record.path());
        }
    }

    #[test]
    fn docker_messages_are_quoted_when_the_run_has_no_result() {
        let identity = age::x25519::Identity::generate();
        let sealed = sealed(&identity, PASSWORD);
        let record = tempfile::NamedTempFile::new().unwrap();
        let fake = Fake {
            sealed: &sealed,
            lease: "",
            first: "docker: no such container: lanlord-nt4",
            second: "",
        };
        let session = gateway(record.path(), &fake);
        let error = run(&session, &identity, &mut no_password, job(), &mut |_, _| {}).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("without a result"), "{text}");
        assert!(text.contains("no such container: lanlord-nt4"), "{text}");
        argv_calls(record.path());
    }

    #[test]
    fn an_exit_code_outside_i32_is_clamped() {
        assert_eq!(
            parse_event(&event(r#"{"exit":4294967296}"#)),
            Some(Event::Exit(i32::MAX))
        );
        assert_eq!(
            parse_event(&event(r#"{"exit":-4294967296}"#)),
            Some(Event::Exit(i32::MIN))
        );
        assert_eq!(parse_event(&event(r#"{"exit":-1}"#)), Some(Event::Exit(-1)));
    }

    #[test]
    fn only_the_last_lines_of_noise_are_kept() {
        let mut collector = Collector::default();
        for n in 0..50 {
            collector.feed(&format!("noise {n}"));
        }
        collector.feed(&format!("long {}", "x".repeat(10_000)));
        let text = collector.finish(1).unwrap_err().to_string();
        assert!(text.len() < NOTE_BYTES + 200, "{}", text.len());
        assert!(
            text.contains("long xxx") || text.contains("noise 49"),
            "{text}"
        );
        assert!(!text.contains("noise 0 "), "{text}");
        assert!(!text.contains("noise 10 "), "{text}");
    }

    #[test]
    fn nothing_is_decoded_once_both_streams_are_full() {
        let mut collector = Collector::default();
        let chunk = base64::engine::general_purpose::STANDARD.encode(vec![b'x'; OUTPUT_CAP]);
        for stream in ["out", "err"] {
            collector.feed(&event(&format!(
                r#"{{"stream":"{stream}","data":"{chunk}"}}"#
            )));
        }
        assert!(collector.is_full());
        // not even valid base64 is looked at, and the final event still counts
        assert_eq!(
            collector.feed(&event(r#"{"stream":"out","data":"!!!"}"#)),
            None
        );
        collector.feed(&event(r#"{"exit":2}"#));
        let outcome = collector.finish(0).unwrap();
        assert!(outcome.truncated);
        assert_eq!(outcome.exit_code, Some(2));
    }
}
