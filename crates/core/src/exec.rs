use anyhow::{bail, Result};

use crate::bootstrap::AskPassword;
use crate::operations::{execute, Operation, Reply, DEFAULT_TIMEOUT_SECS};
use crate::ssh::Session;

/// where a command runs.
pub enum Target {
    /// the lanlord gateway itself, reached directly over ssh.
    Gateway { host: String },
    /// a machine on the retronet lan, reached through the gateway.
    Machine { gateway: String, name: String },
}

/// run `command` on `target`, streaming its output; returns the exit code. a command on a machine
/// is run by the gateway, so its output is shown when it ends.
pub fn exec(target: &Target, command: &[String], ask: AskPassword) -> Result<i32> {
    match target {
        Target::Gateway { host } => Session::new(host)?.run(command),
        Target::Machine { gateway, name } => on_machine(gateway, name, command, ask),
    }
}

/// like `exec`, but the command reads nothing from our stdin; for the tui.
pub fn exec_detached(target: &Target, command: &[String], ask: AskPassword) -> Result<i32> {
    match target {
        Target::Gateway { host } => Session::new(host)?.stream(command),
        Target::Machine { gateway, name } => on_machine(gateway, name, command, ask),
    }
}

/// the exit code a timed out run reports, as `timeout(1)` does.
const TIMED_OUT: i32 = 124;

fn on_machine(gateway: &str, name: &str, command: &[String], ask: AskPassword) -> Result<i32> {
    if command.is_empty() {
        bail!("no command to run on {name}");
    }
    let operation = Operation::MachineRun {
        machine: name.to_string(),
        command: command.join(" "),
        timeout_secs: DEFAULT_TIMEOUT_SECS,
    };
    let Reply::MachineRun {
        exit_code,
        stdout,
        stderr,
        timed_out,
        truncated,
    } = execute(gateway, &operation, ask)?
    else {
        bail!("the gateway answered something other than a run");
    };
    print!("{stdout}");
    eprint!("{stderr}");
    if truncated {
        eprintln!("(output cut at 1 MiB)");
    }
    if timed_out {
        eprintln!("timed out; the command may still be running on {name}");
    }
    Ok(exit_code.unwrap_or(TIMED_OUT))
}
