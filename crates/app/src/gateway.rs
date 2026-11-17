//! everything the windows do to a gateway, behind one trait so tests can
//! swap in a fake that never touches the network.
//!
//! every action is an [`Op`] answered by a [`Reply`]: a backend implements [`Gateway::call`],
//! and the typed methods the windows use are a thin layer over it. on the desktop the op runs
//! over ssh (client mode) or on the gateway itself (server mode); in the browser it goes to
//! the web ui's `/api/op`.

use anyhow::{anyhow, Result};
use lanlord_model::clock::{Change as ClockChange, ClockState, Stored, ZoneNow};
use lanlord_model::leases::Lease;
use lanlord_model::network::Internal;
use lanlord_model::ops::{Op, Reply, SecretUpdate, UserChange};
use lanlord_model::power::{Outcome, Power};
use lanlord_model::secret::Secret;
use lanlord_model::server::ServerFacts;
use lanlord_model::settings::{Document, Settings};
use lanlord_model::users::Change;

use crate::ask::{never, AskPassword};

/// an administrator to create; the password is wiped when this is dropped.
pub struct NewAdmin {
    pub name: String,
    pub password: zeroize::Zeroizing<String>,
    pub ssh_key: Option<String>,
    pub passwordless_sudo: bool,
}

/// a reply that does not fit the op it answers.
fn unexpected(op: &str, reply: Reply) -> anyhow::Error {
    anyhow!("unexpected answer to {op}: {reply:?}")
}

pub trait Gateway: Send + Sync {
    /// run `op` on `host`, asking for the sudo password through `ask` when the server needs it.
    fn call(&self, host: &str, op: Op, ask: AskPassword) -> Result<Reply>;

    /// true when lanlord runs on the gateway itself: there is one server and no host list.
    fn is_local(&self) -> bool {
        false
    }

    /// set the gateway up for lanlord.
    fn bootstrap(&self, host: &str, ask: AskPassword) -> Result<()> {
        done(self.call(host, Op::Bootstrap, ask)?, "bootstrap")
    }

    /// apply `modules` (all of them when empty) with the stored settings.
    fn apply(&self, host: &str, modules: &[String], ask: AskPassword) -> Result<()> {
        let op = Op::Apply {
            modules: modules.to_vec(),
        };
        done(self.call(host, op, ask)?, "apply")
    }

    /// the plain settings text and which secrets are set; empty when nothing is stored yet.
    fn load_document(&self, host: &str, ask: AskPassword) -> Result<Document> {
        match self.call(host, Op::LoadDocument, ask)? {
            Reply::Document(document) => Ok(document),
            other => Err(unexpected("load_document", other)),
        }
    }

    /// store exactly `plain` as the plain settings and apply the secret
    /// `updates` (path, new value); blank values change nothing.
    fn save_document(
        &self,
        host: &str,
        plain: &str,
        updates: &[(String, String)],
        ask: AskPassword,
    ) -> Result<()> {
        let op = Op::SaveDocument {
            plain: plain.to_string(),
            updates: updates
                .iter()
                .map(|(path, value)| SecretUpdate {
                    path: path.clone(),
                    value: Secret::new(value.as_str()),
                })
                .collect(),
        };
        done(self.call(host, op, ask)?, "save_document")
    }

    /// the internal interface and address, read without sudo.
    fn probe(&self, host: &str) -> Result<Internal> {
        match self.call(host, Op::Probe, &mut never)? {
            Reply::Internal(internal) => Ok(internal),
            other => Err(unexpected("probe", other)),
        }
    }

    /// change logins (names and flags in the plain settings, passwords in the
    /// secrets) and apply the nt4-domain module.
    fn change_users(&self, host: &str, changes: &[Change], ask: AskPassword) -> Result<()> {
        let op = Op::ChangeUsers {
            changes: changes.iter().map(UserChange::from).collect(),
        };
        done(self.call(host, op, ask)?, "change_users")
    }

