use anyhow::{bail, Result};

/// run `command` on `host` and stream its output back.
pub fn run(_host: &str, _command: &[String]) -> Result<()> {
    bail!("run is not implemented yet")
}
