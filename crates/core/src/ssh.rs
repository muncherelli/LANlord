//! remote execution through the system OpenSSH client, so ~/.ssh/config,
//! ssh-agent (1Password included) and known_hosts behave as they do for `ssh`.

use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use anyhow::{bail, Result};

use crate::log::{emit, Kind};

/// why an ssh connection failed, as far as we can tell from its stderr.
#[derive(Debug)]
pub enum SshError {
    NotInstalled,
    AgentLocked,
    HostKey,
    AuthFailed,
    Failed(String),
}

impl fmt::Display for SshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled => write!(f, "the ssh client was not found; install openssh"),
            Self::AgentLocked => {
                write!(f, "ssh-agent is not responding; unlock 1password and retry")
            }
            Self::HostKey => write!(
                f,
                "host key verification failed; connect once with ssh to trust the host"
            ),
            Self::AuthFailed => {
                write!(f, "ssh authentication failed; check your key and the agent")
            }
            Self::Failed(detail) => write!(f, "ssh failed: {detail}"),
        }
    }
}

impl std::error::Error for SshError {}

/// map ssh's stderr to a specific failure.
fn classify(stderr: &str) -> SshError {
    if stderr.contains("communication with agent failed") {
        SshError::AgentLocked
    } else if stderr.contains("Host key verification failed") {
        SshError::HostKey
    } else if stderr.contains("Permission denied") {
        SshError::AuthFailed
    } else {
        SshError::Failed(stderr.trim().to_string())
    }
}

