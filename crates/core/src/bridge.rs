//! `lanlord bridge`, both ends. on the gateway [`serve`] reads requests and runs the ops it can in
//! plain rust ([`run_local`]); on the controller [`Session::bridge`] starts it over the ssh session
//! that is already open and [`Bridge::call`] talks to it. frames are in `lanlord_model::bridge`.
//!
//! the bridge is a speedup and a cleaner way to run an op, never a requirement: when the gateway
//! has no bridge, or one that speaks another protocol, [`try_call`] says so and the caller uses
//! the shell scripts it always had.
//!
//! # sudo and stdin
//!
//! the bridge reads its frames from stdin, and `sudo -S` reads the password from stdin too. sudo
//! reads exactly one line (up to the first newline) and leaves the rest of the stream untouched,
//! so the controller sends `password\n` first and the frames after it. checked with sudo 1.9 on
//! debian (a pipe, a login user with a password, frames sent at once and one second late): the
//! program under sudo saw every byte after the first newline. this is the same thing the shell
//! path has always relied on (`password\nscript` into `sudo -S sh -s`).

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout};
use std::sync::atomic::Ordering;

use anyhow::{bail, Context, Result};

use lanlord_model::bridge::{Event, Hello, Request};
use lanlord_model::ops::{Op, Reply};

use crate::bootstrap::{root_argv, AskPassword};
use crate::config;
use crate::log::{emit, Kind};
use crate::ssh::Session;

/// where `lanlord install` puts the gateway's lanlord.
pub const BRIDGE_BIN: &str = "/opt/lanlord/bin/lanlord";

