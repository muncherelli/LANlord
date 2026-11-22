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

/// the operation that runs the preset `id` on `machine`; the arguments are checked here, so an
/// unknown preset or an argument that is not plain text never leaves the caller.
pub fn machine_preset(
    machine: &str,
    id: &str,
    args: &[String],
    timeout_secs: u32,
) -> Result<Operation> {
    if !(1..=900).contains(&timeout_secs) {
        bail!("the timeout must be 1 to 900 seconds");
    }
    lanlord_model::nt4_exec::render(id, args)
        .map_err(|reason| anyhow::anyhow!("preset {id} {reason}"))?;
    Ok(Operation::MachinePreset {
        machine: machine.to_string(),
        preset: id.to_string(),
        args: args.to_vec(),
        timeout_secs,
    })
}

/// ask before a preset that changes the machine runs: `yes` skips the question, a read-only
/// preset never asks. without `yes` the question needs a terminal, so a script cannot answer it.
/// `confirm` gets the question and answers whether to go on.
pub fn confirm_preset(
    id: &str,
    machine: &str,
    (yes, interactive): (bool, bool),
    confirm: &mut dyn FnMut(&str) -> Result<bool>,
) -> Result<()> {
    if yes || !lanlord_model::nt4_exec::needs_confirmation(Some(id)) {
        return Ok(());
    }
    if !interactive {
        bail!("needs --yes when not run at a terminal");
    }
    if confirm(&format!(
        "preset {id} changes {machine}; run it? type yes: "
    ))? {
        Ok(())
    } else {
        bail!("not confirmed; nothing was run")
    }
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

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn a_preset_operation_is_built_from_a_known_preset_and_plain_arguments() {
        assert_eq!(
            machine_preset("ws1", "startweb", &words(&["1"]), 30).unwrap(),
            Operation::MachinePreset {
                machine: "ws1".into(),
                preset: "startweb".into(),
                args: words(&["1"]),
                timeout_secs: 30
            }
        );
        let unknown = machine_preset("ws1", "nope", &[], 30).unwrap_err();
        assert!(unknown.to_string().contains("not a known preset"));
        assert!(machine_preset("ws1", "startweb", &[], 30).is_err());
        assert!(machine_preset("ws1", "ver", &[], 0).is_err());
        assert!(machine_preset("ws1", "ver", &[], 901).is_err());
        for bad in [
            "1 & del *",
            "1|x",
            "%PATH%",
            "a\"b",
            "'a'",
            "-c",
            "//S",
            "/x",
            "a,b",
            "",
        ] {
            assert!(
                machine_preset("ws1", "startweb", &words(&[bad]), 30).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn only_a_mutating_preset_asks_and_yes_skips_the_question() {
        let asked = std::cell::RefCell::new(Vec::new());
        let mut agree = |q: &str| -> Result<bool> {
            asked.borrow_mut().push(q.to_string());
            Ok(true)
        };
        confirm_preset("ver", "ws1", (false, true), &mut agree).unwrap();
        assert!(asked.borrow().is_empty());
        confirm_preset("startweb", "ws1", (true, true), &mut agree).unwrap();
        assert!(asked.borrow().is_empty());
        confirm_preset("startweb", "ws1", (false, true), &mut agree).unwrap();
        assert_eq!(asked.borrow().len(), 1);
        assert!(asked.borrow()[0].contains("startweb") && asked.borrow()[0].contains("ws1"));
        let mut refuse = |_: &str| -> Result<bool> { Ok(false) };
        assert!(confirm_preset("startweb", "ws1", (false, true), &mut refuse).is_err());
        assert!(confirm_preset("startweb", "ws1", (true, true), &mut refuse).is_ok());
        // no terminal and no --yes: refused without asking
        let error = confirm_preset("startweb", "ws1", (false, false), &mut agree).unwrap_err();
        assert!(error.to_string().contains("--yes"));
        assert_eq!(asked.borrow().len(), 1);
        confirm_preset("startweb", "ws1", (true, false), &mut agree).unwrap();
        confirm_preset("ver", "ws1", (false, false), &mut agree).unwrap();
        // an unknown preset counts as mutating
        assert!(confirm_preset("nope", "ws1", (false, true), &mut refuse).is_err());
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
