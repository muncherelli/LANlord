//! every action a front end asks of a server, as data.
//!
//! the gui, the tui, the cli and the web ui all describe what they want as an [`Operation`], and
//! lanlord answers with a [`Reply`]. on a client the operation runs over ssh; on the gateway
//! (server mode, and the web ui) it runs locally. an operation names no server: whoever runs it
//! knows which one.
//!
//! sudo passwords are not part of an operation. how a front end that cannot answer a prompt gets
//! privileges (a root helper, or a socket daemon only the lanlord-admins group may use) is not
//! decided yet; until then lanlord asks on its terminal while it runs an operation.

use serde::{Deserialize, Serialize};

use crate::access::{Access, Area, Need, Profile};
use crate::clock::{Change as ClockChange, ClockState, Stored, ZoneNow};
use crate::doctor::Report;
use crate::identity::{AccountInfo, PasswordReport};
use crate::internet::Change as InternetChange;
use crate::lan_edit::Edit as NetworkEdit;
use crate::leases::Lease;
use crate::login_text::LoginText;
use crate::netstatus::InterfaceStatus;
use crate::network::Internal;
use crate::power::{Outcome, Power};
use crate::rootssh::{RootSsh, RootSshInfo};
use crate::secret::Secret;
use crate::server::ServerFacts;
use crate::settings::Document;
use crate::ssh_access::Change as SshChange;
use crate::sshauth::SshAuthInfo;
use crate::users::Change as UserChangeInMemory;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Operation {
    /// set the gateway up for lanlord.
    Bootstrap,
    /// apply these modules (all of them when empty) with the stored settings.
    Apply { modules: Vec<String> },
    /// what applying these modules would change, without changing anything: the engine's check
    /// and diff. a module that runs on Ansible says so instead of a preview.
    PlanApply { modules: Vec<String> },
    /// the plain settings text and which secrets are set.
    LoadDocument,
    /// store exactly `plain` as the plain settings and set these secrets; blank values change
    /// nothing.
    SaveDocument {
        plain: String,
        updates: Vec<SecretUpdate>,
    },
    /// seal the stored secrets to the gateway's own key, once, when an older client's key sealed
    /// them. `key` is that client's old key file; it is used for this one request and dropped,
    /// and the client stops using it afterwards.
    MoveSecrets { key: Secret },
    /// bring the stored settings files up to the format this lanlord writes (an upgrade does
    /// this once). each file that changes is first copied beside itself under `stamp`, the
    /// gateway's time as `20260101T120000Z`. the answer says what was done.
    MigrateStored { stamp: String },
    /// the internal interface and address, read without sudo.
    Probe,
    /// change logins and apply the nt4-domain module.
    ChangeUsers { changes: Vec<UserChange> },
    /// set the account of machine `name` back to its first password, so it can join again.
    ResetMachine { name: String },
    /// create an administrator login, in admins and sudo, with an optional ssh key.
    CreateAdmin {
        name: String,
        password: Secret,
        ssh_key: Option<String>,
        passwordless_sudo: bool,
    },
    /// switch internet access on or off for a network, or allow or deny one computer, and apply
    /// the docker and dns modules.
    ChangeInternet { change: InternetChange },
    /// switch ssh to the server on or off for a network, or allow or deny one computer, and apply
    /// the docker and dns modules (the firewall is in the docker module).
    ChangeSshAccess { change: SshChange },
    /// change the lanlord networks (add, change or remove one, or a static lease) and apply the
    /// docker and dns modules.
    ChangeNetworks { edit: Box<NetworkEdit> },
    /// first-run setup, for root's web session while no other administrator exists: create the
    /// administrator `name` (in admins and sudo) and set root's ssh
    /// access, which must be chosen. refused once an administrator other than root exists.
    SetupFirstAdmin {
        name: String,
        password: Secret,
        root_ssh: RootSsh,
        /// the new account's first authorized ssh key, if one was given.
        #[serde(default)]
        ssh_key: Option<String>,
    },
    /// how root may sign in over ssh: the level lanlord set, if any.
    RootSshState,
    /// set how root may sign in over ssh.
    SetRootSsh { access: RootSsh },
    /// whether sshd takes passwords, and the administrator accounts' ssh keys.
    SshAuthState,
    /// turn ssh password sign-in on or off. turning it off is refused while an administrator
    /// would be left with no key, unless `confirmed`.
    SetPasswordLogin {
        passwords: bool,
        #[serde(default)]
        confirmed: bool,
    },
    /// add a public key to the authorized keys of an administrator account.
    AddAdminKey { user: String, key: String },
    /// remove a public key from the authorized keys of an administrator. `key` is the key id
    /// (type and base64 field, see `sshauth::key_id`), whatever options the line has. removing
    /// the last key while passwords are off is refused unless `confirmed`.
    RemoveAdminKey {
        user: String,
        key: String,
        #[serde(default)]
        confirmed: bool,
    },
    /// the dhcp leases the gateway has handed out.
    Leases,
    /// the names of the logon scripts in the netlogon share.
    LogonScripts,
    /// one logon script with unix line endings.
    ReadLogon { name: String },
    /// the generated `lanlord.bat`, for reference.
    ReadGeneratedLogon,
    /// store a logon script (saved with dos line endings).
    WriteLogon { name: String, text: String },
    /// run a command on the gateway; the reply is its exit code.
    Run { command: Vec<String> },
    /// reboot or power off the gateway.
    Power { power: Power },
    /// what the server is: its system, uptime and interfaces.
    ServerFacts,
    /// the status of each network connection, like Windows' connection Status box.
    NetworkStatus,
    /// everything a purge would remove; nothing is changed.
    PurgePreview { remove_admins: bool },
    /// remove lanlord from the server.
    Purge { remove_admins: bool },
    /// the server's time, time zone and internet time.
    ClockState,
    /// the time zones the server knows.
    ClockZones,
    /// what a time zone looks like on the server right now.
    ClockZoneNow { zone: String },
    /// change the time, the time zone or internet time.
    ClockApply { change: ClockChange },
    /// ask the time service to synchronize now.
    ClockUpdateNow,
    /// what the server can do for lanlord's networks, and how to fix what it cannot.
    Doctor,
    /// the caller's own account: profile, roles, what they may do and the linked accounts.
    AccountInfo,
    /// change the caller's own password on every linked account, all or none. the old
    /// password is checked first.
    ChangePassword { old: Secret, new: Secret },
    /// the caller's own authorized ssh keys, one public key line each.
    MyKeys,
    /// add a public key to the caller's own authorized keys.
    AddMyKey { key: String },
    /// remove a public key (the whole line) from the caller's own authorized keys.
    RemoveMyKey { key: String },
    /// change the caller's own profile fields.
    UpdateProfile { profile: Profile },
    /// set the password of `user` on every linked account, all or none.
    ResetPassword { user: String, new: Secret },
    /// the roles and who has them.
    LoadAccess,
    /// store the roles and who has them.
    SaveAccess { access: Access },
    /// keep sending the events of `topic` (see [`topic`]). the web ui serves it as a stream on
    /// its own route, not as a reply; the other backends refuse it.
    Subscribe { topic: String },
    /// the machine names of the domain; they are low sensitivity, so a view of the domain is enough.
    Machines,
    /// run a command on a domain machine; the reply is its output and exit code. `timeout_secs`
    /// is 1 to 900 and 120 when left out. a plain `exit N` in the command is refused: it ends the
    /// outer cmd.exe on nt4 (no `exit /b`), so the run would show as timed out; use `cmd /c exit N`.
    MachineRun {
        machine: String,
        command: String,
        #[serde(default = "default_timeout_secs")]
        timeout_secs: u32,
    },
    /// run a preset from [`crate::nt4_exec`] on a domain machine; `args` fill its placeholders.
    MachinePreset {
        machine: String,
        preset: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

/// how long a machine command may run when the caller says nothing.
pub const DEFAULT_TIMEOUT_SECS: u32 = 120;

fn default_timeout_secs() -> u32 {
    DEFAULT_TIMEOUT_SECS
}

impl Operation {
    /// what this operation needs from its caller. the match has no catch-all on purpose: a new
    /// operation does not compile until it says what it needs.
    pub fn requires(&self) -> Need {
        match self {
            Operation::Bootstrap | Operation::MigrateStored { .. } => Need::manage(Area::Updates),
            Operation::Apply { .. } | Operation::PlanApply { .. } => Need::manage(Area::Services),
            Operation::LoadDocument => Need::view(Area::Services),
            Operation::SaveDocument { .. } | Operation::MoveSecrets { .. } => {
                Need::manage(Area::Services)
            }
            Operation::Probe => Need::view(Area::Network),
            Operation::ChangeUsers { .. } => Need::manage(Area::Domain),
            Operation::ResetMachine { .. } => Need::manage(Area::Domain),
            Operation::CreateAdmin { .. } => Need::manage(Area::Users),
            Operation::ChangeInternet { .. } => Need::manage(Area::Network),
            Operation::ChangeSshAccess { .. } => Need::manage(Area::Network),
            Operation::ChangeNetworks { .. } => Need::manage(Area::Network),
            // not given to any role: the server only runs it for root's setup session
            Operation::SetupFirstAdmin { .. } => Need::manage(Area::Users),
            Operation::RootSshState => Need::view(Area::Users),
            // changes what the whole server's sshd allows: administrators only
            Operation::SetRootSsh { .. }
            | Operation::SshAuthState
            | Operation::SetPasswordLogin { .. }
            | Operation::AddAdminKey { .. }
            | Operation::RemoveAdminKey { .. } => Need::Admin,
            Operation::Leases => Need::view(Area::Network),
            Operation::LogonScripts
            | Operation::ReadLogon { .. }
            | Operation::ReadGeneratedLogon => Need::view(Area::Shares),
            Operation::WriteLogon { .. } => Need::manage(Area::Shares),
            Operation::Run { .. } => Need::manage(Area::Commands),
            Operation::Machines => Need::view(Area::Domain),
            Operation::MachineRun { .. } | Operation::MachinePreset { .. } => {
                Need::manage(Area::Commands)
            }
            Operation::Power { .. } => Need::manage(Area::Status),
            Operation::ServerFacts | Operation::Doctor => Need::view(Area::Status),
            Operation::NetworkStatus => Need::view(Area::Network),
            Operation::PurgePreview { .. } => Need::view(Area::Updates),
            Operation::Purge { .. } => Need::manage(Area::Updates),
            Operation::ClockState | Operation::ClockZones | Operation::ClockZoneNow { .. } => {
                Need::view(Area::Clock)
            }
            Operation::ClockApply { .. } | Operation::ClockUpdateNow => Need::manage(Area::Clock),
            Operation::AccountInfo
            | Operation::ChangePassword { .. }
            | Operation::MyKeys
            | Operation::AddMyKey { .. }
            | Operation::RemoveMyKey { .. }
            | Operation::UpdateProfile { .. } => Need::SelfOnly,
            Operation::ResetPassword { .. } | Operation::SaveAccess { .. } => {
                Need::manage(Area::Users)
            }
            Operation::LoadAccess => Need::view(Area::Users),
            // apply progress carries the output of the tasks, so it needs what applying shows
            Operation::Subscribe { topic } if topic == topic::APPLY => Need::view(Area::Services),
            // command output can hold anything, so it needs what running the command needs
            Operation::Subscribe { topic } if topic::machine_run_id(topic).is_some() => {
                Need::manage(Area::Commands)
            }
            Operation::Subscribe { .. } => Need::view(Area::Status),
        }
    }
}

/// the topics a stream can be subscribed to.
pub mod topic {
    /// monitor samples without the process list.
    pub const MONITOR: &str = "monitor";
    /// monitor samples with the process list.
    pub const PROCS: &str = "monitor:procs";
    /// the events of an apply, as `lanlord_model::apply::ApplyEvent` json.
    pub const APPLY: &str = "apply";
    /// the start of a machine run's topic, `machine-run:<id>`.
    pub const MACHINE_RUN: &str = "machine-run:";

    /// the topic of the machine run with this id (8 hex digits).
    pub fn machine_run(id: &str) -> String {
        format!("{MACHINE_RUN}{id}")
    }

    /// the run id of a `machine-run:<id>` topic; `None` for any other topic or a bad id.
    pub fn machine_run_id(topic: &str) -> Option<&str> {
        let id = topic.strip_prefix(MACHINE_RUN)?;
        (id.len() == 8 && id.bytes().all(|b| b.is_ascii_hexdigit())).then_some(id)
    }
}

impl Operation {
    /// check the arguments before anything runs: the first wrong field is named. the server does
    /// this whatever the client did; a client may too, for early feedback.
    pub fn validate(&self) -> Result<(), crate::validate::Invalid> {
        crate::validate::operation(self)
    }

    /// does the operation touch the stored secrets? those run only where the gateway's key is: the
    /// root operations service, or `lanlord operate` under sudo. no client decrypts anything.
    pub fn needs_gateway_key(&self) -> bool {
        matches!(
            self,
            Operation::Apply { .. }
                | Operation::PlanApply { .. }
                | Operation::LoadDocument
                | Operation::SaveDocument { .. }
                | Operation::MoveSecrets { .. }
                | Operation::ChangeInternet { .. }
                | Operation::ChangeSshAccess { .. }
                | Operation::MigrateStored { .. }
                | Operation::ChangeNetworks { .. }
                | Operation::ChangeUsers { .. }
                | Operation::ClockApply { .. }
                | Operation::MachineRun { .. }
                | Operation::MachinePreset { .. }
        )
    }

    /// does the operation change the server, so that the bridge that runs it must be root?
    pub fn needs_root(&self) -> bool {
        matches!(
            self,
            Operation::ClockApply { .. } | Operation::ClockUpdateNow
        )
    }

    /// can `lanlord bridge` on the gateway run the operation itself? the others stay on the shell
    /// path.
    pub fn runs_on_bridge(&self) -> bool {
        matches!(
            self,
            Operation::ClockState
                | Operation::ClockZones
                | Operation::ClockZoneNow { .. }
                | Operation::ClockApply { .. }
                | Operation::ClockUpdateNow
        )
    }
}

/// a secret setting to change: its path in the settings and the new value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretUpdate {
    pub path: String,
    pub value: Secret,
}

/// a change to a login, as it travels; the password is a [`Secret`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum UserChange {
    SetPassword { name: String, password: Secret },
    SetAdmin { name: String, admin: bool },
    SetSoftwareAdmin { name: String, software_admin: bool },
    Remove { name: String },
}