    /// set the account of machine `name` back to its first password, so it can join again.
    fn reset_machine(&self, host: &str, name: &str, ask: AskPassword) -> Result<()> {
        let op = Op::ResetMachine {
            name: name.to_string(),
        };
        done(self.call(host, op, ask)?, "reset_machine")
    }

    /// create an administrator login, in admins and sudo, with an optional ssh key.
    fn create_admin(&self, host: &str, new: &NewAdmin, ask: AskPassword) -> Result<()> {
        let op = Op::CreateAdmin {
            name: new.name.clone(),
            password: Secret::new(new.password.as_str()),
            ssh_key: new.ssh_key.clone(),
            passwordless_sudo: new.passwordless_sudo,
        };
        done(self.call(host, op, ask)?, "create_admin")
    }

    /// the dhcp leases the gateway has handed out.
    fn leases(&self, host: &str, ask: AskPassword) -> Result<Vec<Lease>> {
        match self.call(host, Op::Leases, ask)? {
            Reply::Leases(leases) => Ok(leases),
            other => Err(unexpected("leases", other)),
        }
    }

    /// the names of the logon scripts in the netlogon share.
    fn logon_scripts(&self, host: &str, ask: AskPassword) -> Result<Vec<String>> {
        names(self.call(host, Op::LogonScripts, ask)?, "logon_scripts")
    }

    /// one logon script with unix line endings; `None` when it does not exist yet.
    fn read_logon(&self, host: &str, name: &str, ask: AskPassword) -> Result<Option<String>> {
        let op = Op::ReadLogon {
            name: name.to_string(),
        };
        text(self.call(host, op, ask)?, "read_logon")
    }

    /// the generated `lanlord.bat`, for reference.
    fn read_generated_logon(&self, host: &str, ask: AskPassword) -> Result<Option<String>> {
        text(
            self.call(host, Op::ReadGeneratedLogon, ask)?,
            "read_generated_logon",
        )
    }

    /// store a logon script (saved with dos line endings).
    fn write_logon(&self, host: &str, name: &str, text: &str, ask: AskPassword) -> Result<()> {
        let op = Op::WriteLogon {
            name: name.to_string(),
            text: text.to_string(),
        };
        done(self.call(host, op, ask)?, "write_logon")
    }

    /// run a command on the gateway, its output going to the console; returns the exit code.
    fn run(&self, host: &str, command: &[String]) -> Result<i32> {
        let op = Op::Run {
            command: command.to_vec(),
        };
        match self.call(host, op, &mut never)? {
            Reply::ExitCode(code) => Ok(code),
            other => Err(unexpected("run", other)),
        }
    }

    /// reboot or power off the gateway.
    fn power(&self, host: &str, power: Power, ask: AskPassword) -> Result<Outcome> {
        match self.call(host, Op::Power { power }, ask)? {
            Reply::Outcome(outcome) => Ok(outcome),
            other => Err(unexpected("power", other)),
        }
    }

    /// what the server is: its system, uptime and interfaces (read-only, no sudo).
    fn server_facts(&self, host: &str, ask: AskPassword) -> Result<ServerFacts> {
        match self.call(host, Op::ServerFacts, ask)? {
            Reply::Facts(facts) => Ok(facts),
            other => Err(unexpected("server_facts", other)),
        }
    }

    /// everything a purge would remove, as lines; nothing is changed.
    fn purge_preview(
        &self,
        host: &str,
        remove_admins: bool,
        ask: AskPassword,
    ) -> Result<Vec<String>> {
        let op = Op::PurgePreview { remove_admins };
        lines(self.call(host, op, ask)?, "purge_preview")
    }

    /// remove lanlord from the server; the lines say what was removed.
    fn purge(&self, host: &str, remove_admins: bool, ask: AskPassword) -> Result<Vec<String>> {
        lines(self.call(host, Op::Purge { remove_admins }, ask)?, "purge")
    }

