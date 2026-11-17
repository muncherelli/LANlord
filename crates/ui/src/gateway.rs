//! everything the windows do to a gateway, behind one trait so tests can
//! swap in a fake that never touches the network.

use anyhow::{anyhow, Result};
use lanlord_core::account;
use lanlord_core::apply;
use lanlord_core::bootstrap::{self, AnsibleMode, AskPassword, Options};
use lanlord_core::clock::{self, Change as ClockChange, ClockState, Stored, ZoneNow};
use lanlord_core::crypto::{self, Identity};
use lanlord_core::exec::{self, Target};
use lanlord_core::leases::{self, Lease};
use lanlord_core::logon;
use lanlord_core::netstatus::{self, InterfaceStatus};
use lanlord_core::network::{self, Internal};
use lanlord_core::nt4;
use lanlord_core::power::{self, Outcome, Power};
use lanlord_core::purge::{self, PurgeOptions};
use lanlord_core::server::{self, ServerFacts};
use lanlord_core::settings::{self, Document, Settings};
use lanlord_core::ssh::Session;
use lanlord_core::users::{self, Change};

use crate::demo_clock;

/// an administrator to create; the password is wiped when this is dropped.
pub struct NewAdmin {
    pub name: String,
    pub password: zeroize::Zeroizing<String>,
    pub ssh_key: Option<String>,
    pub passwordless_sudo: bool,
}

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
    /// set the account of machine `name` back to its first password, so it can join again.
    fn reset_machine(&self, host: &str, name: &str, ask: AskPassword) -> Result<()>;
    /// create an administrator login, in admins and sudo, with an optional ssh key.
    fn create_admin(&self, host: &str, new: &NewAdmin, ask: AskPassword) -> Result<()>;
    /// the dhcp leases the gateway has handed out.
    fn leases(&self, host: &str, ask: AskPassword) -> Result<Vec<Lease>>;
    /// the names of the logon scripts in the netlogon share.
    fn logon_scripts(&self, host: &str, ask: AskPassword) -> Result<Vec<String>>;
    /// one logon script with unix line endings; `None` when it does not exist yet.
    fn read_logon(&self, host: &str, name: &str, ask: AskPassword) -> Result<Option<String>>;
    /// the generated `lanlord.bat`, for reference.
    fn read_generated_logon(&self, host: &str, ask: AskPassword) -> Result<Option<String>>;
    /// store a logon script (saved with dos line endings).
    fn write_logon(&self, host: &str, name: &str, text: &str, ask: AskPassword) -> Result<()>;
    /// run a command on the gateway, its output going to the console; returns the exit code.
    fn run(&self, host: &str, command: &[String]) -> Result<i32>;
    /// reboot or power off the gateway.
    fn power(&self, host: &str, power: Power, ask: AskPassword) -> Result<Outcome>;
    /// what the server is: its system, uptime and interfaces (read-only, no sudo).
    fn server_facts(&self, host: &str, ask: AskPassword) -> Result<ServerFacts>;
    /// the status of each network connection (read-only, no sudo).
    fn network_status(&self, host: &str) -> Result<Vec<InterfaceStatus>>;
    /// everything a purge would remove, as lines; nothing is changed.
    fn purge_preview(
        &self,
        host: &str,
        remove_admins: bool,
        ask: AskPassword,
    ) -> Result<Vec<String>>;
    /// remove lanlord from the server; the lines say what was removed.
    fn purge(&self, host: &str, remove_admins: bool, ask: AskPassword) -> Result<Vec<String>>;
    /// the server's time, time zone and internet time; asks for nothing.
    fn clock_state(&self, host: &str) -> Result<ClockState>;
    /// the time zones the server knows.
    fn clock_zones(&self, host: &str) -> Result<Vec<String>>;
    /// what a time zone looks like on the server right now.
    fn clock_zone_now(&self, host: &str, zone: &str) -> Result<ZoneNow>;
    /// change the time, the time zone or internet time; what the server said, and whether the
    /// zone and servers were also kept in the settings.
    fn clock_apply(
        &self,
        host: &str,
        change: &ClockChange,
        ask: AskPassword,
    ) -> Result<(Vec<String>, Stored)>;
    /// ask the time service to synchronize now; what it said.
    fn clock_update_now(&self, host: &str, ask: AskPassword) -> Result<Vec<String>>;
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
        if demo() {
            return Ok(Document {
                plain: DEMO_SETTINGS.to_string(),
                ..Document::default()
            });
        }
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

    fn reset_machine(&self, host: &str, name: &str, ask: AskPassword) -> Result<()> {
        nt4::reset_machine(&Session::new(host)?, name, ask)
    }

    fn create_admin(&self, host: &str, new: &NewAdmin, ask: AskPassword) -> Result<()> {
        account::create_admin(
            &Session::new(host)?,
            host,
            &new.name,
            &new.password,
            new.ssh_key.as_deref(),
            new.passwordless_sudo,
            ask,
        )
    }

    fn leases(&self, host: &str, ask: AskPassword) -> Result<Vec<Lease>> {
        if demo() {
            return Ok(leases::parse(DEMO_LEASES));
        }
        leases::read_on(host, ask)
    }

    fn logon_scripts(&self, host: &str, ask: AskPassword) -> Result<Vec<String>> {
        logon::list(&Session::new(host)?, ask)
    }

    fn read_logon(&self, host: &str, name: &str, ask: AskPassword) -> Result<Option<String>> {
        logon::read(&Session::new(host)?, name, ask)
    }

    fn read_generated_logon(&self, host: &str, ask: AskPassword) -> Result<Option<String>> {
        logon::read_generated(&Session::new(host)?, ask)
    }

    fn write_logon(&self, host: &str, name: &str, text: &str, ask: AskPassword) -> Result<()> {
        logon::write(&Session::new(host)?, name, text, ask)
    }

    fn run(&self, host: &str, command: &[String]) -> Result<i32> {
        let target = Target::Gateway { host: host.into() };
        exec::exec_detached(&target, command)
    }

    fn power(&self, host: &str, action: Power, ask: AskPassword) -> Result<Outcome> {
        power::run(host, action, ask)
    }

    fn server_facts(&self, host: &str, _ask: AskPassword) -> Result<ServerFacts> {
        if demo() {
            return Ok(server::demo());
        }
        server::read(&Session::new(host)?)
    }

    fn network_status(&self, host: &str) -> Result<Vec<InterfaceStatus>> {
        if demo() {
            return Ok(netstatus::demo());
        }
        netstatus::read(&Session::new(host)?)
    }

    fn purge_preview(
        &self,
        host: &str,
        remove_admins: bool,
        ask: AskPassword,
    ) -> Result<Vec<String>> {
        if demo() {
            return Ok(demo_purge());
        }
        purge::preview(&Session::new(host)?, PurgeOptions { remove_admins }, ask)
    }

    fn purge(&self, host: &str, remove_admins: bool, ask: AskPassword) -> Result<Vec<String>> {
        if demo() {
            return Err(anyhow!("demo mode: nothing is removed"));
        }
        purge::purge(
            &Session::new(host)?,
            host,
            PurgeOptions { remove_admins },
            ask,
        )
    }
    fn clock_state(&self, host: &str) -> Result<ClockState> {
        if let Some(container) = demo_clock::mode() {
            return Ok(demo_clock::state(container));
        }
        clock::read(&Session::new(host)?)
    }

    fn clock_zones(&self, host: &str) -> Result<Vec<String>> {
        if demo_clock::mode().is_some() {
            return Ok(demo_clock::zones());
        }
        clock::zones(&Session::new(host)?)
    }

    fn clock_zone_now(&self, host: &str, zone: &str) -> Result<ZoneNow> {
        if demo_clock::mode().is_some() {
            return Ok(demo_clock::zone_now(zone));
        }
        clock::zone_now(&Session::new(host)?, zone)
    }

    fn clock_apply(
        &self,
        host: &str,
        change: &ClockChange,
        ask: AskPassword,
    ) -> Result<(Vec<String>, Stored)> {
        if let Some(container) = demo_clock::mode() {
            return Ok(demo_clock::apply(change, container));
        }
        // no key just means the change is not kept in the settings
        let identity = crypto::load_default()?;
        clock::apply_stored(&Session::new(host)?, host, identity.as_ref(), change, ask)
    }

    fn clock_update_now(&self, host: &str, ask: AskPassword) -> Result<Vec<String>> {
        if demo_clock::mode().is_some() {
            return Ok(vec!["Server: 162.159.200.1 (pool.ntp.org)".into()]);
        }
        clock::update_now(&Session::new(host)?, ask)
    }
}

