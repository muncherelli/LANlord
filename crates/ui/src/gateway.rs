//! everything the windows do to a gateway, behind one trait so tests can
//! swap in a fake that never touches the network.

use anyhow::{anyhow, Result};
use lanlord_core::bootstrap::{self, AnsibleMode, AskPassword, Options};
use lanlord_core::network::{self, Internal};
use lanlord_core::settings::{self, Settings};
use lanlord_core::ssh::Session;
use lanlord_core::{apply, crypto};

pub trait Gateway: Send + Sync {
    /// set the gateway up for lanlord.
    fn bootstrap(&self, host: &str, ask: AskPassword) -> Result<()>;
    /// apply `modules` (all of them when empty) with the stored settings.
    fn apply(&self, host: &str, modules: &[String], ask: AskPassword) -> Result<()>;
    /// the decrypted settings text; empty when nothing is stored yet.
    fn load_settings_text(&self, host: &str, ask: AskPassword) -> Result<String>;
    /// parse-check `text`, then store exactly that text.
    fn save_settings_text(&self, host: &str, text: &str, ask: AskPassword) -> Result<()>;
    /// the internal interface and address, read without sudo.
    fn probe(&self, host: &str) -> Result<Internal>;

    /// the settings, parsed; fails when the stored text is not valid yaml.
    fn load_settings(&self, host: &str, ask: AskPassword) -> Result<Settings> {
        Settings::parse(&self.load_settings_text(host, ask)?)
    }

    fn save_settings(&self, host: &str, settings: &Settings, ask: AskPassword) -> Result<()> {
        self.save_settings_text(host, &settings.text(), ask)
    }
}

/// the message when there is no key to decrypt the settings with.
pub const NO_KEY: &str = "no key yet; run `lanlord keygen` in a terminal first";

/// your key, or a message saying how to get one.
fn key_or_message(found: Result<Option<age::x25519::Identity>>) -> Result<age::x25519::Identity> {
    found?.ok_or_else(|| anyhow!(NO_KEY))
}

fn key() -> Result<age::x25519::Identity> {
    key_or_message(crypto::load_default())
}

/// the gateway reached over ssh.
pub struct Ssh;

impl Gateway for Ssh {
    fn bootstrap(&self, host: &str, ask: AskPassword) -> Result<()> {
        let options = Options {
            host: host.to_string(),
            ansible: AnsibleMode::detect(),
            passwordless_sudo: false,
        };
        bootstrap::bootstrap(&options, ask)
    }

    fn apply(&self, host: &str, modules: &[String], ask: AskPassword) -> Result<()> {
        let identity = crypto::load_default()?;
        apply::apply(host, identity.as_ref(), modules, ask)
    }

    fn load_settings_text(&self, host: &str, ask: AskPassword) -> Result<String> {
        settings::load_text(host, &key()?, ask)
    }

    fn save_settings_text(&self, host: &str, text: &str, ask: AskPassword) -> Result<()> {
        settings::save_text(host, &key()?, text, ask)
    }

    fn probe(&self, host: &str) -> Result<Internal> {
        network::probe(&Session::new(host)?)
    }
}

#[cfg(test)]
pub use fake::Fake;

#[cfg(test)]
mod fake {
    use std::sync::Mutex;

    use super::*;

    /// a gateway that records what it was asked and answers from memory.
    pub struct Fake {
        pub calls: Mutex<Vec<String>>,
        pub stored: Mutex<String>,
        /// when set, every call fails with this message.
        pub fail: Mutex<Option<String>>,
        /// when set, calls ask for this password first.
        pub password_prompt: Mutex<Option<String>>,
        pub answer: Mutex<Option<String>>,
        pub internal: Mutex<Option<Internal>>,
    }

    impl Fake {
        pub fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                stored: Mutex::new(String::new()),
                fail: Mutex::new(None),
                password_prompt: Mutex::new(None),
                answer: Mutex::new(None),
                internal: Mutex::new(None),
            }
        }

        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn enter(&self, call: String, ask: AskPassword) -> Result<()> {
            self.calls.lock().unwrap().push(call);
            let prompt = self.password_prompt.lock().unwrap().clone();
            if let Some(prompt) = prompt {
                let password = ask(&prompt)?;
                *self.answer.lock().unwrap() = Some(password);
            }
            match self.fail.lock().unwrap().clone() {
                Some(message) => Err(anyhow!(message)),
                None => Ok(()),
            }
        }
    }

    impl Gateway for Fake {
        fn bootstrap(&self, host: &str, ask: AskPassword) -> Result<()> {
            self.enter(format!("bootstrap {host}"), ask)
        }

        fn apply(&self, host: &str, modules: &[String], ask: AskPassword) -> Result<()> {
            self.enter(format!("apply {host} [{}]", modules.join(",")), ask)
        }

        fn load_settings_text(&self, host: &str, ask: AskPassword) -> Result<String> {
            self.enter(format!("load {host}"), ask)?;
            Ok(self.stored.lock().unwrap().clone())
        }

        fn save_settings_text(&self, host: &str, text: &str, ask: AskPassword) -> Result<()> {
            Settings::parse(text)?;
            self.enter(format!("save {host}"), ask)?;
            *self.stored.lock().unwrap() = text.to_string();
            Ok(())
        }

        fn probe(&self, host: &str) -> Result<Internal> {
            self.calls.lock().unwrap().push(format!("probe {host}"));
            self.internal
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| anyhow!("no network"))
        }
    }

    #[test]
    fn the_fake_records_and_stores() {
        let fake = Fake::new();
        let mut ask = |_: &str| Ok(String::new());
        fake.save_settings("a@b", &Settings::parse("x: 1\n").unwrap(), &mut ask)
            .unwrap();
        assert_eq!(
            fake.load_settings("a@b", &mut ask).unwrap().text(),
            "x: 1\n"
        );
        assert!(fake.save_settings_text("a@b", "[oops", &mut ask).is_err());
        assert!(fake.probe("a@b").is_err());
        assert_eq!(fake.calls(), ["save a@b", "load a@b", "probe a@b"]);
    }

    #[test]
    fn a_missing_key_says_what_to_do() {
        let err = key_or_message(Ok(None)).unwrap_err().to_string();
        assert!(err.contains("lanlord keygen"), "{err}");
        assert!(key_or_message(Err(anyhow!("damaged"))).is_err());
    }
}
