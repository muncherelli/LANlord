//! where lanlord keeps its files on the client.

use std::path::PathBuf;

/// ~/.config/lanlord, honouring XDG_CONFIG_HOME and APPDATA.
pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(base.join("lanlord"))
}

/// the age identity that unlocks the settings.
pub fn identity_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("identity"))
}