impl From<&UserChange> for UserChangeInMemory {
    fn from(change: &UserChange) -> Self {
        match change {
            UserChange::SetPassword { name, password } => Self::SetPassword {
                name: name.clone(),
                password: password.expose().to_string(),
            },
            UserChange::SetAdmin { name, admin } => Self::SetAdmin {
                name: name.clone(),
                admin: *admin,
            },
            UserChange::SetSoftwareAdmin {
                name,
                software_admin,
            } => Self::SetSoftwareAdmin {
                name: name.clone(),
                software_admin: *software_admin,
            },
            UserChange::Remove { name } => Self::Remove { name: name.clone() },
        }
    }
}

impl From<&UserChangeInMemory> for UserChange {
    fn from(change: &UserChangeInMemory) -> Self {
        match change {
            UserChangeInMemory::SetPassword { name, password } => Self::SetPassword {
                name: name.clone(),
                password: Secret::new(password.as_str()),
            },
            UserChangeInMemory::SetAdmin { name, admin } => Self::SetAdmin {
                name: name.clone(),
                admin: *admin,
            },
            UserChangeInMemory::SetSoftwareAdmin {
                name,
                software_admin,
            } => Self::SetSoftwareAdmin {
                name: name.clone(),
                software_admin: *software_admin,
            },
            UserChangeInMemory::Remove { name } => Self::Remove { name: name.clone() },
        }
    }
}

