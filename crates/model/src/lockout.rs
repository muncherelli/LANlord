//! keeping the person who closes ssh to the server from locking themselves out. the firewall
//! drops the sshd ports on a lanlord network unless the network or the computer's mac is allowed,
//! so a change made from a computer on that network would end the connection it was made over.
//! the gateway says where the connection comes from (see `lanlord_core::lockout`); this decides,
//! on the settings as they would be after the change, whether that place is still let in.
//! nothing here touches a machine.

use anyhow::Result;

use crate::lan_networks::LanNetwork;
use crate::ssh_access::{apply_change, Change};

/// where the connection that makes a change comes from, as the gateway sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spot {
    /// the client's address, from `$SSH_CONNECTION`.
    pub address: String,
    /// the interface the gateway reaches it by.
    pub interface: String,
    /// the mac its packets arrive from (the router's, for a client behind one), lowercase.
    pub mac: Option<String>,
}

impl Spot {
    /// read the `address=`, `interface=` and `mac=` lines the gateway's probe prints; none when
    /// the connection is not over a network (no address, or no interface).
    pub fn parse(text: &str) -> Option<Self> {
        let value = |key: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
                .map(str::trim)
                .filter(|v| !v.is_empty())
        };
        Some(Self {
            address: value("address")?.to_string(),
            interface: value("interface")?.to_string(),
            mac: value("mac").map(str::to_lowercase),
        })
    }
}

/// a change that would end the connection it is made over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lockout {
    /// the network the connection comes from.
    pub network: String,
    /// the mac to allow, when the gateway could tell it.
    pub mac: Option<String>,
}

impl Lockout {
    /// what is wrong, in words.
    pub fn sentence(&self) -> String {
        let who = self
            .mac
            .as_ref()
            .map_or(String::new(), |m| format!(" (computer {m})"));
        format!(
            "you are connected from the network {}{who}, and ssh to this server would be off \
             for it, so this connection would be cut off",
            self.network
        )
    }

    /// the commands that keep the connection, the narrowest first.
    pub fn commands(&self, host: &str) -> Vec<String> {
        let network = &self.network;
        let mut commands = Vec::new();
        if let Some(mac) = &self.mac {
            commands.push(format!("lanlord networks {host} ssh {network} allow {mac}"));
        }
        commands.push(format!("lanlord networks {host} ssh {network} on"));
        commands
    }
}

/// does the change take ssh access away? only such a change can cut a connection off.
pub fn takes_access(change: &Change) -> bool {
    matches!(
        change,
        Change::Network { on: false, .. } | Change::Deny { .. }
    )
}

/// would `spot` be let in by the firewall of `list`? `now` is when the grants are looked at; one
/// that has ended lets nobody in. `Some` when it would be kept out, none when it is let in or is
/// not on a lanlord network at all (the host network is never blocked).
pub fn lockout(spot: &Spot, list: &[LanNetwork], now: i64) -> Option<Lockout> {
    let network = list
        .iter()
        .find(|n| !n.interface.is_empty() && n.interface == spot.interface)?;
    let allowed = network.ssh.enabled
        || spot.mac.as_ref().is_some_and(|mac| {
            network
                .ssh
                .live(now)
                .any(|host| host.mac.eq_ignore_ascii_case(mac))
        });
    (!allowed).then(|| Lockout {
        network: network.name.clone(),
        mac: spot.mac.clone(),
    })
}

