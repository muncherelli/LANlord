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
/// the sealed secret holding its password.
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
        return i32::try_from(code).ok().map(Event::Exit);
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
}

impl Collector {
    /// take one output line; returns the stream and the bytes that were kept, for live display.
    pub fn feed(&mut self, line: &str) -> Option<(bool, Vec<u8>)> {
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

    /// the outcome; an error event, or a stream that ended with no final event, is an error.
    pub fn finish(self, docker_exit: i32) -> Result<Outcome> {
        if let Some(message) = self.error {
            bail!("{message}");
        }
        if !self.finished {
            bail!("the run ended without a result (docker exited with {docker_exit})");
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

/// the json request for stdin.
fn request(password: &str, command: &str) -> Vec<u8> {
    serde_json::json!({"user": USER, "password": password, "command": command})
        .to_string()
        .into_bytes()
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
    let sealed = config::pull_with(session, identity, ask)?.ok_or_else(|| anyhow!(NOT_SET_UP))?;
    let value = lanlord_model::settings::parse_value(&sealed)?;
    lanlord_model::settings::secret_values(&value)
        .into_iter()
        .find(|(path, text)| path == SECRET && !text.is_empty())
        .map(|(_, text)| text)
        .ok_or_else(|| anyhow!(NOT_SET_UP))
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
    let code = session.run_with_stdin_lines(&remote, &input, &mut |line| {
        if let Some((err, bytes)) = collector.feed(line) {
            on_chunk(err, &bytes);
        }
    });
    input.zeroize();
    collector.finish(code?)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::testgw::no_password;

    fn event(json: &str) -> String {
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

    /// a gateway whose docker lists `ws1$`, which has no sealed settings, and whose lanlord-exec
    /// saves its argv and stdin next to `record` and answers `events`.
    fn gateway(record: &std::path::Path, events: &str) -> Session {
        Session::fake(&format!(
            r#"for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sudo -n true") exit 0 ;;
  *"lanlord-user list") printf 'ws1$:1001:\n' ;;
  *lanlord-exec*) printf '%s\n' "$last" > '{rec}.argv'; cat > '{rec}'; printf '%s\n' '{events}' ;;
  *"sh -s") cat >/dev/null ;;
  *) exit 1 ;;
esac"#,
            rec = record.display(),
            events = events,
        ))
    }

    #[test]
    fn a_missing_secret_says_to_run_apply() {
        let record = tempfile::NamedTempFile::new().unwrap();
        let session = gateway(record.path(), "");
        let error = run(
            &session,
            &age::x25519::Identity::generate(),
            &mut no_password,
            ("ws1", "ver", 5),
            &mut |_, _| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains(NOT_SET_UP), "{error}");
    }

    #[test]
    fn a_machine_outside_the_domain_is_refused() {
        let record = tempfile::NamedTempFile::new().unwrap();
        let session = gateway(record.path(), "");
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
    fn a_request_reaches_the_script_on_stdin_only() {
        let record = tempfile::NamedTempFile::new().unwrap();
        let events = format!(
            "{}\n{}",
            event(r#"{"stream":"out","data":"aGk="}"#),
            event(r#"{"exit":0}"#)
        );
        let session = gateway(record.path(), &events);
        let mut seen = Vec::new();
        let outcome = run_request(
            &session,
            &mut no_password,
            "WS1",
            &request("s3cretpw", "ver"),
            9,
            None,
            &mut |err, bytes| seen.push((err, bytes.to_vec())),
        )
        .unwrap();
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(seen, [(false, b"hi".to_vec())]);
        let argv_file = format!("{}.argv", record.path().display());
        let argv = std::fs::read_to_string(&argv_file).unwrap();
        assert!(
            argv.contains("--machine WS1") && !argv.contains("s3cretpw"),
            "{argv}"
        );
        let stdin = std::fs::read_to_string(record.path()).unwrap();
        assert!(stdin.contains("s3cretpw"), "{stdin}");
        let _ = std::fs::remove_file(argv_file);
    }
}