/// why there is no bridge to talk to; callers fall back to the shell scripts.
#[derive(Debug)]
pub enum BridgeError {
    /// the gateway has no `lanlord bridge` (not installed, or too old to know it).
    Missing,
    /// it answers, but in another protocol; the text says what to do.
    Mismatch(String),
    /// it did not start or did not greet (a failed sudo, noise from a login script); this time
    /// only, the next call may work.
    Failed(String),
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => write!(f, "the gateway has no lanlord bridge"),
            Self::Mismatch(why) | Self::Failed(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for BridgeError {}

/// run `op` the way the bridge does, on this machine. `log` gets progress lines.
pub fn run_local(op: &Op, log: &mut dyn FnMut(&str)) -> Result<Reply> {
    match op {
        Op::Subscribe { .. } => bail!("live streams are not available yet"),
        op if op.runs_on_bridge() => run_clock(op, log),
        _ => bail!("the bridge does not run this operation"),
    }
}

/// the clock ops; the bridge only runs on linux gateways, so elsewhere there is nothing to run.
#[cfg(unix)]
fn run_clock(op: &Op, log: &mut dyn FnMut(&str)) -> Result<Reply> {
    use crate::local::clock;
    Ok(match op {
        Op::ClockState => Reply::Clock(clock::read()?),
        Op::ClockZones => Reply::Names(clock::zones()?),
        Op::ClockZoneNow { zone } => Reply::ZoneNow(clock::zone_now(zone)?),
        Op::ClockApply { change } => Reply::Lines(clock::apply(change, log)?),
        Op::ClockUpdateNow => Reply::Lines(clock::update_now()?),
        _ => bail!("the bridge does not run this operation"),
    })
}

#[cfg(not(unix))]
fn run_clock(_: &Op, _: &mut dyn FnMut(&str)) -> Result<Reply> {
    bail!("the bridge runs on a gateway")
}

/// what runs an op for [`serve`]: it gets the op and a sink for progress lines.
pub type RunOp<'a> = dyn FnMut(&Op, &mut dyn FnMut(&str)) -> Result<Reply> + 'a;

/// the gateway side: greet, then answer requests from `input` until it ends. nothing but frames
/// is written to `out`.
pub fn serve(
    input: impl BufRead,
    out: &mut impl Write,
    version: &str,
    run: &mut RunOp,
) -> Result<()> {
    write_frame(out, &Hello::current(version))?;
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request = match serde_json::from_str::<Request>(&line) {
            Ok(request) => request,
            Err(e) => {
                let message = format!("not a request: {e}");
                write_frame(out, &Event::Error { id: 0, message })?;
                continue;
            }
        };
        let id = request.id;
        let mut failed = None;
        let result = run(&request.op, &mut |line: &str| {
            let event = Event::Log {
                id,
                line: line.to_string(),
            };
            if let Err(e) = write_frame(&mut *out, &event) {
                failed.get_or_insert(e);
            }
        });
        if let Some(e) = failed {
            return Err(e);
        }
        write_frame(
            out,
            &match result {
                Ok(reply) => Event::Reply { id, reply },
                Err(e) => Event::Error {
                    id,
                    message: format!("{e:#}"),
                },
            },
        )?;
    }
    Ok(())
}

fn write_frame(out: &mut impl Write, frame: &impl serde::Serialize) -> Result<()> {
    writeln!(out, "{}", serde_json::to_string(frame)?)?;
    out.flush()?;
    Ok(())
}

/// a bridge running on the gateway, reached through one ssh exec.
pub struct Bridge {
    child: Child,
    stdin: Option<ChildStdin>,
    out: BufReader<ChildStdout>,
    next: u64,
}

impl Session {
    /// start `lanlord bridge` on the gateway over the open ssh connection; as root when `root`
    /// (sudo, asking for the password once if it has to). the first frame is checked, so a
    /// bridge that is missing or speaks another protocol is a [`BridgeError`].
    pub fn bridge(&self, root: bool, ask_password: AskPassword) -> Result<Bridge> {
        // LC_ALL=C keeps the messages in English for the not-found check (exit 127 comes first)
        let argv = ["env", "LC_ALL=C", BRIDGE_BIN, "bridge"];
        let (remote, first) = if root {
            let admin = config::admin(self)?;
            root_argv(self, &admin, &argv, ask_password)?
        } else {
            (
                argv.iter().map(ToString::to_string).collect(),
                String::new(),
            )
        };
        let mut child = self.spawn_piped(&remote)?;
        let mut stdin = child.stdin.take();
        let stdout = child.stdout.take().context("no output from ssh")?;
        // what the bridge says on stderr is progress for the log, never protocol
        let stderr = child.stderr.take().map(|stderr| {
            std::thread::spawn(move || {
                let mut said = Vec::new();
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    emit(Kind::Debug, line.as_str());
                    said.push(line);
                }
                said
            })
        });
        if let Some(stdin) = stdin.as_mut() {
            stdin.write_all(first.as_bytes())?;
        }
        let mut bridge = Bridge {
            child,
            stdin,
            out: BufReader::new(stdout),
            next: 0,
        };
        let mut line = String::new();
        let hello = match bridge.out.read_line(&mut line) {
            Ok(n) if n > 0 => serde_json::from_str::<Hello>(&line).ok(),
            _ => None,
        };
        let Some(hello) = hello else {
            return Err(bridge.not_started(stderr).into());
        };
        if let Some(why) = hello.mismatch() {
            return Err(BridgeError::Mismatch(why).into());
        }
        Ok(bridge)
    }
}

impl Bridge {
    /// why nothing greeted: the program is not there (exit 127), or it failed some other way.
    fn not_started(&mut self, stderr: Option<std::thread::JoinHandle<Vec<String>>>) -> BridgeError {
        self.stdin.take();
        let code = self.child.wait().ok().and_then(|s| s.code());
        let said = stderr.and_then(|h| h.join().ok()).unwrap_or_default();
        let said = said.join(" ");
        if code == Some(127) || said.contains("not found") || said.contains("No such file") {
            return BridgeError::Missing;
        }
        BridgeError::Failed(format!(
            "the bridge did not start (status {}): {}",
            code.map_or_else(|| "none".to_string(), |c| c.to_string()),
            said.trim()
        ))
    }

