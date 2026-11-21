//! the modules the engine does natively, and how one is picked up by the dispatcher.

use anyhow::Result;
use lanlord_model::apply::ApplyEvent;
use lanlord_model::engine::ChangeView;
use serde_yaml_ng::Value;

use super::host::Host;
use super::{Events, Tally};

pub mod nt4;

/// the modules in the order a plain apply runs them (the order of `site.yml`).
pub const ORDER: &[&str] = &[
    "gateway",
    "docker",
    "nt4-domain",
    "web-proxy",
    "dns",
    "time-server",
    "webui-login",
];

/// whether the engine has an implementation of `module`.
pub fn has_native(module: &str) -> bool {
    module == nt4::ID
}

/// what an apply of `module` would change. `Unsupported` is in the error when the playbook has to
/// do it. `shadow` is a plan after Ansible ran the module, which leaves no record of the passwords
/// it set: a password is then not reported as changed.
pub fn plan(
    module: &str,
    host: &dyn Host,
    doc: &Value,
    state_dir: &str,
    shadow: bool,
) -> Result<Vec<ChangeView>> {
    match module {
        nt4::ID => nt4::plan(host, doc, state_dir, shadow),
        _ => Err(anyhow::Error::new(super::Unsupported(format!(
            "{module} runs with Ansible"
        )))),
    }
}

/// do `module`. `Unsupported` is in the error when nothing was changed and the playbook has to do it.
pub fn apply(
    module: &str,
    host: &dyn Host,
    doc: &Value,
    state_dir: &str,
    run: &str,
    events: &mut Events,
) -> Result<Tally> {
    match module {
        nt4::ID => nt4::apply(host, doc, state_dir, run, events),
        _ => Err(anyhow::Error::new(super::Unsupported(format!(
            "{module} runs with Ansible"
        )))),
    }
}

/// record the passwords in the settings as set, without setting them; what was recorded, by name.
pub fn record(module: &str, host: &dyn Host, doc: &Value, state_dir: &str) -> Result<Vec<String>> {
    match module {
        nt4::ID => nt4::record(host, doc, state_dir),
        _ => Err(anyhow::anyhow!("{module} keeps no password fingerprints")),
    }
}

/// the log line for a native module's changes that are still there after Ansible ran it.
pub fn shadow_lines(module: &str, left: &[ChangeView]) -> Vec<ApplyEvent> {
    left.iter()
        .map(|change| ApplyEvent::Log {
            text: format!(
                "engine shadow: {module}: after Ansible the engine would still change {}: {}",
                change.resource, change.summary
            ),
        })
        .collect()
}
