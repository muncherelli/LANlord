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
/// do it.
pub fn plan(
    module: &str,
    host: &dyn Host,
    doc: &Value,
    state_dir: &str,
) -> Result<Vec<ChangeView>> {
    match module {
        nt4::ID => nt4::plan(host, doc, state_dir),
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
