use anyhow::{bail, Result};

use crate::ssh::Session;

/// where a command runs.
pub enum Target {
    /// the lanlord gateway itself, reached directly over ssh.
    Gateway { host: String },
    /// a machine on the retronet lan, reached through the gateway.
    Machine { gateway: String, name: String },
}

/// run `command` on `target`, streaming its output; returns the exit code.
pub fn exec(target: &Target, command: &[String]) -> Result<i32> {
    match target {
        Target::Gateway { host } => Session::new(host)?.run(command),
        Target::Machine { .. } => bail!("running commands on lan machines is not implemented yet"),
    }
}
