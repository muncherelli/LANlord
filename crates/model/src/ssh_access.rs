//! ssh to the server from the lanlord networks. a network has none by default: the firewall drops
//! the sshd ports on its interface. it can be switched on for the whole network, or for chosen
//! computers (by mac) on a network that stays off, for good or until a time, the same way the
//! internet is (see `crate::internet`). the grants live in the network's entry in the settings:
//!
//! ```yaml
//! networks:
//!   - name: lab
//!     ssh: false                          # the whole network; default false
//!     ssh_allow:                          # chosen computers; default none
//!       - {mac: "02:00:00:00:00:01", name: dana-pc}                       # for good
//!       - {mac: "02:00:00:00:00:02", until: "2030-01-31T09:00:00Z"}       # until, in utc
//! ```
//!
//! nothing here touches a machine; the docker module turns the grants into nft rules (see
//! ansible/playbooks/docker/firewall.nft.j2). the host network is never affected.

use anyhow::Result;
use serde_yaml_ng::Mapping;

use crate::internet::{
    apply_grant, edit_list, format_time, grant_details, grant_errors, read_grants, write_grants,
    AllowedHost, Change as InternetChange, Expiry,
};
use crate::lan_networks::LanNetwork;
use crate::settings::ValidationError;

/// the ssh settings of one network.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SshAccess {
    /// every computer on the network may reach sshd.
    pub enabled: bool,
    pub allow: Vec<AllowedHost>,
}

impl SshAccess {
    /// the lines of the networks window's read-only box.
    pub fn details(&self) -> Vec<(String, String)> {
        grant_details(
            self.enabled,
            &self.allow,
            "Allow ssh to this server",
            "Ssh allowed for",
        )
    }

    /// read `ssh` and `ssh_allow` from a network's entry; both may be missing.
    pub fn from_entry(item: &Mapping, path: &str) -> Result<Self> {
        let (enabled, allow) = read_grants(item, "ssh", path)?;
        Ok(Self { enabled, allow })
    }

    /// write what differs from the defaults into a network's entry.
    pub fn put_in(&self, entry: &mut Mapping) {
        write_grants(entry, "ssh", self.enabled, &self.allow);
    }
}

/// one change to the ssh settings.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Change {
    /// switch the whole network on or off.
    Network { network: String, on: bool },
    /// let one computer reach sshd; a grant for the same mac is replaced.
    Allow {
        network: String,
        mac: String,
        name: Option<String>,
        expiry: Expiry,
    },
    /// take one computer's grant away.
    Deny { network: String, mac: String },
}

impl Change {
    pub fn describe(&self) -> String {
        match self {
            Change::Network { network, on } => format!(
                "turned ssh to this server {} for {network}",
                if *on { "on" } else { "off" }
            ),
            Change::Allow {
                network,
                mac,
                expiry,
                ..
            } => match expiry {
                Expiry::Never => format!("allowed {mac} to use ssh to this server from {network}"),
                Expiry::At(t) => format!(
                    "allowed {mac} to use ssh to this server from {network} until {}",
                    format_time(*t)
                ),
            },
            Change::Deny { network, mac } => {
                format!("took ssh access away from {mac} on {network}")
            }
        }
    }

    /// the same change as an internet one, which has the same arguments and the same checks.
    fn like_internet(&self) -> InternetChange {
        match self.clone() {
            Change::Network { network, on } => InternetChange::Network { network, on },
            Change::Allow {
                network,
                mac,
                name,
                expiry,
            } => InternetChange::Allow {
                network,
                mac,
                name,
                expiry,
            },
            Change::Deny { network, mac } => InternetChange::Deny { network, mac },
        }
    }

    /// check the arguments on their own; `now` is when the request is made.
    pub fn check(&self, now: i64) -> Result<(), (String, String)> {
        self.like_internet().check(now)
    }
}

impl From<InternetChange> for Change {
    fn from(change: InternetChange) -> Self {
        match change {
            InternetChange::Network { network, on } => Change::Network { network, on },
            InternetChange::Allow {
                network,
                mac,
                name,
                expiry,
            } => Change::Allow {
                network,
                mac,
                name,
                expiry,
            },
            InternetChange::Deny { network, mac } => Change::Deny { network, mac },
        }
    }
}

/// apply one change to a list of networks. `now` is when it is made.
pub fn apply_change(list: &mut [LanNetwork], change: &Change, now: i64) -> Result<()> {
    apply_grant(list, &change.like_internet(), now, "ssh", |n| {
        (&mut n.ssh.enabled, &mut n.ssh.allow)
    })
}

/// what is wrong with a network's ssh settings; empty means fine.
pub fn errors(network: &LanNetwork, path: &str) -> Vec<ValidationError> {
    grant_errors(&network.ssh.allow, &format!("{path}.ssh_allow"))
}

/// the settings text after `change`, checked, with the networks written as a `networks:` list.
pub fn edit_settings(plain: &str, change: &Change, now: i64) -> Result<String> {
    edit_list(plain, |list| apply_change(list, change, now))
}

