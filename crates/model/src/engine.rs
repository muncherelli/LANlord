//! the types of the native apply engine that travel between the gateway and a front end: which
//! engine runs a module, and what an apply would change. no i/o. nothing here holds a secret:
//! a change that comes from one says only that it will be set, and is marked `sensitive`.

use serde::{Deserialize, Serialize};

use crate::settings::Settings;

/// which engine applies a module.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// the playbooks, as before.
    #[default]
    Ansible,
    /// the native plan is made and compared, then the playbooks run.
    Shadow,
    /// the native engine runs it; what it cannot do goes to the playbooks for that run.
    Native,
}

impl Mode {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "ansible" => Some(Self::Ansible),
            "shadow" => Some(Self::Shadow),
            "native" => Some(Self::Native),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ansible => "ansible",
            Self::Shadow => "shadow",
            Self::Native => "native",
        }
    }
}

/// the modules the native engine can apply, with the mode a build uses when the settings say
/// nothing. at first every module is on Ansible.
pub const DEFAULTS: &[(&str, Mode)] = &[("nt4-domain", Mode::Ansible)];

/// the mode for `module`: the `engine:` section of the settings, else the build's default.
/// a value that is not a mode is ignored here; `validate` refuses it before it is stored.
pub fn mode_for(settings: Option<&Settings>, module: &str) -> Mode {
    let set = settings
        .and_then(|s| s.section("engine"))
        .and_then(|section| section.get(module))
        .and_then(|value| value.as_str())
        .and_then(Mode::parse);
    set.or_else(|| {
        DEFAULTS
            .iter()
            .find(|(id, _)| *id == module)
            .map(|(_, mode)| *mode)
    })
    .unwrap_or_default()
}

/// what a step does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Create,
    Update,
    Replace,
    Remove,
    Run,
}

/// one planned change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeView {
    pub resource: String,
    pub action: Action,
    pub summary: String,
    /// a unified diff for a file; never present when `sensitive`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    /// the change comes from a secret: the summary says it happens, never what the value is.
    #[serde(default)]
    pub sensitive: bool,
}

/// what applying one module would do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModulePlan {
    pub module: String,
    /// the engine that would run it: `ansible` or `native`.
    pub engine: String,
    /// why it is not native, when it is not ("runs with Ansible; preview not available").
    #[serde(default)]
    pub reason: String,
    pub changes: Vec<ChangeView>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_ansible() {
        assert_eq!(mode_for(None, "nt4-domain"), Mode::Ansible);
        let empty = Settings::parse("gateway:\n  motd: x\n").unwrap();
        assert_eq!(mode_for(Some(&empty), "nt4-domain"), Mode::Ansible);
    }

    #[test]
    fn the_settings_choose_the_engine() {
        for (text, mode) in [
            ("ansible", Mode::Ansible),
            ("shadow", Mode::Shadow),
            ("native", Mode::Native),
        ] {
            let settings = Settings::parse(&format!("engine:\n  nt4-domain: {text}\n")).unwrap();
            assert_eq!(mode_for(Some(&settings), "nt4-domain"), mode);
            assert_eq!(Mode::parse(mode.as_str()), Some(mode));
        }
    }

    #[test]
    fn the_section_is_checked_by_validate() {
        use crate::validate::settings_document;
        assert!(settings_document("engine:\n  nt4-domain: native\n").is_ok());
        assert!(settings_document("engine:\n  nt4-domain: fast\n").is_err());
    }

    #[test]
    fn the_section_never_reaches_the_playbooks() {
        let kept = crate::validate::for_ansible("engine:\n  nt4-domain: native\n").unwrap();
        assert!(!kept.contains("engine"), "{kept}");
    }
}
