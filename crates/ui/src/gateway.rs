//! everything the windows do to a gateway, behind one trait so tests can
//! swap in a fake that never touches the network.

use anyhow::{anyhow, Result};
use lanlord_core::apply;
use lanlord_core::bootstrap::{self, AnsibleMode, AskPassword, Options};
use lanlord_core::crypto::{self, Identity};
use lanlord_core::exec::{self, Target};
use lanlord_core::leases::{self, Lease};
use lanlord_core::network::{self, Internal};
use lanlord_core::power::{self, Outcome, Power};
use lanlord_core::settings::{self, Document, Settings};
use lanlord_core::ssh::Session;
use lanlord_core::users::{self, Change};

pub trait Gateway: Send + Sync {
    /// set the gateway up for lanlord.
    fn bootstrap(&self, host: &str, ask: AskPassword) -> Result<()>;
    /// apply `modules` (all of them when empty) with the stored settings.
    fn apply(&self, host: &str, modules: &[String], ask: AskPassword) -> Result<()>;
    /// the plain settings text and which secrets are set; empty when nothing is stored yet.
    fn load_document(&self, host: &str, ask: AskPassword) -> Result<Document>;
    /// store exactly `plain` as the plain settings and apply the secret
    /// `updates` (path, new value); blank values change nothing.
    fn save_document(
        &self,
        host: &str,
        plain: &str,
        updates: &[(String, String)],
        ask: AskPassword,
    ) -> Result<()>;
    /// the internal interface and address, read without sudo.
    fn probe(&self, host: &str) -> Result<Internal>;
    /// change logins (names and flags in the plain settings, passwords in the
    /// secrets) and apply the nt4-domain module.
    fn change_users(&self, host: &str, changes: &[Change], ask: AskPassword) -> Result<()>;
    /// the dhcp leases the gateway has handed out.
    fn leases(&self, host: &str, ask: AskPassword) -> Result<Vec<Lease>>;
    /// run a command on the gateway, its output going to the console; returns the exit code.
    fn run(&self, host: &str, command: &[String]) -> Result<i32>;
    /// reboot or power off the gateway.
    fn power(&self, host: &str, power: Power, ask: AskPassword) -> Result<Outcome>;
    /// true when lanlord runs on the gateway itself: there is one server and no host list.
    fn is_local(&self) -> bool {
        false
    }

    /// the plain settings, parsed; fails when the stored text is not valid yaml.
    fn load_settings(&self, host: &str, ask: AskPassword) -> Result<Settings> {
        Settings::parse(&self.load_document(host, ask)?.plain)
    }

    #[allow(dead_code)] // the windows read, edit and save through `update_settings`
    /// store plain `settings`; the secrets stay as they are.
    fn save_settings(&self, host: &str, settings: &Settings, ask: AskPassword) -> Result<()> {
        self.save_document(host, &settings.text(), &[], ask)
    }
}

/// the message when there is no key to decrypt the secrets with.
pub const NO_KEY: &str = "no key yet; run `lanlord keygen` in a terminal first";

/// your key, or a message saying how to get one.
fn key_or_message(found: Result<Option<Identity>>) -> Result<Identity> {
    found?.ok_or_else(|| anyhow!(NO_KEY))
}