/// quote `arg` so the remote shell sees it as one literal word.
fn shell_quote(arg: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c);
    if !arg.is_empty() && arg.chars().all(plain) {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

fn spawn_error(err: std::io::Error) -> SshError {
    if err.kind() == std::io::ErrorKind::NotFound {
        SshError::NotInstalled
    } else {
        SshError::Failed(err.to_string())
    }
}

/// what the remote command reads from.
enum Input<'a> {
    /// our own stdin, for the cli.
    Inherit,
    /// nothing, for front ends that own the terminal.
    Null,
    Bytes(&'a [u8]),
}

/// a host reached through the system ssh client.
pub struct Session {
    host: String,
    /// the ssh program and any leading arguments.
    program: Vec<String>,
}

impl Session {
    pub fn new(host: &str) -> Result<Self> {
        if host.is_empty() || host.starts_with('-') || host.chars().any(char::is_whitespace) {
            bail!("invalid host: {host:?}");
        }
        Ok(Self {
            host: host.to_string(),
            program: vec!["ssh".to_string()],
        })
    }

    fn command(&self, remote: &[String]) -> Command {
        let mut cmd = Command::new(&self.program[0]);
        cmd.args(&self.program[1..]);
        // never prompt on the terminal; the gui and tui have no tty to answer
        cmd.args(["-o", "BatchMode=yes"]);
        // share one connection so the agent prompts once per session
        // (windows openssh has no control sockets)
        #[cfg(unix)]
        cmd.args([
            "-o",
            "ControlMaster=auto",
            "-o",
            "ControlPersist=10m",
            "-o",
            "ControlPath=~/.ssh/lanlord-%C",
        ]);
        cmd.arg("--").arg(&self.host);
        if !remote.is_empty() {
            let line: Vec<String> = remote.iter().map(|a| shell_quote(a)).collect();
            cmd.arg(line.join(" "));
        }
        cmd
    }

    /// run `remote` with stdout and stdin attached to ours; returns its exit code.
    pub fn run(&self, remote: &[String]) -> Result<i32> {
        self.spawn(remote, Input::Inherit)
    }

    /// like `run`, but the remote reads nothing from our stdin, so a tui
    /// that owns the terminal keeps its keystrokes.
    pub fn stream(&self, remote: &[String]) -> Result<i32> {
        self.spawn(remote, Input::Null)
    }

    /// run `remote` with `input` as its stdin; returns its exit code.
    pub fn run_with_stdin(&self, remote: &[String], input: &[u8]) -> Result<i32> {
        self.spawn(remote, Input::Bytes(input))
    }

    fn spawn(&self, remote: &[String], input: Input) -> Result<i32> {
        emit(
            Kind::Debug,
            format!("ssh {} {}", self.host, remote.join(" ")),
        );
        let mut cmd = self.command(remote);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        match input {
            Input::Inherit => {}
            Input::Null => {
                cmd.stdin(Stdio::null());
            }
            Input::Bytes(_) => {
                cmd.stdin(Stdio::piped());
            }
        }
        let mut child = cmd.spawn().map_err(spawn_error)?;
        if let (Input::Bytes(input), Some(mut stdin)) = (input, child.stdin.take()) {
            // dropping stdin closes it so the remote shell sees end of input
            stdin.write_all(input)?;
        }
        let stdout = child.stdout.take().map(|out| {
            std::thread::spawn(move || {
                for line in BufReader::new(out).split(b'\n').map_while(|l| l.ok()) {
                    emit(Kind::Out, String::from_utf8_lossy(&line));
                }
            })
        });
        let mut captured = String::new();
        if let Some(stderr) = child.stderr.take() {
            for line in BufReader::new(stderr).split(b'\n') {
                let line = String::from_utf8_lossy(&line?).into_owned();
                emit(Kind::Err, line.as_str());
                captured.push_str(&line);
                captured.push('\n');
            }
        }
        if let Some(thread) = stdout {
            let _ = thread.join();
        }
        match child.wait()?.code() {
            // 255 is ssh's own failure code
            Some(255) => Err(match classify(&captured) {
                // stderr was already shown above
                SshError::Failed(_) => SshError::Failed("could not connect".into()),
                known => known,
            }
            .into()),
            Some(code) => Ok(code),
            None => bail!("ssh was terminated by a signal"),
        }
    }

    /// run `remote` and return its stdout, for front ends with no terminal.
    pub fn capture(&self, remote: &[String]) -> Result<String> {
        let out = self
            .command(remote)
            .stdin(Stdio::null())
            .output()
            .map_err(spawn_error)?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        match out.status.code() {
            Some(0) => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
            Some(255) => Err(classify(&stderr).into()),
            Some(code) => bail!("command exited with status {code}: {}", stderr.trim()),
            None => bail!("ssh was terminated by a signal"),
        }
    }
}

#[cfg(all(test, unix))]
impl Session {
    /// a session that runs `script` instead of ssh, to test without a server.
    pub(crate) fn fake(script: &str) -> Session {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "lanlord-fake-ssh-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, format!("{script}\n")).unwrap();
        let mut session = Session::new("gw").unwrap();
        // run it through sh so we never exec a file we just wrote (ETXTBSY)
        session.program = vec!["sh".into(), path.to_string_lossy().into_owned()];
        session
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_only_when_needed() {
        assert_eq!(shell_quote("uptime"), "uptime");
        assert_eq!(shell_quote("/opt/lanlord/bin"), "/opt/lanlord/bin");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote("$(rm -rf /)"), "'$(rm -rf /)'");
    }

    #[test]
    fn rejects_hosts_that_look_like_options() {
        assert!(Session::new("-oProxyCommand=x").is_err());
        assert!(Session::new("").is_err());
        assert!(Session::new("a b").is_err());
        assert!(Session::new("user@gw").is_ok());
    }

    #[test]
    fn classifies_agent_and_auth_failures() {
        let agent = "sign_and_send_pubkey: signing failed: communication with agent failed";
        assert!(matches!(classify(agent), SshError::AgentLocked));
        assert!(matches!(
            classify("user@gw: Permission denied (publickey)."),
            SshError::AuthFailed
        ));
        assert!(matches!(
            classify("Host key verification failed."),
            SshError::HostKey
        ));
        assert!(matches!(classify("boom"), SshError::Failed(_)));
    }

    #[cfg(unix)]
    #[test]
    fn capture_returns_stdout() {
        let session = Session::fake("echo hello");
        assert_eq!(session.capture(&["x".into()]).unwrap(), "hello\n");
    }

    #[cfg(unix)]
    #[test]
    fn capture_reports_a_failing_command() {
        let session = Session::fake("echo nope >&2; exit 3");
        let err = session.capture(&[]).unwrap_err().to_string();
        assert!(err.contains("status 3") && err.contains("nope"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn capture_classifies_ssh_failures() {
        let session = Session::fake("echo 'communication with agent failed' >&2; exit 255");
        let err = session.capture(&[]).unwrap_err();
        assert!(matches!(
            err.downcast_ref::<SshError>(),
            Some(SshError::AgentLocked)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn run_sends_output_to_the_log() {
        use crate::log::subscribe;
        let rx = subscribe();
        Session::fake("echo log-ssh-out; echo log-ssh-err >&2")
            .run(&[])
            .unwrap();
        let lines: Vec<_> = rx.try_iter().collect();
        assert!(lines
            .iter()
            .any(|l| l.kind == Kind::Out && l.text == "log-ssh-out"));
        assert!(lines
            .iter()
            .any(|l| l.kind == Kind::Err && l.text == "log-ssh-err"));
        assert!(lines.iter().any(|l| l.kind == Kind::Debug));
    }

    #[cfg(unix)]
    #[test]
    fn stream_gives_the_remote_no_stdin() {
        // wc reads nothing, so the fake exits 0 instead of waiting on us
        assert_eq!(
            Session::fake("exit $(wc -c | tr -d ' ')")
                .stream(&[])
                .unwrap(),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_returns_the_exit_code() {
        assert_eq!(Session::fake("exit 0").run(&[]).unwrap(), 0);
        assert_eq!(Session::fake("exit 7").run(&[]).unwrap(), 7);
    }

    #[cfg(unix)]
    #[test]
    fn run_maps_connection_failures() {
        let err = Session::fake("echo 'Host key verification failed.' >&2; exit 255")
            .run(&[])
            .unwrap_err();
        assert!(matches!(
            err.downcast_ref::<SshError>(),
            Some(SshError::HostKey)
        ));
        let err = Session::fake("echo weird >&2; exit 255")
            .run(&[])
            .unwrap_err();
        assert!(err.to_string().contains("could not connect"));
    }

    #[cfg(unix)]
    #[test]
    fn run_with_stdin_feeds_the_remote() {
        // the fake ssh exits with the number of bytes it read
        let session = Session::fake("exit $(wc -c | tr -d ' ')");
        assert_eq!(session.run_with_stdin(&[], b"abcde").unwrap(), 5);
    }

    #[test]
    fn missing_ssh_is_reported() {
        let mut session = Session::new("gw").unwrap();
        session.program = vec!["/nonexistent/ssh".into()];
        let err = session.capture(&[]).unwrap_err();
        assert!(matches!(
            err.downcast_ref::<SshError>(),
            Some(SshError::NotInstalled)
        ));
    }
}
