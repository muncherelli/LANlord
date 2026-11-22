//! runs a command on a domain machine through `lanlord-exec` in the nt4 container: the request
//! (with the exec account's password) goes in on stdin, and the run comes back as event lines.

use anyhow::{anyhow, bail, Result};
use base64::Engine;
use zeroize::Zeroize;

use crate::bootstrap::{root_argv, AskPassword};
use crate::config;
use crate::leases;
use crate::ssh::Session;
use crate::users::cached;

pub use lanlord_model::nt4_exec::*;

/// the account the commands run as; apply creates it as a domain admin.
pub const USER: &str = "lanlord-exec";
/// the sealed secret holding its password: `exec_password` in the `nt4-domain` section.
pub const SECRET: &str = "nt4-domain.exec_password";
/// each stream of a run keeps this many bytes; the rest is dropped and `truncated` says so.
pub const OUTPUT_CAP: usize = 1024 * 1024;
/// the message when the account has no password yet.
pub const NOT_SET_UP: &str = "the exec account is not set up yet; run apply";

const MARKER: &str = "LANLORD_EVENT ";
const CONTAINER: &str = "lanlord-nt4";
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
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Outcome {
    pub fn stdout_text(&self) -> String {
        decode_cp437(&self.stdout)
    }

    pub fn stderr_text(&self) -> String {
        decode_cp437(&self.stderr)
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
        match parse_event(line)? {
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
pub fn argv(machine: &str, address: Option<&str>, timeout_secs: u32) -> Vec<String> {
    let mut argv: Vec<String> = [
        "docker",
        "exec",
        "-i",
        CONTAINER,
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

/// the json request for stdin. it is written straight into one buffer that is big enough for
/// the worst case, so no copy of the password is left behind by a regrowing buffer.
fn request(password: &str, command: &str) -> Vec<u8> {
    #[derive(serde::Serialize)]
    struct Request<'a> {
        user: &'a str,
        password: &'a str,
        command: &'a str,
    }
    // an escaped character takes at most six bytes
    let mut body = Vec::with_capacity(64 + 6 * (password.len() + command.len()));
    // writing to a vec cannot fail
    let _ = serde_json::to_writer(
        &mut body,
        &Request {
            user: USER,
            password,
            command,
        },
    );
    body
}

/// the names of the domain's machine accounts, without the `$`.
pub fn machines(session: &Session, ask: AskPassword) -> Result<Vec<String>> {
    let admin = config::admin(session)?;
    let (remote, first) = root_argv(
        session,
        &admin,
        &["docker", "exec", CONTAINER, "lanlord-user", "list"],
        ask,
    )?;
    let listed = session.capture_with_stdin(&remote, first.as_bytes())?;
    Ok(machine_names(&listed))
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

/// what to run: the machine, the command line and the timeout in seconds.
pub type Job<'a> = (&'a str, &'a str, u32);

/// run a job, handing the output to `on_chunk` (true for stderr) as it comes. the machine must
/// be an account of the domain. a name that does not resolve is tried again at the address of
/// its dhcp lease.
pub fn run(
    session: &Session,
    identity: &age::x25519::Identity,
    ask: AskPassword,
    (machine, command, timeout_secs): Job,
    on_chunk: &mut dyn FnMut(bool, &[u8]),
) -> Result<Outcome> {
    let mut ask = cached(ask);
    let known = machines(session, &mut ask)?;
    let Some(machine) = known.iter().find(|name| name.eq_ignore_ascii_case(machine)) else {
        bail!("{machine} is not a machine of the domain");
    };
    let mut secret = password(session, identity, &mut ask)?;
    let mut body = request(&secret, command);
    secret.zeroize();
    let run = |address: Option<&str>, ask: AskPassword, on_chunk: &mut dyn FnMut(bool, &[u8])| {
        run_request(
            session,
            ask,
            machine,
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
    machine: &str,
    body: &[u8],
    timeout_secs: u32,
    address: Option<&str>,
    on_chunk: &mut dyn FnMut(bool, &[u8]),
) -> Result<Outcome> {
    let admin = config::admin(session)?;
    let command = argv(machine, address, timeout_secs);
    let refs: Vec<&str> = command.iter().map(String::as_str).collect();
    let (remote, first) = root_argv(session, &admin, &refs, ask)?;
    let mut input = first.into_bytes();
    input.extend_from_slice(body);
    let mut collector = Collector::default();
    let code = session.run_with_stdin_filtered(&remote, &input, &mut |line| {
        if let Some((err, bytes)) = collector.feed(line) {
            on_chunk(err, &bytes);
        }
        // output past the cap is dropped, so it is not kept in the debug log either
        !(collector.is_full() && line.starts_with(concat!("LANLORD_EVENT ", r#"{"stream""#)))
    });
    input.zeroize();
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
            let argv = argv("WS1", address, 30);
            assert!(argv.iter().all(|a| !a.contains(secret)), "{argv:?}");
            assert_eq!(
                argv[..6],
                ["docker", "exec", "-i", "lanlord-nt4", "lanlord-exec", "run"]
            );
            assert_eq!(argv[argv.len() - 2..], ["--timeout", "30"]);
        }
        let body = String::from_utf8(request(secret, "ver")).unwrap();
        assert!(body.contains(secret) && body.contains(USER));
    }

    #[test]
    fn domain_machines_are_listed_without_the_dollar() {
        let listed = "root:0:Administrator\nws1$:1001:\nlanlord-exec:1002:\nnt4srv$:1003:\n$:1:\n";
        assert_eq!(machine_names(listed), ["ws1", "nt4srv"]);
    }

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
        ("ws1", "ver", 5)
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
            ("other", "ver", 5),
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
            ("WS1", "ver", 9),
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
