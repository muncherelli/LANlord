//! every action a front end asks of a server, as data.
//!
//! the gui, the tui, the cli and the web ui all describe what they want as an [`Op`], and lanlord
//! answers with a [`Reply`]. on a client the op runs over ssh; on the gateway (server mode, and
//! the web ui) it runs locally. an op names no server: whoever runs it knows which one.
//!
//! sudo passwords are not part of an op. how a front end that cannot answer a prompt gets
//! privileges (a root helper, or a socket daemon only the lanlord-admins group may use) is not
//! decided yet; until then lanlord asks on its terminal while it runs an op.

use serde::{Deserialize, Serialize};

use crate::clock::{Change as ClockChange, ClockState, Stored, ZoneNow};
use crate::doctor::Report;
use crate::leases::Lease;
use crate::netstatus::InterfaceStatus;
use crate::network::Internal;
use crate::power::{Outcome, Power};
use crate::secret::Secret;
use crate::server::ServerFacts;
use crate::settings::Document;
use crate::users::Change as UserChangeInMemory;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// set the gateway up for lanlord.
    Bootstrap,
    /// apply these modules (all of them when empty) with the stored settings.
    Apply { modules: Vec<String> },
    /// the plain settings text and which secrets are set.
    LoadDocument,
    /// store exactly `plain` as the plain settings and set these secrets; blank values change
    /// nothing.
    SaveDocument {
        plain: String,
        updates: Vec<SecretUpdate>,
    },
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
    /// keep sending samples of `topic` (live monitoring). reserved: no server answers it yet.
    Subscribe { topic: String },
}

impl Op {
    /// does the op change the server, so that the bridge that runs it must be root?
    pub fn needs_root(&self) -> bool {
        matches!(self, Op::ClockApply { .. } | Op::ClockUpdateNow)
    }

    /// can `lanlord bridge` on the gateway run the op itself? the others stay on the shell path.
    pub fn runs_on_bridge(&self) -> bool {
        matches!(
            self,
            Op::ClockState
                | Op::ClockZones
                | Op::ClockZoneNow { .. }
                | Op::ClockApply { .. }
                | Op::ClockUpdateNow
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

/// what an op answered.
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
    Outcome(Outcome),
    Facts(ServerFacts),
    Interfaces(Vec<InterfaceStatus>),
    /// what the server said, one line each.
    Lines(Vec<String>),
    Clock(ClockState),
    ZoneNow(ZoneNow),
    /// what the server said about a clock change, and whether it was kept in the settings.
    ClockApplied {
        lines: Vec<String>,
        stored: Stored,
    },
    /// what the server can and cannot do, from `lanlord doctor`.
    Doctor(Report),
    /// the server needs the sudo password: a front end that cannot answer a prompt while an op
    /// runs (the web ui) asks the person and sends the same op again in a [`Request`] carrying
    /// it. the terminal and the desktop answer the prompt directly and never see this.
    NeedsPassword {
        prompt: String,
    },
}

/// an op as the web ui sends it: the op, and the sudo password once the server has asked for
/// it with [`Reply::NeedsPassword`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub op: Op,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<Secret>,
}

/// a reply, or why there is none; what `lanlord op` prints.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Response {
    Ok(Reply),
    Error(String),
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
    use crate::server::Iface;

    fn round_trip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(
        value: &T,
    ) {
        let json = serde_json::to_string(value).unwrap();
        let back: T = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, value, "{json}");
    }

    fn every_op() -> Vec<Op> {
        vec![
            Op::Bootstrap,
            Op::Apply {
                modules: vec!["dns".into()],
            },
            Op::LoadDocument,
            Op::SaveDocument {
                plain: "dns:\n  zone: example\n".into(),
                updates: vec![SecretUpdate {
                    path: "nt4-domain.admin_password".into(),
                    value: Secret::new("s3cret"),
                }],
            },
            Op::Probe,
            Op::ChangeUsers {
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
            Op::ResetMachine {
                name: "pc01".into(),
            },
            Op::CreateAdmin {
                name: "admin".into(),
                password: Secret::new("pw"),
                ssh_key: Some("ssh-ed25519 AAAA example".into()),
                passwordless_sudo: false,
            },
            Op::Leases,
            Op::LogonScripts,
            Op::ReadLogon {
                name: "alice.bat".into(),
            },
            Op::ReadGeneratedLogon,
            Op::WriteLogon {
                name: "alice.bat".into(),
                text: "net use s: \\\\server\\software\n".into(),
            },
            Op::Run {
                command: vec!["uptime".into()],
            },
            Op::Power {
                power: Power::Reboot,
            },
            Op::ServerFacts,
            Op::NetworkStatus,
            Op::PurgePreview {
                remove_admins: true,
            },
            Op::Purge {
                remove_admins: false,
            },
            Op::ClockState,
            Op::ClockZones,
            Op::ClockZoneNow {
                zone: "Europe/Berlin".into(),
            },
            Op::ClockApply {
                change: ClockChange {
                    timezone: Some("UTC".into()),
                    time: None,
                    ntp: Some(NtpChange {
                        enabled: true,
                        servers: vec!["pool.ntp.org".into()],
                    }),
                },
            },
            Op::ClockUpdateNow,
            Op::Doctor,
            Op::Subscribe {
                topic: "clock".into(),
            },
        ]
    }

    #[test]
    fn every_op_survives_the_wire() {
        let ops = every_op();
        // one per action a front end can take: keep this in step with the enum
        assert_eq!(ops.len(), 26);
        for op in &ops {
            round_trip(op);
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
            cpus: 2,
            mem_total_mb: 2_048,
            mem_available_mb: 1_024,
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
            Reply::Outcome(Outcome::Dropped),
            Reply::Facts(facts),
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
            op: Op::Leases,
            password: None,
        };
        let json = serde_json::to_string(&first).unwrap();
        assert!(!json.contains("password"), "{json}");
        round_trip(&first);
        let retry = Request {
            op: Op::Leases,
            password: Some(Secret::new("s3cret")),
        };
        round_trip(&retry);
        assert!(!format!("{retry:?}").contains("s3cret"));
    }

    #[test]
    fn no_op_shows_a_password_in_debug_output() {
        let shown = format!("{:?}", every_op());
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
    fn an_op_reads_as_tagged_json() {
        let op: Op = serde_json::from_str(r#"{"op":"reset_machine","name":"pc01"}"#).unwrap();
        assert_eq!(
            op,
            Op::ResetMachine {
                name: "pc01".into()
            }
        );
        let op: Op = serde_json::from_str(r#"{"op":"leases"}"#).unwrap();
        assert_eq!(op, Op::Leases);
    }
}