/// what an operation answered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply", content = "value", rename_all = "snake_case")]
pub enum Reply {
    /// it worked and there is nothing to show.
    Done,
    Document(Document),
    Internal(Internal),
    Leases(Vec<Lease>),
    /// names, such as the logon scripts or the time zones.
    Names(Vec<String>),
    /// a text that may not exist yet, such as a logon script.
    Text(Option<String>),
    ExitCode(i32),
    /// the machine names of the domain, without the trailing `$`, as `MachineRun` takes them.
    Machines(Vec<String>),
    /// what a command on a machine did. the output is decoded text, capped at 1 MiB per stream
    /// (`truncated` says it was cut). `exit_code` is `None` when the run timed out.
    MachineRun {
        exit_code: Option<i32>,
        stdout: String,
        stderr: String,
        timed_out: bool,
        truncated: bool,
    },
    Outcome(Outcome),
    Facts(Box<ServerFacts>),
    Interfaces(Vec<InterfaceStatus>),
    /// what the server said, one line each.
    Lines(Vec<String>),
    /// what an apply would change, per module.
    Plan(Vec<crate::engine::ModulePlan>),
    Clock(ClockState),
    ZoneNow(ZoneNow),
    /// what the server said about a clock change, and whether it was kept in the settings.
    ClockApplied {
        lines: Vec<String>,
        stored: Stored,
    },
    /// what the server can and cannot do, from `lanlord doctor`.
    Doctor(Report),
    RootSsh(RootSshInfo),
    SshAuth(SshAuthInfo),
    /// first-run setup is done; `note` says what did not go as asked (root's ssh access could not
    /// be set, say) and is `None` when everything did.
    SetupDone {
        note: Option<String>,
    },
    Account(AccountInfo),
    /// how a password change went on each linked account.
    Password(PasswordReport),
    Access(Access),
    /// the title and message of the sign-in screen; also what the web ui serves before sign-in.
    LoginText(LoginText),
    /// the server needs the sudo password: a front end that cannot answer a prompt while an
    /// operation runs (the web ui) asks the person and sends the same operation again in a
    /// [`Request`] carrying it. the terminal and the desktop answer the prompt directly and never
    /// see this.
    NeedsPassword {
        prompt: String,
    },
}