/// `LANLORD_FAKE_SERVER=1` answers the server windows with made-up facts, so they can be tried
/// without a server; a purge never runs in it.
fn demo() -> bool {
    std::env::var("LANLORD_FAKE_SERVER").as_deref() == Ok("1")
}

const DEMO_SETTINGS: &str = "docker:\n  internal_address: 172.16.120.1/22\ndns:\n  zone: retronet\n  dhcp:\n    range: [172.16.120.100, 172.16.120.200]\n    lease: 7d\n";

const DEMO_LEASES: &str = "1795000000 02:00:00:00:00:01 172.16.120.101 ws1 *\n1795000000 02:00:00:00:00:02 172.16.120.102 ws2 *\n1795000000 02:00:00:00:00:03 172.16.120.103 nt4pc *\n";

fn demo_purge() -> Vec<String> {
    [
        "data: 12 KB in /opt/lanlord/config",
        "data: 5120 KB in /opt/lanlord/nt4/lib/profiles",
        "note: settings.age is the only copy of the stored secrets",
        "remove container lanlord-nt4",
        "remove container lanlord-dns",
        "remove image ghcr.io/muncherelli/lanlord-samba-nt4:3f2a9c1d7e44",
        "remove unit lanlord-firewall.service",
        "remove the firewall table inet lanlord",
        "remove /opt/lanlord",
        "remove the service account lanlord",
        "remove the group admins",
        "remove the sudo rules admins",
        "kept: the docker engine, /etc/motd and the accounts of people",
        "dry run: nothing was changed",
    ]
    .map(String::from)
    .to_vec()
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
        /// the logon scripts by name, with unix line endings.
        pub logon: Mutex<std::collections::BTreeMap<String, String>>,
        /// the generated lanlord.bat.
        pub generated: Mutex<Option<String>>,
        /// the exit code `run` reports.
        pub exit: Mutex<i32>,
        /// what `is_local` says.
        pub local: Mutex<bool>,
        /// what `power` reports.
        pub outcome: Mutex<Outcome>,
        /// what the server facts say; the demo server when not set.
        pub facts: Mutex<Option<ServerFacts>>,
        /// the lines a purge preview and a purge answer with.
        pub purge_lines: Mutex<Vec<String>>,
        /// when set, reading the network status fails with this.
        pub network_error: Mutex<Option<String>>,
        /// the server's clock, changed by `clock_apply`.
        pub clock: Mutex<ClockState>,
        /// what `clock_apply` says about keeping the change in the settings.
        pub kept: Mutex<Stored>,
        /// the time zones it lists.
        pub zones: Mutex<Vec<String>>,
        /// the small reads that are not tasks (the taskbar clock).
        pub reads: Mutex<Vec<String>>,
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
                logon: Mutex::new(std::collections::BTreeMap::new()),
                generated: Mutex::new(None),
                exit: Mutex::new(0),
                local: Mutex::new(false),
                outcome: Mutex::new(Outcome::Dropped),
                facts: Mutex::new(None),
                purge_lines: Mutex::new(vec!["remove /opt/lanlord".into()]),
                network_error: Mutex::new(None),
                kept: Mutex::new(Stored::Yes),
                clock: Mutex::new(ClockState {
                    timezone: "Etc/UTC".into(),
                    utc_secs: 1_794_759_123,
                    offset_secs: 0,
                    ntp: true,
                    synchronized: true,
                    container: false,
                    sync: clock::Sync::Timesyncd,
                    servers: Vec::new(),
                    status: vec!["Server: 10.0.0.1 (pool.ntp.org)".into()],
                }),
                reads: Mutex::new(Vec::new()),
                zones: Mutex::new(
                    [
                        "Africa/Cairo",
                        "America/Argentina/Buenos_Aires",
                        "America/New_York",
                    ]
                    .iter()
                    .chain(&["Asia/Kolkata", "Etc/UTC", "Europe/Berlin", "UTC"])
                    .map(|z| z.to_string())
                    .collect(),
                ),
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

        fn reset_machine(&self, host: &str, name: &str, ask: AskPassword) -> Result<()> {
            self.enter(format!("reset {host} {name}"), ask)
        }

        fn create_admin(&self, host: &str, new: &NewAdmin, ask: AskPassword) -> Result<()> {
            self.enter(
                format!(
                    "create-admin {host} {} key={} nopasswd={} pw={}",
                    new.name,
                    new.ssh_key.is_some(),
                    new.passwordless_sudo,
                    new.password.as_str()
                ),
                ask,
            )
        }

        fn leases(&self, host: &str, ask: AskPassword) -> Result<Vec<Lease>> {
            self.enter(format!("leases {host}"), ask)?;
            Ok(self.leases.lock().unwrap().clone())
        }

        fn logon_scripts(&self, host: &str, ask: AskPassword) -> Result<Vec<String>> {
            self.enter(format!("logon-list {host}"), ask)?;
            Ok(self.logon.lock().unwrap().keys().cloned().collect())
        }

        fn read_logon(&self, host: &str, name: &str, ask: AskPassword) -> Result<Option<String>> {
            self.enter(format!("logon-read {host} {name}"), ask)?;
            Ok(self.logon.lock().unwrap().get(name).cloned())
        }

        fn read_generated_logon(&self, host: &str, ask: AskPassword) -> Result<Option<String>> {
            self.enter(format!("logon-generated {host}"), ask)?;
            Ok(self.generated.lock().unwrap().clone())
        }

        fn write_logon(&self, host: &str, name: &str, text: &str, ask: AskPassword) -> Result<()> {
            if !logon::valid_script_name(name) {
                return Err(anyhow!("{name} is not a valid script name"));
            }
            self.enter(format!("logon-write {host} {name}"), ask)?;
            self.logon
                .lock()
                .unwrap()
                .insert(name.to_string(), logon::to_unix(text));
            Ok(())
        }

        fn power(&self, host: &str, action: Power, ask: AskPassword) -> Result<Outcome> {
            self.enter(format!("power {host} {}", action.doing()), ask)?;
            Ok(*self.outcome.lock().unwrap())
        }

        fn is_local(&self) -> bool {
            *self.local.lock().unwrap()
        }

        fn server_facts(&self, host: &str, ask: AskPassword) -> Result<ServerFacts> {
            self.enter(format!("facts {host}"), ask)?;
            let facts = self.facts.lock().unwrap().clone();
            Ok(facts.unwrap_or_else(server::demo))
        }

        fn network_status(&self, host: &str) -> Result<Vec<InterfaceStatus>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("network-status {host}"));
            match self.network_error.lock().unwrap().clone() {
                Some(error) => Err(anyhow!(error)),
                None => Ok(netstatus::demo()),
            }
        }

        fn purge_preview(
            &self,
            host: &str,
            remove_admins: bool,
            ask: AskPassword,
        ) -> Result<Vec<String>> {
            self.enter(format!("purge-preview {host} admins={remove_admins}"), ask)?;
            Ok(self.purge_lines.lock().unwrap().clone())
        }

        fn purge(&self, host: &str, remove_admins: bool, ask: AskPassword) -> Result<Vec<String>> {
            self.enter(format!("purge {host} admins={remove_admins}"), ask)?;
            Ok(self.purge_lines.lock().unwrap().clone())
        }

        fn clock_state(&self, host: &str) -> Result<ClockState> {
            // quiet reads are not tasks, so they are kept apart from `calls`
            self.reads
                .lock()
                .unwrap()
                .push(format!("clock-state {host}"));
            match self.fail.lock().unwrap().clone() {
                Some(message) => Err(anyhow!(message)),
                None => Ok(self.clock.lock().unwrap().clone()),
            }
        }

        fn clock_zones(&self, host: &str) -> Result<Vec<String>> {
            self.reads
                .lock()
                .unwrap()
                .push(format!("clock-zones {host}"));
            Ok(self.zones.lock().unwrap().clone())
        }

        fn clock_zone_now(&self, host: &str, zone: &str) -> Result<ZoneNow> {
            self.reads
                .lock()
                .unwrap()
                .push(format!("clock-zone-now {host} {zone}"));
            Ok(if zone.contains("Berlin") {
                ZoneNow {
                    offset_secs: 3600,
                    abbrev: "CET".into(),
                }
            } else {
                ZoneNow {
                    offset_secs: 0,
                    abbrev: "UTC".into(),
                }
            })
        }

        fn clock_apply(
            &self,
            host: &str,
            change: &ClockChange,
            ask: AskPassword,
        ) -> Result<(Vec<String>, Stored)> {
            self.enter(
                format!(
                    "clock-apply {host} zone={:?} time={:?} ntp={:?}",
                    change.timezone,
                    change.time,
                    change
                        .ntp
                        .as_ref()
                        .map(|n| (n.enabled, n.servers.join(" ")))
                ),
                ask,
            )?;
            let mut state = self.clock.lock().unwrap();
            if let Some(zone) = &change.timezone {
                state.timezone.clone_from(zone);
            }
            if let Some(ntp) = &change.ntp {
                state.ntp = ntp.enabled;
                state.servers.clone_from(&ntp.servers);
            }
            Ok((vec!["applied".into()], self.kept.lock().unwrap().clone()))
        }

        fn clock_update_now(&self, host: &str, ask: AskPassword) -> Result<Vec<String>> {
            self.enter(format!("clock-update {host}"), ask)?;
            Ok(vec!["Server: 10.0.0.2 (updated)".into()])
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
