//! what the engine remembers between runs: a fingerprint of each password it set, so a password is
//! set again only when it changes in LANlord, never because Windows changed it. the fingerprint is
//! an HMAC under a key that lives next to it, so a state file reveals nothing about a password.

use std::collections::BTreeMap;

use anyhow::{bail, Result};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use super::host::Host;
use super::Secret;

pub struct State {
    dir: String,
    key: Vec<u8>,
    prints: BTreeMap<String, String>,
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl State {
    fn state_path(dir: &str) -> String {
        format!("{dir}/state.json")
    }

    fn key_path(dir: &str) -> String {
        format!("{dir}/fingerprint.key")
    }

    /// read the state; a missing key is made when `create` is set (an apply), and not otherwise
    /// (a plan changes nothing).
    pub fn load(host: &dyn Host, dir: &str, create: bool) -> Result<Self> {
        let key = match host.read(&Self::key_path(dir))? {
            Some(key) if key.len() >= 32 => key,
            _ if create => {
                let mut key = vec![0u8; 32];
                getrandom::fill(&mut key).map_err(|e| anyhow::anyhow!("no random bytes: {e}"))?;
                host.mkdir(dir, 0o700)?;
                host.write(&Self::key_path(dir), &key, 0o600)?;
                key
            }
            _ => vec![0u8; 32],
        };
        let prints = match host.read(&Self::state_path(dir))? {
            Some(bytes) => {
                let value: serde_json::Value = serde_json::from_slice(&bytes)
                    .map_err(|e| anyhow::anyhow!("the engine state is not readable: {e}"))?;
                value
                    .get("fingerprints")
                    .and_then(|v| v.as_object())
                    .map(|map| {
                        map.iter()
                            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                            .collect()
                    })
                    .unwrap_or_default()
            }
            None => BTreeMap::new(),
        };
        Ok(Self {
            dir: dir.into(),
            key,
            prints,
        })
    }

    pub fn fingerprint(&self, secret: &Secret) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).expect("hmac takes any key length");
        mac.update(secret.expose().as_bytes());
        hex(&mac.finalize().into_bytes())
    }

    /// whether `secret` is the one last set under `name`.
    pub fn is_current(&self, name: &str, secret: &Secret) -> bool {
        self.prints.get(name) == Some(&self.fingerprint(secret))
    }

    pub fn remember(&mut self, host: &dyn Host, name: &str, secret: &Secret) -> Result<()> {
        self.prints.insert(name.into(), self.fingerprint(secret));
        self.save(host)
    }

    pub fn forget(&mut self, host: &dyn Host, name: &str) -> Result<()> {
        if self.prints.remove(name).is_some() {
            self.save(host)?;
        }
        Ok(())
    }

    fn save(&self, host: &dyn Host) -> Result<()> {
        if self.dir.is_empty() {
            bail!("no state directory");
        }
        let doc = serde_json::json!({ "fingerprints": self.prints });
        host.mkdir(&self.dir, 0o700)?;
        host.write(
            &Self::state_path(&self.dir),
            serde_json::to_string_pretty(&doc)?.as_bytes(),
            0o600,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::host::FakeHost;

    #[test]
    fn a_password_is_current_until_it_changes_in_lanlord() {
        let host = FakeHost::new();
        let mut state = State::load(&host, "/s", true).unwrap();
        let first = Secret::new("hunter2");
        assert!(!state.is_current("user:dana", &first));
        state.remember(&host, "user:dana", &first).unwrap();
        let again = State::load(&host, "/s", true).unwrap();
        assert!(again.is_current("user:dana", &first));
        assert!(!again.is_current("user:dana", &Secret::new("other")));
    }

    #[test]
    fn the_state_file_holds_no_password() {
        let host = FakeHost::new();
        let mut state = State::load(&host, "/s", true).unwrap();
        state
            .remember(&host, "admin", &Secret::new("hunter2"))
            .unwrap();
        assert!(!host.text("/s/state.json").unwrap().contains("hunter2"));
        assert_eq!(host.stat("/s/state.json").unwrap().unwrap().mode, 0o600);
        assert_eq!(
            host.stat("/s/fingerprint.key").unwrap().unwrap().mode,
            0o600
        );
    }

    #[test]
    fn a_plan_does_not_make_the_key() {
        let host = FakeHost::new();
        State::load(&host, "/s", false).unwrap();
        assert!(host.text("/s/fingerprint.key").is_none());
    }
}