/// would `spot` be kept out once `change` is made to `list`? a change that gives access cannot.
pub fn after(
    spot: &Spot,
    list: &[LanNetwork],
    change: &Change,
    now: i64,
) -> Result<Option<Lockout>> {
    if !takes_access(change) {
        return Ok(None);
    }
    let mut next = list.to_vec();
    apply_change(&mut next, change, now)?;
    Ok(lockout(spot, &next, now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internet::{AllowedHost, Expiry};

    const NOW: i64 = 1_900_000_000;
    const MAC: &str = "02:00:00:00:00:01";

    fn network(name: &str, interface: &str) -> LanNetwork {
        LanNetwork::new(name, interface, "192.0.2.1/24")
    }

    fn spot(interface: &str, mac: Option<&str>) -> Spot {
        Spot {
            address: "192.0.2.50".into(),
            interface: interface.into(),
            mac: mac.map(str::to_string),
        }
    }

    fn host(mac: &str, until: Option<i64>) -> AllowedHost {
        AllowedHost {
            mac: mac.into(),
            name: None,
            until,
        }
    }

    #[test]
    fn the_probe_output_is_read_and_a_connection_off_the_network_is_none() {
        let text = format!(
            "address=192.0.2.50\ninterface=eth2\nmac={}\n",
            MAC.to_uppercase()
        );
        assert_eq!(Spot::parse(&text), Some(spot("eth2", Some(MAC))));
        // no mac found: still a place, with no mac
        assert_eq!(
            Spot::parse("address=192.0.2.50\ninterface=eth2\nmac=\n"),
            Some(spot("eth2", None))
        );
        assert_eq!(Spot::parse("address=\ninterface=\nmac=\n"), None);
        assert_eq!(Spot::parse(""), None);
    }

    #[test]
    fn a_network_with_ssh_off_keeps_out_a_computer_that_is_not_allowed() {
        let list = [network("lab", "eth2")];
        let held = lockout(&spot("eth2", Some(MAC)), &list, NOW).unwrap();
        assert_eq!(held.network, "lab");
        assert_eq!(held.mac.as_deref(), Some(MAC));
        // unknown mac: kept out too, since nothing can be matched
        assert!(lockout(&spot("eth2", None), &list, NOW).is_some());
    }

    #[test]
    fn a_connection_from_the_host_network_or_an_unknown_interface_is_never_cut() {
        let list = [network("lab", "eth2")];
        assert_eq!(lockout(&spot("eth0", Some(MAC)), &list, NOW), None);
        // a network read from older settings has no interface yet: it matches nothing
        assert_eq!(
            lockout(&spot("", Some(MAC)), &[network("old", "")], NOW),
            None
        );
    }

    #[test]
    fn a_network_that_is_on_or_an_allowed_live_computer_is_let_in() {
        let mut on = network("lab", "eth2");
        on.ssh.enabled = true;
        assert_eq!(lockout(&spot("eth2", None), &[on], NOW), None);
        let mut some = network("lab", "eth2");
        some.ssh.allow = vec![host(MAC, Some(NOW + 60))];
        assert_eq!(
            lockout(&spot("eth2", Some(MAC)), &[some.clone()], NOW),
            None
        );
        // another computer, or the same one after its grant ended, is not
        assert!(lockout(
            &spot("eth2", Some("02:00:00:00:00:09")),
            &[some.clone()],
            NOW
        )
        .is_some());
        assert!(lockout(&spot("eth2", Some(MAC)), &[some], NOW + 61).is_some());
    }

    #[test]
    fn only_a_change_that_takes_access_away_can_cut_a_connection() {
        let mut on = network("lab", "eth2");
        on.ssh.enabled = true;
        let s = spot("eth2", Some(MAC));
        let off = Change::Network {
            network: "lab".into(),
            on: false,
        };
        assert_eq!(
            after(&s, &[on.clone()], &off, NOW)
                .unwrap()
                .map(|l| l.network),
            Some("lab".to_string())
        );
        // turning it off is fine while this computer stays allowed by its mac
        on.ssh.allow = vec![host(MAC, None)];
        assert_eq!(after(&s, &[on.clone()], &off, NOW).unwrap(), None);
        // denying that computer on a network that is off cuts it
        on.ssh.enabled = false;
        let deny = Change::Deny {
            network: "lab".into(),
            mac: MAC.into(),
        };
        assert!(after(&s, &[on.clone()], &deny, NOW).unwrap().is_some());
        // but denying while the whole network is on does not
        on.ssh.enabled = true;
        assert_eq!(after(&s, &[on.clone()], &deny, NOW).unwrap(), None);
        // a change that gives access is never checked
        let allow = Change::Allow {
            network: "lab".into(),
            mac: MAC.into(),
            name: None,
            expiry: Expiry::Never,
        };
        assert_eq!(
            after(&s, &[network("lab", "eth2")], &allow, NOW).unwrap(),
            None
        );
        // the check does not change the list it reads
        assert!(on.ssh.enabled);
    }

    #[test]
    fn the_commands_name_the_network_and_the_mac() {
        let held = Lockout {
            network: "lab".into(),
            mac: Some(MAC.into()),
        };
        assert_eq!(
            held.commands("gw"),
            [
                format!("lanlord networks gw ssh lab allow {MAC}"),
                "lanlord networks gw ssh lab on".to_string()
            ]
        );
        let unknown = Lockout {
            network: "lab".into(),
            mac: None,
        };
        assert_eq!(unknown.commands("gw"), ["lanlord networks gw ssh lab on"]);
        assert!(held.sentence().contains("lab") && held.sentence().contains(MAC));
    }
}