    /// run `op` and wait for its reply; `on_log` sees the progress lines.
    pub fn call(&mut self, op: &Op, on_log: &mut dyn FnMut(&str)) -> Result<Reply> {
        self.next += 1;
        let id = self.next;
        let stdin = self.stdin.as_mut().context("the bridge is closed")?;
        write_frame(stdin, &Request { id, op: op.clone() })?;
        loop {
            let mut line = String::new();
            if self.out.read_line(&mut line)? == 0 {
                bail!("the bridge ended without an answer");
            }
            match serde_json::from_str::<Event>(&line).context("the bridge sent a broken frame")? {
                Event::Log { line, .. } => on_log(&line),
                Event::Reply { reply, .. } => return Ok(reply),
                Event::Error { message, .. } => bail!(message),
                // reserved for subscriptions
                Event::Sample { .. } => {}
            }
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        // closing stdin ends the bridge
        self.stdin.take();
        let _ = self.child.wait();
    }
}

/// run `op` through the bridge. `Ok(None)` when the gateway has none (or a mismatched one): the
/// reason is logged, later ops skip the attempt, and the caller runs its shell script instead.
pub fn try_call(session: &Session, op: &Op, ask_password: AskPassword) -> Result<Option<Reply>> {
    if session.no_bridge.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let mut bridge = match session.bridge(op.needs_root(), ask_password) {
        Ok(bridge) => bridge,
        Err(err) => match err.downcast_ref::<BridgeError>() {
            Some(BridgeError::Failed(why)) => {
                // one-off: the next call may work, so the session keeps trying the bridge
                emit(
                    Kind::Info,
                    format!("warning: the bridge failed, using the scripts: {why}"),
                );
                return Ok(None);
            }
            Some(reason) => {
                session.no_bridge.store(true, Ordering::Relaxed);
                emit(Kind::Debug, format!("not using the bridge: {reason}"));
                if let BridgeError::Mismatch(why) = reason {
                    emit(Kind::Info, why.clone());
                }
                return Ok(None);
            }
            None => return Err(err),
        },
    };
    bridge
        .call(op, &mut |line| emit(Kind::Debug, line))
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(input: &str, run: &mut RunOp) -> Vec<String> {
        let mut out = Vec::new();
        serve(input.as_bytes(), &mut out, "9.9.9", run).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn the_bridge_greets_then_answers_many_requests_on_stdout_only() {
        let input = concat!(
            r#"{"id":1,"op":{"op":"clock_zones"}}"#,
            "\n",
            "\n",
            "nonsense\n",
            r#"{"id":2,"op":{"op":"clock_update_now"}}"#,
            "\n",
            r#"{"id":3,"op":{"op":"subscribe","topic":"clock"}}"#,
            "\n",
        );
        let lines = frames(input, &mut |op, log| match op {
            Op::ClockZones => Ok(Reply::Names(vec!["UTC".into()])),
            Op::ClockUpdateNow => {
                log("restarting");
                bail!("there is no time service to restart")
            }
            _ => run_local(op, log),
        });
        assert_eq!(lines[0], r#"{"proto":1,"version":"9.9.9"}"#);
        assert_eq!(
            lines[1],
            r#"{"kind":"reply","id":1,"reply":{"reply":"names","value":["UTC"]}}"#
        );
        assert!(
            lines[2].contains(r#""kind":"error","id":0"#),
            "{}",
            lines[2]
        );
        assert_eq!(lines[3], r#"{"kind":"log","id":2,"line":"restarting"}"#);
        assert!(
            lines[4].contains("there is no time service"),
            "{}",
            lines[4]
        );
        assert!(lines[5].contains("not available yet"), "{}", lines[5]);
        assert_eq!(lines.len(), 6);
    }

    /// a session whose "gateway" runs `body` after answering `id -un` as root.
    #[cfg(unix)]
    fn gateway(body: &str) -> Session {
        let session = Session::fake(&format!(
            "for a; do last=$a; done\ncase \"$last\" in\n  \"id -un\") echo root ;;\n  *) {body} ;;\nesac"
        ));
        session.no_bridge.store(false, Ordering::Relaxed);
        session
    }

    #[cfg(unix)]
    fn no_password(_: &str) -> Result<String> {
        bail!("no password expected")
    }

    #[cfg(unix)]
    const HELLO: &str = r#"echo '{"proto":1,"version":"t"}'"#;
    #[cfg(unix)]
    const REPLY: &str =
        r#"echo '{"kind":"reply","id":1,"reply":{"reply":"names","value":["UTC"]}}'"#;

    #[cfg(unix)]
    #[test]
    fn a_reply_comes_back_with_its_log_lines() {
        let log = r#"echo '{"kind":"log","id":1,"line":"working"}'"#;
        let gw = gateway(&format!("{HELLO}; read req; {log}; echo '{{\"kind\":\"sample\",\"id\":1,\"topic\":\"t\",\"value\":\"1\"}}'; {REPLY}"));
        let mut bridge = gw.bridge(false, &mut no_password).unwrap();
        let mut lines = Vec::new();
        let reply = bridge.call(&Op::ClockZones, &mut |l| lines.push(l.to_string()));
        assert_eq!(reply.unwrap(), Reply::Names(vec!["UTC".into()]));
        assert_eq!(lines, ["working"]);
    }

    #[cfg(unix)]
    #[test]
    fn the_root_bridge_runs_directly_for_root() {
        let gw = gateway(&format!("{HELLO}; read req; {REPLY}"));
        let got = try_call(&gw, &Op::ClockUpdateNow, &mut no_password).unwrap();
        assert_eq!(got, Some(Reply::Names(vec!["UTC".into()])));
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_request_is_an_error_with_the_bridges_words() {
        let err = r#"echo '{"kind":"error","id":1,"message":"unknown time zone: X"}'"#;
        let gw = gateway(&format!("{HELLO}; read req; {err}"));
        let got = try_call(&gw, &Op::ClockZones, &mut no_password).unwrap_err();
        assert!(got.to_string().contains("unknown time zone: X"), "{got}");
    }

    #[cfg(unix)]
    #[test]
    fn a_broken_or_missing_answer_is_an_error() {
        let gw = gateway(&format!("{HELLO}; read req; echo nonsense"));
        let err = try_call(&gw, &Op::ClockZones, &mut no_password).unwrap_err();
        assert!(err.to_string().contains("broken frame"), "{err}");
        let gw = gateway(&format!("{HELLO}; read req"));
        let err = try_call(&gw, &Op::ClockZones, &mut no_password).unwrap_err();
        assert!(err.to_string().contains("without an answer"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_missing_bridge_falls_back_once_and_is_not_tried_again() {
        let gw = gateway("echo 'sh: lanlord: not found' >&2; exit 127");
        assert_eq!(
            try_call(&gw, &Op::ClockZones, &mut no_password).unwrap(),
            None
        );
        assert!(gw.no_bridge.load(Ordering::Relaxed));
        assert_eq!(
            try_call(&gw, &Op::ClockZones, &mut no_password).unwrap(),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_start_falls_back_once_without_giving_up_on_the_bridge() {
        let gw = gateway("echo 'sudo: a password is required' >&2; exit 1");
        let err = gw.bridge(false, &mut no_password).err().unwrap();
        assert!(matches!(
            err.downcast_ref::<BridgeError>(),
            Some(BridgeError::Failed(why)) if why.contains("password is required")
        ));
        assert_eq!(
            try_call(&gw, &Op::ClockZones, &mut no_password).unwrap(),
            None
        );
        assert!(!gw.no_bridge.load(Ordering::Relaxed));
        let gw = gateway("echo 'welcome back'");
        let err = gw.bridge(false, &mut no_password).err().unwrap();
        assert!(matches!(
            err.downcast_ref::<BridgeError>(),
            Some(BridgeError::Failed(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn another_protocol_falls_back_with_the_update_advice() {
        let gw = gateway(r#"echo '{"proto":99,"version":"9.0.0"}'"#);
        let err = gw.bridge(false, &mut no_password).err().unwrap();
        let text = err.to_string();
        assert!(
            text.contains("lanlord update") && text.contains("9.0.0"),
            "{text}"
        );
        assert!(matches!(
            err.downcast_ref::<BridgeError>(),
            Some(BridgeError::Mismatch(_))
        ));
        assert_eq!(
            try_call(&gw, &Op::ClockZones, &mut no_password).unwrap(),
            None
        );
        assert!(gw.no_bridge.load(Ordering::Relaxed));
    }

    #[test]
    fn the_errors_say_what_is_wrong() {
        assert!(BridgeError::Missing
            .to_string()
            .contains("no lanlord bridge"));
        assert_eq!(BridgeError::Mismatch("why".into()).to_string(), "why");
        assert_eq!(BridgeError::Failed("how".into()).to_string(), "how");
    }

    #[test]
    fn a_bad_frame_does_not_stop_the_bridge() {
        let lines = frames("{}\n", &mut |_, _| Ok(Reply::Done));
        assert_eq!(lines.len(), 2);
        assert!(lines[1].contains("not a request"), "{}", lines[1]);
    }

    #[test]
    fn an_op_the_bridge_does_not_run_is_an_error() {
        let err = run_local(&Op::Doctor, &mut |_| {}).unwrap_err();
        assert!(err.to_string().contains("does not run"), "{err}");
    }
}
