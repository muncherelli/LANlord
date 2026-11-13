use anyhow::{bail, Result};

/// where a command runs.
pub enum Target {
    /// the lanlord gateway itself, reached directly over ssh.
    Gateway { host: String },
    /// a machine on the retronet lan, reached through the gateway.
    Machine { gateway: String, name: String },
}

/// run `command` on `target` and stream its output back.
pub fn exec(_target: &Target, _command: &[String]) -> Result<()> {
    bail!("exec is not implemented yet")
}
