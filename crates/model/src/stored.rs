//! the settings files on a gateway and their steps: the plain settings and the sealed secrets,
//! both yaml, each with its own list of steps (see [`crate::format`]).

use anyhow::Result;

use crate::format::{check, current, run_steps, Context, Step, TooNew};
use crate::settings::Settings;
use crate::{access, lan_networks};

/// which settings file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `settings.yml`: everything but the secrets, readable by the admins.
    Plain,
    /// `settings.age`: the secrets, sealed to the gateway's own key.
    Sealed,
}

/// version N is turned into N+1 by the step at position N (counting from 1). never reorder or
/// remove one: a file in the wild may be at any version.
pub const PLAIN_STEPS: &[Step<Settings>] = &[
    Step {
        name: "the single network becomes a networks list",
        run: lan_networks::migrate_legacy,
    },
    Step {
        name: "the role assignments are stored",
        run: access::migrate_settings,
    },
];

/// the sealed secrets have not changed shape since they were first stored.
pub const SEALED_STEPS: &[Step<Settings>] = &[];

impl Kind {
    pub fn steps(self) -> &'static [Step<Settings>] {
        match self {
            Kind::Plain => PLAIN_STEPS,
            Kind::Sealed => SEALED_STEPS,
        }
    }

    /// the newest format this lanlord reads and writes.
    pub fn current(self) -> u32 {
        current(self.steps())
    }

    pub fn file(self) -> &'static str {
        match self {
            Kind::Plain => "settings.yml",
            Kind::Sealed => "settings.age",
        }
    }
}

/// the settings in `text`, brought up to date in memory. a file from a newer lanlord is refused
/// with [`TooNew`]. a file that already is current comes back exactly as it was; an older one
/// (or one with no version) is run through the steps and stamped.
pub fn load(kind: Kind, text: &str, context: &Context) -> Result<Settings> {
    let mut settings = Settings::parse(text)?;
    let (found, by) = settings.format()?;
    let known = kind.current();
    check(found.unwrap_or(1), by.as_deref(), known)?;
    if found != Some(known) {
        run_steps(&mut settings, found.unwrap_or(1), kind.steps(), context)?;
        settings.stamp(known);
    }
    Ok(settings)
}

/// the text to store: `text` stamped with the current format of `kind`. a file from a newer
/// lanlord is refused. text that is not settings at all is stored as it is.
pub fn seal(kind: Kind, text: &str) -> Result<String> {
    let Ok(mut settings) = Settings::parse(text) else {
        return Ok(text.to_string());
    };
    let (found, by) = settings.format()?;
    check(found.unwrap_or(1), by.as_deref(), kind.current())?;
    if found != Some(kind.current()) || by.as_deref() != Some(crate::format::written_by()) {
        settings.stamp(kind.current());
    }
    Ok(settings.text())
}

/// refuse stored `text` that is from a newer lanlord; used before anything is written over it.
pub fn refuse_newer(kind: Kind, text: &str) -> Result<(), TooNew> {
    let Ok(settings) = Settings::parse(text) else {
        return Ok(());
    };
    match settings.format() {
        Ok((found, by)) => check(found.unwrap_or(1), by.as_deref(), kind.current()),
        Err(_) => Ok(()),
    }
}
