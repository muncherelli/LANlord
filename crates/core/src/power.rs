//! reboot or power off the gateway.

use anyhow::{bail, Result};

use crate::bootstrap::{run_as_root, AskPassword};
use crate::config;
use crate::ssh::Session;

pub use lanlord_model::power::*;

/// reboot or power off `host`; sudo's password is asked for when needed.
pub fn run(host: &str, power: Power, ask_password: AskPassword) -> Result<Outcome> {
    run_with(&Session::new(host)?, power, ask_password)
}

pub(crate) fn run_with(
    session: &Session,
    power: Power,
    ask_password: AskPassword,
) -> Result<Outcome> {
    let admin = config::admin(session)?;
    let mut cancelled = false;
    let mut ask = |what: &str| {
        let answer = ask_password(what);
        cancelled |= answer.is_err();
        answer
    };
    match run_as_root(session, &admin, power.script().to_string(), &mut ask) {
        Ok(0) => Ok(Outcome::Done),
        Ok(code) => bail!("{} failed with status {code}", power.doing()),
        Err(err) if cancelled => Err(err),
        Err(_) => Ok(Outcome::Dropped),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::testgw::no_password;

    fn record(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("lanlord-power-{}-{name}", std::process::id()))
    }

    #[test]
    fn the_command_runs_as_root_and_a_dropped_connection_is_expected() {
        for (power, text) in [
            (Power::Reboot, "systemctl reboot"),
            (Power::PowerOff, "systemctl poweroff"),
        ] {
            let path = record(text.split(' ').nth(1).unwrap());
            let done = run_with(
                &Session::fake_gateway(true, &path, 0),
                power,
                &mut no_password,
            );
            assert_eq!(done.unwrap(), Outcome::Done);
            assert!(std::fs::read_to_string(&path).unwrap().contains(text));
            let _ = std::fs::remove_file(&path);
        }
        let path = record("dropped");
        let dropped = run_with(
            &Session::fake_gateway(true, &path, 255),
            Power::Reboot,
            &mut no_password,
        );
        assert_eq!(dropped.unwrap(), Outcome::Dropped);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn failures_and_a_cancelled_password_are_errors() {
        let path = record("failed");
        let failed = run_with(
            &Session::fake_gateway(true, &path, 1),
            Power::Reboot,
            &mut no_password,
        );
        assert!(failed.unwrap_err().to_string().contains("rebooting failed"));
        let cancelled = run_with(
            &Session::fake_gateway(false, &path, 0),
            Power::PowerOff,
            &mut no_password,
        );
        assert!(cancelled.is_err());
        let mut asked = |_: &str| Ok("pw".to_string());
        let with_password = run_with(
            &Session::fake_gateway(false, &path, 0),
            Power::Reboot,
            &mut asked,
        );
        assert_eq!(with_password.unwrap(), Outcome::Done);
        let _ = std::fs::remove_file(&path);
        assert_eq!(Power::PowerOff.doing(), "shutting down");
    }
}