fn key() -> Result<Identity> {
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

    fn load_document(&self, host: &str, ask: AskPassword) -> Result<Document> {
        settings::load_document(host, &key()?, ask)
    }

    fn save_document(
        &self,
        host: &str,
        plain: &str,
        updates: &[(String, String)],
        ask: AskPassword,
    ) -> Result<()> {
        settings::save_document(host, &key()?, plain, updates, ask)
    }

    fn probe(&self, host: &str) -> Result<Internal> {
        network::probe(&Session::new(host)?)
    }

    fn change_users(&self, host: &str, changes: &[Change], ask: AskPassword) -> Result<()> {
        users::change_all(host, &key()?, changes, ask).map(|_| ())
    }

    fn leases(&self, host: &str, ask: AskPassword) -> Result<Vec<Lease>> {
        leases::read_on(host, ask)
    }

    fn run(&self, host: &str, command: &[String]) -> Result<i32> {
        let target = Target::Gateway { host: host.into() };
        exec::exec_detached(&target, command)
    }

    fn power(&self, host: &str, action: Power, ask: AskPassword) -> Result<Outcome> {
        power::run(host, action, ask)
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
        /// the plain settings.
        pub stored: Mutex<String>,
        /// the secrets, by path; only their paths are ever read back.
        pub secrets: Mutex<Vec<(String, String)>>,
        /// when set, every call fails with this message.
        pub fail: Mutex<Option<String>>,
        /// when set, calls ask for this password first.
        pub password_prompt: Mutex<Option<String>>,
        pub answer: Mutex<Option<String>>,
        pub internal: Mutex<Option<Internal>>,
        pub leases: Mutex<Vec<Lease>>,
        /// the exit code `run` reports.
        pub exit: Mutex<i32>,
        /// what `is_local` says.
        pub local: Mutex<bool>,
        /// what `power` reports.
        pub outcome: Mutex<Outcome>,
    }

    impl Fake {
        pub fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                stored: Mutex::new(String::new()),
                secrets: Mutex::new(Vec::new()),
                fail: Mutex::new(None),
                password_prompt: Mutex::new(None),
                answer: Mutex::new(None),
                internal: Mutex::new(None),
                leases: Mutex::new(Vec::new()),
                exit: Mutex::new(0),
                local: Mutex::new(false),
                outcome: Mutex::new(Outcome::Dropped),
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

        fn load_document(&self, host: &str, ask: AskPassword) -> Result<Document> {
            self.enter(format!("load {host}"), ask)?;
            Ok(Document {
                plain: self.stored.lock().unwrap().clone(),
                secrets_set: self
                    .secrets
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|(p, _)| p.clone())
                    .collect(),
                legacy: false,
            })
        }

        fn save_document(
            &self,
            host: &str,
            plain: &str,
            updates: &[(String, String)],
            ask: AskPassword,
        ) -> Result<()> {
            let leaked = Settings::parse(plain)?.secret_paths();
            if !leaked.is_empty() {
                return Err(anyhow!("secrets in the plain text: {}", leaked.join(", ")));
            }
            self.enter(format!("save {host}"), ask)?;
            *self.stored.lock().unwrap() = plain.to_string();
            let mut secrets = self.secrets.lock().unwrap();
            for (path, value) in updates.iter().filter(|(_, v)| !v.is_empty()) {
                secrets.retain(|(p, _)| p != path);
                secrets.push((path.clone(), value.clone()));
            }
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

        fn change_users(&self, host: &str, changes: &[Change], ask: AskPassword) -> Result<()> {
            self.enter(format!("users {host} {}", changes.len()), ask)?;
            let stored = self.stored.lock().unwrap().clone();
            let edited = users::edit_split(stored.as_bytes(), changes)?;
            *self.stored.lock().unwrap() = edited.plain;
            let mut secrets = self.secrets.lock().unwrap();
            secrets.retain(|(p, _)| !edited.removed.contains(p));
            for (path, value) in edited.secrets {
                secrets.retain(|(p, _)| *p != path);
                secrets.push((path, value));
            }
            self.calls
                .lock()
                .unwrap()
                .push(format!("apply {host} [nt4-domain]"));
            Ok(())
        }

        fn leases(&self, host: &str, ask: AskPassword) -> Result<Vec<Lease>> {
            self.enter(format!("leases {host}"), ask)?;
            Ok(self.leases.lock().unwrap().clone())
        }

        fn power(&self, host: &str, action: Power, ask: AskPassword) -> Result<Outcome> {
            self.enter(format!("power {host} {}", action.doing()), ask)?;
            Ok(*self.outcome.lock().unwrap())
        }

        fn is_local(&self) -> bool {
            *self.local.lock().unwrap()
        }

        fn run(&self, host: &str, command: &[String]) -> Result<i32> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("run {host} {}", command.join(" ")));
            if let Some(message) = self.fail.lock().unwrap().clone() {
                return Err(anyhow!(message));
            }
            lanlord_core::log::emit(
                lanlord_core::log::Kind::Out,
                format!("output of {}", command.join(" ")),
            );
            Ok(*self.exit.lock().unwrap())
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
        assert!(fake.save_document("a@b", "[oops", &[], &mut ask).is_err());
        assert!(fake
            .save_document("a@b", "p:\n  password: x\n", &[], &mut ask)
            .is_err());
        assert!(fake.probe("a@b").is_err());
        assert_eq!(fake.calls(), ["save a@b", "load a@b", "probe a@b"]);
    }

    #[test]
    fn a_missing_key_says_what_to_do() {
        let err = key_or_message(Ok(None)).err().unwrap().to_string();
        assert!(err.contains("lanlord keygen"), "{err}");
        assert!(key_or_message(Err(anyhow!("damaged"))).is_err());
    }
}