    /// the server's time, time zone and internet time; asks for nothing.
    fn clock_state(&self, host: &str) -> Result<ClockState> {
        match self.call(host, Op::ClockState, &mut never)? {
            Reply::Clock(state) => Ok(state),
            other => Err(unexpected("clock_state", other)),
        }
    }

    /// the time zones the server knows.
    fn clock_zones(&self, host: &str) -> Result<Vec<String>> {
        names(self.call(host, Op::ClockZones, &mut never)?, "clock_zones")
    }

    /// what a time zone looks like on the server right now.
    fn clock_zone_now(&self, host: &str, zone: &str) -> Result<ZoneNow> {
        let op = Op::ClockZoneNow {
            zone: zone.to_string(),
        };
        match self.call(host, op, &mut never)? {
            Reply::ZoneNow(now) => Ok(now),
            other => Err(unexpected("clock_zone_now", other)),
        }
    }

    /// change the time, the time zone or internet time; what the server said, and whether the
    /// zone and servers were also kept in the settings.
    fn clock_apply(
        &self,
        host: &str,
        change: &ClockChange,
        ask: AskPassword,
    ) -> Result<(Vec<String>, Stored)> {
        let op = Op::ClockApply {
            change: change.clone(),
        };
        match self.call(host, op, ask)? {
            Reply::ClockApplied { lines, stored } => Ok((lines, stored)),
            other => Err(unexpected("clock_apply", other)),
        }
    }

