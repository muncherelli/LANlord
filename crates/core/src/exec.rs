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

/// the status for `process::exit` of a run on a machine. a process keeps only the low 8 bits, so
/// an nt code such as 256 would read as success: a code that is not zero never becomes zero.
pub fn exit_status(exit_code: Option<i32>) -> i32 {
    match exit_code {
        None => TIMED_OUT,
        Some(0) => 0,
        Some(code) if code & 0xff == 0 => 1,
        Some(code) => code & 0xff,
    }
}

/// show what a machine run did (output, then notes on stderr); returns the status to exit with.
pub fn show_run(reply: &Reply) -> Result<i32> {
    let Reply::MachineRun {
        exit_code,
        stdout,
        stderr,
        timed_out,
        truncated,
    } = reply
    else {
        bail!("the server answered something other than a run");
    };
    print!("{stdout}");
    eprint!("{stderr}");
    if *truncated {
        eprintln!("(output cut at 1 MiB)");
    }
    if *timed_out {
        eprintln!("timed out; the command may still be running");
    }
    Ok(exit_status(*exit_code))
}

/// the operation that runs `command` on `machine` for at most `timeout_secs`.
pub fn machine_run(machine: &str, command: &[String], timeout_secs: u32) -> Result<Operation> {
    if command.is_empty() {
        bail!("no command to run on {machine}");
    }
    Ok(Operation::MachineRun {
        machine: machine.to_string(),
        command: command.join(" "),
        timeout_secs,
    })
}

fn on_machine(gateway: &str, name: &str, command: &[String], ask: AskPassword) -> Result<i32> {
    let operation = machine_run(name, command, DEFAULT_TIMEOUT_SECS)?;
    show_run(&execute(gateway, &operation, ask)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exit_code_never_wraps_to_success() {
        assert_eq!(exit_status(Some(0)), 0);
        assert_eq!(exit_status(Some(3)), 3);
        assert_eq!(exit_status(Some(255)), 255);
        assert_eq!(exit_status(Some(256)), 1);
        assert_eq!(exit_status(Some(512)), 1);
        assert_eq!(exit_status(Some(257)), 1);
        assert_eq!(exit_status(Some(-1)), 255);
        assert_eq!(exit_status(Some(i32::MIN)), 1);
        assert_eq!(exit_status(None), 124);
    }

    fn run_reply(exit_code: Option<i32>, timed_out: bool, truncated: bool) -> Reply {
        Reply::MachineRun {
            exit_code,
            stdout: "out\n".into(),
            stderr: "err\n".into(),
            timed_out,
            truncated,
        }
    }

    #[test]
    fn a_shown_run_gives_the_status_to_exit_with() {
        assert_eq!(show_run(&run_reply(Some(3), false, false)).unwrap(), 3);
        assert_eq!(show_run(&run_reply(Some(256), false, true)).unwrap(), 1);
        assert_eq!(show_run(&run_reply(None, true, false)).unwrap(), 124);
        assert!(show_run(&Reply::Done).is_err());
    }

    #[test]
    fn a_machine_command_is_joined_and_must_not_be_empty() {
        let words = |list: &[&str]| list.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert_eq!(
            machine_run("ws1", &words(&["dir", "C:\\"]), 30).unwrap(),
            Operation::MachineRun {
                machine: "ws1".into(),
                command: "dir C:\\".into(),
                timeout_secs: 30
            }
        );
        assert!(machine_run("ws1", &[], 30).is_err());
    }

    #[test]
    fn a_command_on_a_machine_needs_a_command_before_it_reaches_the_gateway() {
        let target = Target::Machine {
            gateway: "gw".into(),
            name: "ws1".into(),
        };
        let mut ask = |_: &str| -> Result<String> { bail!("no password") };
        assert!(exec(&target, &[], &mut ask).is_err());
        assert!(exec_detached(&target, &[], &mut ask).is_err());
    }
}
