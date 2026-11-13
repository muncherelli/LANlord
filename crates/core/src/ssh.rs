//! remote execution through the system OpenSSH client, so ~/.ssh/config,
//! ssh-agent (1Password included) and known_hosts behave as they do for `ssh`.

use std::fmt;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use anyhow::{bail, Result};

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

/// a host reached through the system ssh client.
pub struct Session {
    host: String,
}

impl Session {
    pub fn new(host: &str) -> Result<Self> {
        if host.is_empty() || host.starts_with('-') || host.chars().any(char::is_whitespace) {
            bail!("invalid host: {host:?}");
        }
        Ok(Self {
            host: host.to_string(),
        })
    }

    fn command(&self, remote: &[String]) -> Command {
        let mut cmd = Command::new("ssh");
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
        let mut child = self
            .command(remote)
            .stderr(Stdio::piped())
            .spawn()
            .map_err(spawn_error)?;
        let mut captured = String::new();
        if let Some(stderr) = child.stderr.take() {
            for line in BufReader::new(stderr).split(b'\n') {
                let line = String::from_utf8_lossy(&line?).into_owned();
                eprintln!("{line}");
                captured.push_str(&line);
                captured.push('\n');
            }
        }
        match child.wait()?.code() {
            // 255 is ssh's own failure code
            Some(255) => Err(match classify(&captured) {
                // stderr was already echoed above
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
}