    /// ask the time service to synchronize now; what it said.
    fn clock_update_now(&self, host: &str, ask: AskPassword) -> Result<Vec<String>> {
        lines(
            self.call(host, Op::ClockUpdateNow, ask)?,
            "clock_update_now",
        )
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

fn done(reply: Reply, op: &str) -> Result<()> {
    match reply {
        Reply::Done => Ok(()),
        other => Err(unexpected(op, other)),
    }
}

fn names(reply: Reply, op: &str) -> Result<Vec<String>> {
    match reply {
        Reply::Names(names) => Ok(names),
        other => Err(unexpected(op, other)),
    }
}

fn lines(reply: Reply, op: &str) -> Result<Vec<String>> {
    match reply {
        Reply::Lines(lines) => Ok(lines),
        other => Err(unexpected(op, other)),
    }
}

fn text(reply: Reply, op: &str) -> Result<Option<String>> {
    match reply {
        Reply::Text(text) => Ok(text),
        other => Err(unexpected(op, other)),
    }
}

#[cfg(feature = "native")]
pub use native::{Ssh, NO_KEY};

#[cfg(feature = "native")]
mod native {
    use anyhow::{anyhow, Result};
    use lanlord_core::ops::execute;
    use lanlord_model::ops::{Op, Reply};
    use lanlord_model::settings::Document;
    use lanlord_model::{leases, server};

    use super::Gateway;
    use crate::ask::AskPassword;
    use crate::demo_clock;

    pub use lanlord_core::ops::NO_KEY;

    /// the gateway reached over ssh: lanlord on a mac or pc (client mode).
    pub struct Ssh;

    impl Gateway for Ssh {
        fn call(&self, host: &str, op: Op, ask: AskPassword) -> Result<Reply> {
            if let Some(reply) = demo_reply(&op) {
                return reply;
            }
            execute(host, &op, ask)
        }
    }

    /// lanlord running on the gateway itself (server mode): one server, the local one. nothing
    /// uses it yet: core runs every op through an ssh session, and server mode needs a local
    /// path and a privilege model (a root helper, or a socket daemon only the admins group may
    /// use) first.
    #[allow(dead_code)] // server mode is not built yet
    pub(super) struct Local;

    impl Gateway for Local {
        fn call(&self, _host: &str, _op: Op, _ask: AskPassword) -> Result<Reply> {
            Err(anyhow!("server mode backend not built yet"))
        }

        fn is_local(&self) -> bool {
            true
        }
    }

    /// `LANLORD_FAKE_SERVER=1` answers the server windows with made-up facts, so they can be
    /// tried without a server; a purge never runs in it. `LANLORD_FAKE_CLOCK` does the same for
    /// the clock.
    fn demo_reply(op: &Op) -> Option<Result<Reply>> {
        if let Some(reply) = demo_clock::mode().and_then(|container| clock_demo(op, container)) {
            return Some(reply);
        }
        if std::env::var("LANLORD_FAKE_SERVER").as_deref() != Ok("1") {
            return None;
        }
        server_demo(op)
    }

    /// the made-up answer of the demo clock, for the clock ops.
    fn clock_demo(op: &Op, container: bool) -> Option<Result<Reply>> {
        Some(Ok(match op {
            Op::ClockState => Reply::Clock(demo_clock::state(container)),
            Op::ClockZones => Reply::Names(demo_clock::zones()),
            Op::ClockZoneNow { zone } => Reply::ZoneNow(demo_clock::zone_now(zone)),
            Op::ClockApply { change } => {
                let (lines, stored) = demo_clock::apply(change, container);
                Reply::ClockApplied { lines, stored }
            }
            Op::ClockUpdateNow => Reply::Lines(vec!["Server: 162.159.200.1 (pool.ntp.org)".into()]),
            _ => return None,
        }))
    }

    /// the made-up answer of the demo server, for the ops that have one.
    fn server_demo(op: &Op) -> Option<Result<Reply>> {
        Some(match op {
            Op::LoadDocument => Ok(Reply::Document(Document {
                plain: DEMO_SETTINGS.to_string(),
                ..Document::default()
            })),
            Op::Leases => Ok(Reply::Leases(leases::parse(DEMO_LEASES))),
            Op::ServerFacts => Ok(Reply::Facts(server::demo())),
            Op::PurgePreview { .. } => Ok(Reply::Lines(demo_purge())),
            Op::Purge { .. } => Err(anyhow!("demo mode: nothing is removed")),
            _ => return None,
        })
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
    mod tests {
        use super::*;

        #[test]
        fn demo_mode_never_purges() {
            let purge = server_demo(&Op::Purge {
                remove_admins: false,
            });
            let preview = server_demo(&Op::PurgePreview {
                remove_admins: false,
            });
            let apply = server_demo(&Op::Apply { modules: vec![] });
            assert!(purge.unwrap().is_err());
            assert!(matches!(preview, Some(Ok(Reply::Lines(_)))));
            assert!(apply.is_none(), "ops without demo data go to the server");
        }

        #[test]
        fn the_demo_server_answers_its_windows() {
            let load = server_demo(&Op::LoadDocument).unwrap().unwrap();
            assert!(matches!(load, Reply::Document(d) if d.plain.contains("retronet")));
            let leases = server_demo(&Op::Leases).unwrap().unwrap();
            assert!(matches!(leases, Reply::Leases(l) if l.len() == 3));
            let facts = server_demo(&Op::ServerFacts).unwrap().unwrap();
            assert!(matches!(facts, Reply::Facts(_)));
        }

        #[test]
        fn the_demo_clock_answers_every_clock_op() {
            use lanlord_model::clock::Change;
            for container in [false, true] {
                let state = clock_demo(&Op::ClockState, container).unwrap().unwrap();
                assert!(matches!(state, Reply::Clock(_)));
                let zones = clock_demo(&Op::ClockZones, container).unwrap().unwrap();
                assert!(matches!(zones, Reply::Names(z) if !z.is_empty()));
                let now = Op::ClockZoneNow {
                    zone: "Europe/Berlin".into(),
                };
                assert!(matches!(
                    clock_demo(&now, container),
                    Some(Ok(Reply::ZoneNow(_)))
                ));
                let apply = Op::ClockApply {
                    change: Change::default(),
                };
                assert!(matches!(
                    clock_demo(&apply, container),
                    Some(Ok(Reply::ClockApplied { .. }))
                ));
                assert!(matches!(
                    clock_demo(&Op::ClockUpdateNow, container),
                    Some(Ok(Reply::Lines(_)))
                ));
                assert!(clock_demo(&Op::Leases, container).is_none());
            }
        }

        #[test]
        fn the_local_gateway_is_the_one_server_and_not_built_yet() {
            assert!(Local.is_local());
            assert!(!Ssh.is_local());
            let err = Local
                .call("ignored", Op::Leases, &mut crate::ask::never)
                .unwrap_err();
            assert_eq!(err.to_string(), "server mode backend not built yet");
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub mod http;
pub mod round_trip;

#[cfg(any(test, feature = "fake"))]
pub use fake::Fake;

#[cfg(any(test, feature = "fake"))]
mod fake {
    use std::sync::Mutex;

    use lanlord_model::{clock, logon, server, users};

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
        /// the server's clock, changed by `clock_apply`.
        pub clock: Mutex<ClockState>,
        /// what `clock_apply` says about keeping the change in the settings.
        pub kept: Mutex<Stored>,
        /// the time zones it lists.
        pub zones: Mutex<Vec<String>>,
        /// the small reads that are not tasks (the taskbar clock).
        pub reads: Mutex<Vec<String>>,
    }

    impl Default for Fake {
        fn default() -> Self {
            Self::new()
        }
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
        fn call(&self, host: &str, op: Op, ask: AskPassword) -> Result<Reply> {
            Ok(match op {
                Op::Doctor => Reply::Doctor(lanlord_model::doctor::Report::parse(
                    "virt_container=none\nvirt_vm=none\nlink_bridge=ok\n",
                )),
                Op::Bootstrap => {
                    self.bootstrap(host, ask)?;
                    Reply::Done
                }
                Op::Apply { modules } => {
                    self.apply(host, &modules, ask)?;
                    Reply::Done
                }
                Op::LoadDocument => Reply::Document(self.load_document(host, ask)?),
                Op::SaveDocument { plain, updates } => {
                    let updates: Vec<(String, String)> = updates
                        .iter()
                        .map(|u| (u.path.clone(), u.value.expose().to_string()))
                        .collect();
                    self.save_document(host, &plain, &updates, ask)?;
                    Reply::Done
                }
                Op::Probe => Reply::Internal(self.probe(host)?),
                Op::ChangeUsers { changes } => {
                    let changes: Vec<Change> = changes.iter().map(Change::from).collect();
                    self.change_users(host, &changes, ask)?;
                    Reply::Done
                }
                Op::ResetMachine { name } => {
                    self.reset_machine(host, &name, ask)?;
                    Reply::Done
                }
                Op::CreateAdmin {
                    name,
                    password,
                    ssh_key,
                    passwordless_sudo,
                } => {
                    let new = NewAdmin {
                        name,
                        password: zeroize::Zeroizing::new(password.expose().to_string()),
                        ssh_key,
                        passwordless_sudo,
                    };
                    self.create_admin(host, &new, ask)?;
                    Reply::Done
                }
                Op::Leases => Reply::Leases(self.leases(host, ask)?),
                Op::LogonScripts => Reply::Names(self.logon_scripts(host, ask)?),
                Op::ReadLogon { name } => Reply::Text(self.read_logon(host, &name, ask)?),
                Op::ReadGeneratedLogon => Reply::Text(self.read_generated_logon(host, ask)?),
                Op::WriteLogon { name, text } => {
                    self.write_logon(host, &name, &text, ask)?;
                    Reply::Done
                }
                Op::Run { command } => Reply::ExitCode(self.run(host, &command)?),
                Op::Power { power } => Reply::Outcome(self.power(host, power, ask)?),
                Op::ServerFacts => Reply::Facts(self.server_facts(host, ask)?),
                Op::PurgePreview { remove_admins } => {
                    Reply::Lines(self.purge_preview(host, remove_admins, ask)?)
                }
                Op::Purge { remove_admins } => {
                    Reply::Lines(self.purge(host, remove_admins, ask)?)
                }
                Op::ClockState => Reply::Clock(self.clock_state(host)?),
                Op::ClockZones => Reply::Names(self.clock_zones(host)?),
                Op::ClockZoneNow { zone } => Reply::ZoneNow(self.clock_zone_now(host, &zone)?),
                Op::ClockApply { change } => {
                    let (lines, stored) = self.clock_apply(host, &change, ask)?;
                    Reply::ClockApplied { lines, stored }
                }
                Op::ClockUpdateNow => Reply::Lines(self.clock_update_now(host, ask)?),
            })
        }

        fn is_local(&self) -> bool {
            *self.local.lock().unwrap()
        }
    }

    // what the fake does for each op; the gateway's typed methods reach these through `call`
    impl Fake {
        pub fn bootstrap(&self, host: &str, ask: AskPassword) -> Result<()> {
            self.enter(format!("bootstrap {host}"), ask)
        }

        pub fn apply(&self, host: &str, modules: &[String], ask: AskPassword) -> Result<()> {
            self.enter(format!("apply {host} [{}]", modules.join(",")), ask)
        }

        pub fn load_document(&self, host: &str, ask: AskPassword) -> Result<Document> {
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

        pub fn save_document(
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

        pub fn probe(&self, host: &str) -> Result<Internal> {
            self.calls.lock().unwrap().push(format!("probe {host}"));
            self.internal
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| anyhow!("no network"))
        }

        pub fn change_users(&self, host: &str, changes: &[Change], ask: AskPassword) -> Result<()> {
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

        pub fn reset_machine(&self, host: &str, name: &str, ask: AskPassword) -> Result<()> {
            self.enter(format!("reset {host} {name}"), ask)
        }

        pub fn create_admin(&self, host: &str, new: &NewAdmin, ask: AskPassword) -> Result<()> {
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

        pub fn leases(&self, host: &str, ask: AskPassword) -> Result<Vec<Lease>> {
            self.enter(format!("leases {host}"), ask)?;
            Ok(self.leases.lock().unwrap().clone())
        }

        pub fn logon_scripts(&self, host: &str, ask: AskPassword) -> Result<Vec<String>> {
            self.enter(format!("logon-list {host}"), ask)?;
            Ok(self.logon.lock().unwrap().keys().cloned().collect())
        }

        pub fn read_logon(
            &self,
            host: &str,
            name: &str,
            ask: AskPassword,
        ) -> Result<Option<String>> {
            self.enter(format!("logon-read {host} {name}"), ask)?;
            Ok(self.logon.lock().unwrap().get(name).cloned())
        }

        pub fn read_generated_logon(&self, host: &str, ask: AskPassword) -> Result<Option<String>> {
            self.enter(format!("logon-generated {host}"), ask)?;
            Ok(self.generated.lock().unwrap().clone())
        }

        pub fn write_logon(
            &self,
            host: &str,
            name: &str,
            text: &str,
            ask: AskPassword,
        ) -> Result<()> {
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

        pub fn power(&self, host: &str, action: Power, ask: AskPassword) -> Result<Outcome> {
            self.enter(format!("power {host} {}", action.doing()), ask)?;
            Ok(*self.outcome.lock().unwrap())
        }

        pub fn server_facts(&self, host: &str, ask: AskPassword) -> Result<ServerFacts> {
            self.enter(format!("facts {host}"), ask)?;
            let facts = self.facts.lock().unwrap().clone();
            Ok(facts.unwrap_or_else(server::demo))
        }

        pub fn purge_preview(
            &self,
            host: &str,
            remove_admins: bool,
            ask: AskPassword,
        ) -> Result<Vec<String>> {
            self.enter(format!("purge-preview {host} admins={remove_admins}"), ask)?;
            Ok(self.purge_lines.lock().unwrap().clone())
        }

        pub fn purge(
            &self,
            host: &str,
            remove_admins: bool,
            ask: AskPassword,
        ) -> Result<Vec<String>> {
            self.enter(format!("purge {host} admins={remove_admins}"), ask)?;
            Ok(self.purge_lines.lock().unwrap().clone())
        }

        pub fn clock_state(&self, host: &str) -> Result<ClockState> {
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

        pub fn clock_zones(&self, host: &str) -> Result<Vec<String>> {
            self.reads
                .lock()
                .unwrap()
                .push(format!("clock-zones {host}"));
            Ok(self.zones.lock().unwrap().clone())
        }

        pub fn clock_zone_now(&self, host: &str, zone: &str) -> Result<ZoneNow> {
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

        pub fn clock_apply(
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

        pub fn clock_update_now(&self, host: &str, ask: AskPassword) -> Result<Vec<String>> {
            self.enter(format!("clock-update {host}"), ask)?;
            Ok(vec!["Server: 10.0.0.2 (updated)".into()])
        }

        pub fn run(&self, host: &str, command: &[String]) -> Result<i32> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("run {host} {}", command.join(" ")));
            if let Some(message) = self.fail.lock().unwrap().clone() {
                return Err(anyhow!(message));
            }
            crate::log::emit(
                crate::log::Kind::Out,
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
    fn every_typed_method_reaches_the_fake_through_an_op() {
        let fake = Fake::new();
        *fake.internal.lock().unwrap() = Some(Internal {
            interface: "eth1".into(),
            address: std::net::Ipv4Addr::new(192, 0, 2, 1),
            prefix: 24,
        });
        fake.logon
            .lock()
            .unwrap()
            .insert("start.bat".into(), "echo hi\n".into());
        let gateway: &dyn Gateway = &fake;
        let mut ask = |_: &str| Ok(String::new());
        let h = "a@b";
        gateway.bootstrap(h, &mut ask).unwrap();
        gateway.apply(h, &["dns".into()], &mut ask).unwrap();
        gateway
            .save_document(h, "x: 1\n", &[("p.password".into(), "pw".into())], &mut ask)
            .unwrap();
        let document = gateway.load_document(h, &mut ask).unwrap();
        assert_eq!(document.plain, "x: 1\n");
        assert_eq!(document.secrets_set, ["p.password"]);
        assert!(gateway.probe(h).is_ok());
        gateway.reset_machine(h, "ws1", &mut ask).unwrap();
        let admin = NewAdmin {
            name: "admin".into(),
            password: zeroize::Zeroizing::new("pw".into()),
            ssh_key: Some("ssh-ed25519 AAAA demo".into()),
            passwordless_sudo: true,
        };
        gateway.create_admin(h, &admin, &mut ask).unwrap();
        assert!(gateway.leases(h, &mut ask).unwrap().is_empty());
        assert_eq!(gateway.logon_scripts(h, &mut ask).unwrap(), ["start.bat"]);
        assert!(gateway
            .read_logon(h, "start.bat", &mut ask)
            .unwrap()
            .is_some());
        assert!(gateway.read_generated_logon(h, &mut ask).unwrap().is_none());
        gateway
            .write_logon(h, "other.bat", "echo\r\n", &mut ask)
            .unwrap();
        assert_eq!(
            gateway.power(h, Power::Reboot, &mut ask).unwrap(),
            Outcome::Dropped
        );
        assert!(gateway.server_facts(h, &mut ask).is_ok());
        assert_eq!(gateway.purge_preview(h, true, &mut ask).unwrap().len(), 1);
        assert_eq!(gateway.purge(h, false, &mut ask).unwrap().len(), 1);
        assert!(!gateway.clock_zones(h).unwrap().is_empty());
        assert_eq!(
            gateway.clock_zone_now(h, "Europe/Berlin").unwrap().abbrev,
            "CET"
        );
        let change = ClockChange {
            timezone: Some("Europe/Berlin".into()),
            ..ClockChange::default()
        };
        let (lines, stored) = gateway.clock_apply(h, &change, &mut ask).unwrap();
        assert_eq!((lines.len(), stored), (1, Stored::Yes));
        assert_eq!(gateway.clock_state(h).unwrap().timezone, "Europe/Berlin");
        assert_eq!(gateway.clock_update_now(h, &mut ask).unwrap().len(), 1);
        assert_eq!(fake.calls().len(), 18, "{:?}", fake.calls());
    }

    /// a backend that answers every op with the wrong kind of reply.
    #[cfg(test)]
    struct Confused;

    #[cfg(test)]
    impl Gateway for Confused {
        fn call(&self, _host: &str, op: Op, _ask: AskPassword) -> Result<Reply> {
            Ok(match op {
                Op::Bootstrap => Reply::ExitCode(1),
                _ => Reply::Done,
            })
        }
    }

    #[test]
    fn a_reply_that_does_not_fit_the_op_is_an_error() {
        let gateway: &dyn Gateway = &Confused;
        let mut ask = |_: &str| Ok(String::new());
        let h = "a@b";
        let err = gateway.bootstrap(h, &mut ask).unwrap_err().to_string();
        assert!(err.contains("unexpected answer to bootstrap"), "{err}");
        assert!(gateway.load_document(h, &mut ask).is_err());
        assert!(gateway.probe(h).is_err());
        assert!(gateway.leases(h, &mut ask).is_err());
        assert!(gateway.logon_scripts(h, &mut ask).is_err());
        assert!(gateway.read_logon(h, "x", &mut ask).is_err());
        assert!(gateway.run(h, &[]).is_err());
        assert!(gateway.power(h, Power::Reboot, &mut ask).is_err());
        assert!(gateway.server_facts(h, &mut ask).is_err());
        assert!(gateway.purge_preview(h, false, &mut ask).is_err());
        assert!(gateway.clock_state(h).is_err());
        assert!(gateway.clock_zone_now(h, "UTC").is_err());
        assert!(gateway
            .clock_apply(h, &ClockChange::default(), &mut ask)
            .is_err());
        assert!(!gateway.is_local());
    }

    #[test]
    fn a_read_that_asks_for_a_password_fails() {
        let fake = Fake::new();
        *fake.password_prompt.lock().unwrap() = Some("[sudo] password:".into());
        let gateway: &dyn Gateway = &fake;
        *fake.internal.lock().unwrap() = Some(Internal {
            interface: "eth1".into(),
            address: std::net::Ipv4Addr::new(192, 0, 2, 1),
            prefix: 24,
        });
        // a read never prompts, so with a network it succeeds without an answer
        assert!(gateway.probe("a@b").is_ok());
        assert!(fake.answer.lock().unwrap().is_none());
        // an op that reaches the prompt fails when nobody can answer
        let mut refuse = |_: &str| Err(anyhow!("no password here"));
        let failed = gateway.reset_machine("a@b", "ws1", &mut refuse);
        assert!(failed.is_err_and(|e| e.to_string().contains("no password here")));
        let mut ask = |_: &str| Ok("pw".to_string());
        gateway.reset_machine("a@b", "ws1", &mut ask).unwrap();
        assert_eq!(fake.answer.lock().unwrap().as_deref(), Some("pw"));
    }

    #[test]
    fn typed_calls_travel_as_ops() {
        let fake = Fake::new();
        let gateway: &dyn Gateway = &fake;
        let mut ask = |_: &str| Ok(String::new());
        let changes = [Change::SetPassword {
            name: "alice".into(),
            password: "pw".into(),
        }];
        *fake.stored.lock().unwrap() = "nt4-domain:\n  users: []\n".into();
        gateway.change_users("a@b", &changes, &mut ask).unwrap();
        assert_eq!(gateway.run("a@b", &["true".into()]).unwrap(), 0);
        assert!(gateway.clock_state("a@b").is_ok());
        assert_eq!(
            fake.calls(),
            ["users a@b 1", "apply a@b [nt4-domain]", "run a@b true"]
        );
    }
}