/// an operation as the web ui sends it: the operation, and the sudo password once the server has
/// asked for it with [`Reply::NeedsPassword`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    #[serde(rename = "op")]
    pub operation: Operation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<Secret>,
}

/// a reply, or why there is none; what `lanlord op` prints.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Response {
    Ok(Reply),
    Error(String),
    /// the caller's roles do not cover the operation; the reason, in words.
    Denied(String),
}

impl<E: std::fmt::Display> From<Result<Reply, E>> for Response {
    fn from(result: Result<Reply, E>) -> Self {
        match result {
            Ok(reply) => Self::Ok(reply),
            Err(e) => Self::Error(format!("{e:#}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::{NtpChange, Sync};
    use crate::identity::{LinkReport, LinkState};
    use crate::server::Iface;

    fn round_trip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(
        value: &T,
    ) {
        let json = serde_json::to_string(value).unwrap();
        let back: T = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, value, "{json}");
    }

    fn every_operation() -> Vec<Operation> {
        vec![
            Operation::Bootstrap,
            Operation::Apply {
                modules: vec!["dns".into()],
            },
            Operation::PlanApply {
                modules: vec!["nt4-domain".into()],
            },
            Operation::LoadDocument,
            Operation::SaveDocument {
                plain: "dns:\n  zone: example\n".into(),
                updates: vec![SecretUpdate {
                    path: "nt4-domain.admin_password".into(),
                    value: Secret::new("s3cret"),
                }],
            },
            Operation::MoveSecrets {
                key: Secret::new("AGE-SECRET-KEY-s3cret"),
            },
            Operation::MigrateStored {
                stamp: "20260101T120000Z".into(),
            },
            Operation::Probe,
            Operation::ChangeUsers {
                changes: vec![
                    UserChange::SetPassword {
                        name: "alice".into(),
                        password: Secret::new("pw"),
                    },
                    UserChange::SetAdmin {
                        name: "alice".into(),
                        admin: true,
                    },
                    UserChange::SetSoftwareAdmin {
                        name: "bob".into(),
                        software_admin: false,
                    },
                    UserChange::Remove { name: "eve".into() },
                ],
            },
            Operation::ResetMachine {
                name: "pc01".into(),
            },
            Operation::CreateAdmin {
                name: "admin".into(),
                password: Secret::new("pw"),
                ssh_key: Some("ssh-ed25519 AAAA example".into()),
                passwordless_sudo: false,
            },
            Operation::SetupFirstAdmin {
                name: "admin".into(),
                password: Secret::new("pw"),
                root_ssh: RootSsh::KeysOnly,
                ssh_key: None,
            },
            Operation::RootSshState,
            Operation::SetRootSsh {
                access: RootSsh::Off,
            },
            Operation::SshAuthState,
            Operation::SetPasswordLogin {
                passwords: false,
                confirmed: true,
            },
            Operation::AddAdminKey {
                user: "dana".into(),
                key: "ssh-ed25519 AAAA example".into(),
            },
            Operation::RemoveAdminKey {
                user: "dana".into(),
                key: "ssh-ed25519 AAAA".into(),
                confirmed: true,
            },
            Operation::Leases,
            Operation::LogonScripts,
            Operation::ReadLogon {
                name: "alice.bat".into(),
            },
            Operation::ReadGeneratedLogon,
            Operation::WriteLogon {
                name: "alice.bat".into(),
                text: "net use s: \\\\server\\software\n".into(),
            },
            Operation::Run {
                command: vec!["uptime".into()],
            },
            Operation::Power {
                power: Power::Reboot,
            },
            Operation::ServerFacts,
            Operation::NetworkStatus,
            Operation::PurgePreview {
                remove_admins: true,
            },
            Operation::Purge {
                remove_admins: false,
            },
            Operation::ClockState,
            Operation::ClockZones,
            Operation::ClockZoneNow {
                zone: "Europe/Berlin".into(),
            },
            Operation::ClockApply {
                change: ClockChange {
                    timezone: Some("UTC".into()),
                    time: None,
                    ntp: Some(NtpChange {
                        enabled: true,
                        servers: vec!["pool.ntp.org".into()],
                    }),
                },
            },
            Operation::ClockUpdateNow,
            Operation::Doctor,
            Operation::AccountInfo,
            Operation::ChangePassword {
                old: Secret::new("pw"),
                new: Secret::new("pw2"),
            },
            Operation::MyKeys,
            Operation::AddMyKey {
                key: "ssh-ed25519 AAAA example".into(),
            },
            Operation::RemoveMyKey {
                key: "ssh-ed25519 AAAA".into(),
            },
            Operation::UpdateProfile {
                profile: Profile {
                    full_name: "Dana Example".into(),
                    description: String::new(),
                },
            },
            Operation::ResetPassword {
                user: "dana".into(),
                new: Secret::new("pw2"),
            },
            Operation::LoadAccess,
            Operation::SaveAccess {
                access: Access::default(),
            },
            Operation::Subscribe {
                topic: "clock".into(),
            },
            Operation::Machines,
            Operation::MachineRun {
                machine: "NT4SRV01".into(),
                command: "ver".into(),
                timeout_secs: 30,
            },
            Operation::MachinePreset {
                machine: "NT4SRV01".into(),
                preset: "startweb".into(),
                args: vec!["1".into()],
            },
            Operation::ChangeInternet {
                change: InternetChange::Network {
                    network: "lab".into(),
                    on: true,
                },
            },
            Operation::ChangeNetworks {
                edit: Box::new(NetworkEdit::Remove("lab".into())),
            },
            Operation::ChangeSshAccess {
                change: SshChange::Network {
                    network: "lab".into(),
                    on: true,
                },
            },
        ]
    }

    /// the place of each variant in the enum. the match has no catch-all, so adding a variant
    /// stops this compiling until it is listed here, and `every_operation` must then hold it.
    fn slot(operation: &Operation) -> usize {
        match operation {
            Operation::Bootstrap => 0,
            Operation::Apply { .. } => 1,
            Operation::LoadDocument => 2,
            Operation::SaveDocument { .. } => 3,
            Operation::MoveSecrets { .. } => 4,
            Operation::MigrateStored { .. } => 5,
            Operation::Probe => 6,
            Operation::ChangeUsers { .. } => 7,
            Operation::ResetMachine { .. } => 8,
            Operation::CreateAdmin { .. } => 9,
            Operation::ChangeInternet { .. } => 10,
            Operation::ChangeNetworks { .. } => 11,
            Operation::SetupFirstAdmin { .. } => 12,
            Operation::RootSshState => 13,
            Operation::SetRootSsh { .. } => 14,
            Operation::Leases => 15,
            Operation::LogonScripts => 16,
            Operation::ReadLogon { .. } => 17,
            Operation::ReadGeneratedLogon => 18,
            Operation::WriteLogon { .. } => 19,
            Operation::Run { .. } => 20,
            Operation::Power { .. } => 21,
            Operation::ServerFacts => 22,
            Operation::NetworkStatus => 23,
            Operation::PurgePreview { .. } => 24,
            Operation::Purge { .. } => 25,
            Operation::ClockState => 26,
            Operation::ClockZones => 27,
            Operation::ClockZoneNow { .. } => 28,
            Operation::ClockApply { .. } => 29,
            Operation::ClockUpdateNow => 30,
            Operation::Doctor => 31,
            Operation::AccountInfo => 32,
            Operation::ChangePassword { .. } => 33,
            Operation::UpdateProfile { .. } => 34,
            Operation::ResetPassword { .. } => 35,
            Operation::LoadAccess => 36,
            Operation::SaveAccess { .. } => 37,
            Operation::Subscribe { .. } => 38,
            Operation::MyKeys => 39,
            Operation::AddMyKey { .. } => 40,
            Operation::RemoveMyKey { .. } => 41,
            Operation::ChangeSshAccess { .. } => 42,
            Operation::SshAuthState => 43,
            Operation::SetPasswordLogin { .. } => 44,
            Operation::AddAdminKey { .. } => 45,
            Operation::RemoveAdminKey { .. } => 46,
            Operation::PlanApply { .. } => 47,
            Operation::Machines => 48,
            Operation::MachineRun { .. } => 49,
            Operation::MachinePreset { .. } => 50,
        }
    }

    #[test]
    fn every_variant_is_in_every_operation() {
        let mut slots: Vec<usize> = every_operation().iter().map(slot).collect();
        slots.sort_unstable();
        slots.dedup();
        assert_eq!(slots, (0..VARIANTS).collect::<Vec<_>>());
    }

    const VARIANTS: usize = 51;

    #[test]
    fn every_operation_says_what_it_needs() {
        use crate::access::Level;
        // each operation has an entry (the match in `requires` is exhaustive); spot-check the ends
        for operation in every_operation() {
            let _ = operation.requires();
        }
        assert_eq!(Operation::Leases.requires(), Need::view(Area::Network));
        assert_eq!(
            Operation::ChangeInternet {
                change: InternetChange::Network {
                    network: "lab".into(),
                    on: true
                }
            }
            .requires(),
            Need::manage(Area::Network)
        );
        assert_eq!(
            Operation::ChangeNetworks {
                edit: Box::new(NetworkEdit::Remove("lab".into()))
            }
            .requires(),
            Need::manage(Area::Network)
        );
        assert_eq!(
            Operation::Purge {
                remove_admins: false
            }
            .requires(),
            Need::Area(Area::Updates, Level::Manage)
        );
        assert_eq!(Operation::AccountInfo.requires(), Need::SelfOnly);
        assert_eq!(
            Operation::SetRootSsh {
                access: crate::rootssh::RootSsh::Off
            }
            .requires(),
            Need::Admin
        );
        let subscription = |t: &str| Operation::Subscribe { topic: t.into() }.requires();
        assert_eq!(subscription(topic::MONITOR), Need::view(Area::Status));
        assert_eq!(subscription(topic::PROCS), Need::view(Area::Status));
        assert_eq!(subscription(topic::APPLY), Need::view(Area::Services));
        assert_eq!(
            subscription(&topic::machine_run("0a1b2c3d")),
            Need::manage(Area::Commands)
        );
        assert_eq!(topic::machine_run_id("machine-run:0a1b2c3"), None);
        assert_eq!(Operation::Machines.requires(), Need::view(Area::Domain));
        assert!(Operation::MachineRun {
            machine: "NT4SRV01".into(),
            command: "ver".into(),
            timeout_secs: 1
        }
        .needs_gateway_key());
        let old: Operation =
            serde_json::from_str(r#"{"op":"machine_run","machine":"NT4SRV01","command":"ver"}"#)
                .unwrap();
        assert!(matches!(
            old,
            Operation::MachineRun {
                timeout_secs: 120,
                ..
            }
        ));
        // no operation needs `Hidden`, which every account has
        for operation in every_operation() {
            assert!(
                !matches!(operation.requires(), Need::Area(_, Level::Hidden)),
                "{operation:?}"
            );
        }
        // nothing that changes something needs less than Manage
        for operation in every_operation() {
            if matches!(
                operation,
                Operation::Apply { .. }
                    | Operation::PlanApply { .. }
                    | Operation::Power { .. }
                    | Operation::Run { .. }
                    | Operation::MachineRun { .. }
                    | Operation::MachinePreset { .. }
            ) {
                assert!(
                    matches!(operation.requires(), Need::Area(_, Level::Manage)),
                    "{operation:?}"
                );
            }
        }
    }

    #[test]
    fn a_person_edits_only_their_own_ssh_keys() {
        let key: String = "ssh-ed25519 AAAA example".into();
        // a key is removed by its id, whatever options its line has
        let line = r#"from="192.0.2.7",command="true" ssh-ed25519 AAAA example"#;
        let id = crate::sshauth::key_id(line).unwrap();
        for operation in [
            Operation::MyKeys,
            Operation::AddMyKey { key: key.clone() },
            Operation::RemoveMyKey { key: id },
        ] {
            assert_eq!(operation.requires(), Need::SelfOnly, "{operation:?}");
            assert_eq!(operation.validate(), Ok(()));
        }
        assert!(Operation::RemoveMyKey { key: line.into() }
            .validate()
            .is_err());
        let bad = Operation::AddMyKey { key: "nope".into() };
        assert_eq!(bad.validate().unwrap_err().field, "key");
        // the wire format of an older client, without a key, still reads
        let old: Operation = serde_json::from_str(
            r#"{"op":"setup_first_admin","name":"pat","password":"pw","root_ssh":"off"}"#,
        )
        .unwrap();
        assert!(matches!(
            old,
            Operation::SetupFirstAdmin { ssh_key: None, .. }
        ));
    }

    #[test]
    fn every_operation_survives_the_wire() {
        let operations = every_operation();
        // one per action a front end can take: keep this in step with the enum
        assert_eq!(operations.len(), VARIANTS);
        for operation in &operations {
            round_trip(operation);
        }
    }

    #[test]
    fn every_reply_survives_the_wire() {
        let facts = ServerFacts {
            hostname: "gateway".into(),
            os: "Example Linux".into(),
            kernel: "6.1.0".into(),
            uptime_secs: 3_600,
            user: "admin".into(),
            is_root: false,
            cpu_model: "Example CPU @ 2.00GHz".into(),
            cpus: 2,
            sockets: 1,
            cores: 2,
            cpus_limited: false,
            mem_total_mb: 2_048,
            mem_available_mb: 1_024,
            mem_limited: false,
            lanlord_installed: true,
            lanlord_version: Some("0.1.0".into()),
            last_applied: None,
            interfaces: vec![Iface {
                name: "eth0".into(),
                mtu: 1_500,
                state: "UP".into(),
                addresses: vec!["192.0.2.10/24".into()],
                default_route: true,
            }],
        };
        let replies = vec![
            Reply::Done,
            Reply::Document(Document::default()),
            Reply::Internal(Internal {
                interface: "eth1".into(),
                address: "198.51.100.1".parse().unwrap(),
                prefix: 24,
            }),
            Reply::Leases(vec![Lease {
                expires: 0,
                mac: "00:00:5e:00:53:01".into(),
                address: "198.51.100.20".into(),
                name: "pc01".into(),
            }]),
            Reply::Names(vec!["alice.bat".into()]),
            Reply::Text(None),
            Reply::Text(Some("echo hi".into())),
            Reply::ExitCode(3),
            Reply::Machines(vec!["NT4SRV01".into()]),
            Reply::MachineRun {
                exit_code: Some(3),
                stdout: "out".into(),
                stderr: String::new(),
                timed_out: false,
                truncated: false,
            },
            Reply::MachineRun {
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: true,
                truncated: true,
            },
            Reply::Outcome(Outcome::Dropped),
            Reply::Facts(Box::new(facts)),
            Reply::Interfaces(crate::netstatus::demo()),
            Reply::Lines(vec!["done".into()]),
            Reply::Clock(ClockState {
                timezone: "UTC".into(),
                utc_secs: 1_767_268_800,
                offset_secs: 0,
                ntp: true,
                synchronized: false,
                container: false,
                sync: Sync::Timesyncd,
                servers: vec![],
                status: vec!["Server: 192.0.2.123".into()],
            }),
            Reply::ZoneNow(ZoneNow {
                offset_secs: 3_600,
                abbrev: "CET".into(),
            }),
            Reply::ClockApplied {
                lines: vec!["ok".into()],
                stored: Stored::NoKey,
            },
            Reply::NeedsPassword {
                prompt: "[sudo] password for admin:".into(),
            },
            Reply::Doctor(Report::parse(
                "virt_container=none\nvirt_vm=none\nlink_bridge=ok\n",
            )),
            Reply::Access(Access::default()),
            Reply::Account(AccountInfo::default()),
            Reply::Password(PasswordReport {
                ok: false,
                links: vec![LinkReport {
                    link: "Linux account".into(),
                    state: LinkState::Failed("no".into()),
                }],
            }),
        ];
        for reply in &replies {
            round_trip(reply);
        }
        round_trip(&Response::Ok(Reply::Done));
        round_trip(&Response::Error("no key yet".into()));
    }

    #[test]
    fn a_request_carries_the_password_only_when_asked_for() {
        let first = Request {
            operation: Operation::Leases,
            password: None,
        };
        let json = serde_json::to_string(&first).unwrap();
        assert!(!json.contains("password"), "{json}");
        round_trip(&first);
        let retry = Request {
            operation: Operation::Leases,
            password: Some(Secret::new("s3cret")),
        };
        round_trip(&retry);
        assert!(!format!("{retry:?}").contains("s3cret"));
    }

    #[test]
    fn no_operation_shows_a_password_in_debug_output() {
        let shown = format!("{:?}", every_operation());
        assert!(
            !shown.contains("s3cret") && !shown.contains("\"pw\""),
            "{shown}"
        );
    }

    #[test]
    fn user_changes_convert_both_ways() {
        let change = UserChange::SetPassword {
            name: "alice".into(),
            password: Secret::new("pw"),
        };
        let in_memory = UserChangeInMemory::from(&change);
        assert_eq!(
            in_memory,
            UserChangeInMemory::SetPassword {
                name: "alice".into(),
                password: "pw".into()
            }
        );
        assert_eq!(UserChange::from(&in_memory), change);
        for change in [
            UserChange::SetAdmin {
                name: "alice".into(),
                admin: true,
            },
            UserChange::SetSoftwareAdmin {
                name: "bob".into(),
                software_admin: false,
            },
            UserChange::Remove { name: "eve".into() },
        ] {
            let in_memory = UserChangeInMemory::from(&change);
            assert_eq!(UserChange::from(&in_memory), change);
        }
    }

    #[test]
    fn an_operation_reads_as_tagged_json() {
        let operation: Operation =
            serde_json::from_str(r#"{"op":"reset_machine","name":"pc01"}"#).unwrap();
        assert_eq!(
            operation,
            Operation::ResetMachine {
                name: "pc01".into()
            }
        );
        let operation: Operation = serde_json::from_str(r#"{"op":"leases"}"#).unwrap();
        assert_eq!(operation, Operation::Leases);
    }

    #[test]
    fn operations_serialize_exactly_as_before_the_rename() {
        let operations = [
            Operation::Leases,
            Operation::ServerFacts,
            Operation::Apply {
                modules: vec!["dhcp".into()],
            },
            Operation::ResetMachine {
                name: "pc01".into(),
            },
            Operation::Run {
                command: vec!["uptime".into()],
            },
        ];
        let got: Vec<String> = operations
            .iter()
            .map(|operation| serde_json::to_string(operation).unwrap())
            .collect();
        assert_eq!(
            got,
            [
                r#"{"op":"leases"}"#,
                r#"{"op":"server_facts"}"#,
                r#"{"op":"apply","modules":["dhcp"]}"#,
                r#"{"op":"reset_machine","name":"pc01"}"#,
                r#"{"op":"run","command":["uptime"]}"#,
            ]
        );
        let request = Request {
            operation: Operation::Leases,
            password: Some(Secret::new("x")),
        };
        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            r#"{"op":{"op":"leases"},"password":"x"}"#
        );
        let bridge = crate::bridge::Request {
            id: 7,
            operation: Operation::ServerFacts,
        };
        assert_eq!(
            serde_json::to_string(&bridge).unwrap(),
            r#"{"id":7,"op":{"op":"server_facts"}}"#
        );
    }
}