/// format step 4 to 5: networks gain the optional `ssh` and `ssh_allow` keys. a file without
/// them already means "no ssh from the lanlord networks", so nothing is rewritten; the step exists
/// so that an older lanlord, which would drop the keys when it saves, refuses a file in this format.
pub fn migrate_networks(
    _: &mut crate::settings::Settings,
    _: &crate::format::Context,
) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lan_networks;
    use crate::settings::Settings;

    const NOW: i64 = 1_900_000_000;
    const MAC: &str = "02:00:00:00:00:01";
    const SETTINGS: &str = "docker: {}\nnetworks:\n  - name: lab\n    interface: eth2\n    address: 192.0.2.1/24\n  - name: other\n    interface: eth3\n    address: 198.51.100.1/24\n";

    fn list_of(text: &str) -> Vec<LanNetwork> {
        lan_networks::read(&Settings::parse(text).unwrap())
            .unwrap()
            .list
    }

    fn allow(network: &str, mac: &str, expiry: Expiry) -> Change {
        Change::Allow {
            network: network.into(),
            mac: mac.into(),
            name: None,
            expiry,
        }
    }

    #[test]
    fn ssh_is_off_by_default_and_writing_back_adds_nothing() {
        let list = list_of(SETTINGS);
        assert!(list.iter().all(|n| n.ssh == SshAccess::default()));
        let text = edit_settings(
            SETTINGS,
            &Change::Network {
                network: "lab".into(),
                on: false,
            },
            NOW,
        )
        .unwrap();
        assert!(!text.contains("ssh"), "{text}");
    }

    #[test]
    fn the_keys_round_trip_and_stay_apart_from_internet() {
        let on = Change::Network {
            network: "lab".into(),
            on: true,
        };
        let mut text = edit_settings(SETTINGS, &on, NOW).unwrap();
        text = edit_settings(&text, &allow("other", MAC, Expiry::At(NOW + 60)), NOW).unwrap();
        assert!(
            text.contains("ssh: true") && text.contains("ssh_allow"),
            "{text}"
        );
        assert!(!text.contains("internet"), "{text}");
        let list = list_of(&text);
        assert!(list[0].ssh.enabled && list[0].ssh.allow.is_empty());
        assert!(!list[1].ssh.enabled);
        assert_eq!(list[1].ssh.allow[0].until, Some(NOW + 60));
        assert!(!list[1].internet.any());
    }

    #[test]
    fn a_grant_is_replaced_keeps_its_label_and_is_denied() {
        let named = Change::Allow {
            network: "lab".into(),
            mac: MAC.to_ascii_uppercase(),
            name: Some("dana-pc".into()),
            expiry: Expiry::Never,
        };
        let text = edit_settings(SETTINGS, &named, NOW).unwrap();
        let text = edit_settings(&text, &allow("lab", MAC, Expiry::At(NOW + 3600)), NOW).unwrap();
        let host = &list_of(&text)[0].ssh.allow;
        assert_eq!(host.len(), 1);
        assert_eq!(host[0].name.as_deref(), Some("dana-pc"));
        assert_eq!(host[0].until, Some(NOW + 3600));
        let deny = Change::Deny {
            network: "lab".into(),
            mac: MAC.into(),
        };
        let text = edit_settings(&text, &deny, NOW).unwrap();
        assert!(list_of(&text)[0].ssh.allow.is_empty());
        assert!(edit_settings(&text, &deny, NOW).is_err());
    }

    #[test]
    fn bad_changes_are_refused() {
        for bad in [
            allow("lab", "nope", Expiry::Never),
            allow("lab", MAC, Expiry::At(NOW - 1)),
            allow("nowhere", MAC, Expiry::Never),
            Change::Network {
                network: "Bad Name".into(),
                on: true,
            },
        ] {
            assert!(edit_settings(SETTINGS, &bad, NOW).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn bad_keys_in_the_settings_are_refused() {
        for extra in [
            "    ssh: yes please\n",
            "    ssh_allow: x\n",
            "    ssh_allow: [3]\n",
        ] {
            let text = format!("{SETTINGS}{extra}");
            assert!(
                lan_networks::read(&Settings::parse(&text).unwrap()).is_err(),
                "{extra}"
            );
        }
        let twice = format!(
            "{SETTINGS}    ssh_allow:\n      - {{mac: \"{MAC}\"}}\n      - {{mac: \"{MAC}\"}}\n"
        );
        let list = list_of(&twice);
        assert_eq!(errors(&list[1], "networks[1]").len(), 1);
        assert!(crate::internet::valid_name("dana-pc"));
    }

    #[test]
    fn the_details_show_the_switch_and_the_computers() {
        let mut ssh = SshAccess::default();
        assert_eq!(ssh.details()[0].1, "Off");
        ssh.allow.push(AllowedHost {
            mac: MAC.into(),
            name: Some("dana-pc".into()),
            until: Some(NOW),
        });
        let lines = ssh.details();
        assert_eq!(lines[0].0, "Allow ssh to this server");
        assert_eq!(lines[0].1, "Only for the computers listed");
        assert!(lines[1].1.contains("dana-pc") && lines[1].1.contains("until 2030-03-17"));
        ssh.enabled = true;
        assert_eq!(ssh.details()[0].1, "On for the whole network");
    }

    #[test]
    fn describing_a_change_names_the_network_and_computer() {
        assert!(Change::Network {
            network: "lab".into(),
            on: true
        }
        .describe()
        .contains("on for lab"));
        assert!(allow("lab", MAC, Expiry::Never).describe().contains(MAC));
        assert!(allow("lab", MAC, Expiry::At(NOW))
            .describe()
            .contains("until"));
        assert!(Change::Deny {
            network: "lab".into(),
            mac: MAC.into()
        }
        .describe()
        .contains("away"));
    }
}
