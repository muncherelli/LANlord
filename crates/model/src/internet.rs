//! internet access for the lanlord networks. a network has none by default: its clients get no
//! router from dhcp and the gateway forwards nothing for them. it can be switched on for the
//! whole network, or for chosen computers (by mac) on a network that stays off, for good or until
//! a time. the grants live in the network's entry in the settings:
//!
//! ```yaml
//! networks:
//!   - name: lab
//!     internet: false                     # the whole network; default false
//!     internet_allow:                     # chosen computers; default none
//!       - {mac: "02:00:00:00:00:01", name: dana-pc}                       # for good
//!       - {mac: "02:00:00:00:00:02", until: "2030-01-31T09:00:00Z"}       # until, in utc
//! ```
//!
//! nothing here touches a machine; the docker module turns the grants into nft rules, nat and
//! dnsmasq options (see ansible/playbooks/docker/firewall.nft.j2), and an expired grant stops
//! working on its own because the nft set element carries a timeout.

use anyhow::{anyhow, bail, Result};
use serde_yaml_ng::{Mapping, Value};

use crate::lan_networks::{self, LanNetwork, Networks};
use crate::settings::{valid_mac, Settings, ValidationError};

/// one computer allowed out, identified by its mac.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AllowedHost {
    /// lowercase, like 02:00:00:00:00:01.
    pub mac: String,
    /// a label for people; never used by a machine.
    pub name: Option<String>,
    /// when the grant ends, in seconds since 1970 (utc); none means it does not end.
    pub until: Option<i64>,
}

/// the internet settings of one network.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Internet {
    /// every computer on the network may go out.
    pub enabled: bool,
    pub allow: Vec<AllowedHost>,
}

impl Internet {
    /// does any computer on the network have access, so that forwarding, nat and dns forwarding
    /// are needed there?
    pub fn any(&self) -> bool {
        self.enabled || !self.allow.is_empty()
    }

    /// the grants that have not ended at `now` (seconds since 1970).
    pub fn live(&self, now: i64) -> impl Iterator<Item = &AllowedHost> {
        self.allow
            .iter()
            .filter(move |h| h.until.is_none_or(|until| until > now))
    }

    /// the lines of the networks window's read-only box.
    pub fn details(&self) -> Vec<(String, String)> {
        grant_details(self.enabled, &self.allow, "Internet access", "Internet for")
    }

    /// read `internet` and `internet_allow` from a network's entry; both may be missing.
    pub fn from_entry(item: &Mapping, path: &str) -> Result<Self> {
        let (enabled, allow) = read_grants(item, "internet", path)?;
        Ok(Self { enabled, allow })
    }

    /// write what differs from the defaults into a network's entry.
    pub fn put_in(&self, entry: &mut Mapping) {
        write_grants(entry, "internet", self.enabled, &self.allow);
    }
}

/// the read-only lines of a switch and its list of computers: `first` names the switch, `each`
/// the computers.
pub(crate) fn grant_details(
    enabled: bool,
    allow: &[AllowedHost],
    first: &str,
    each: &str,
) -> Vec<(String, String)> {
    let state = match (enabled, allow.is_empty()) {
        (true, _) => "On for the whole network",
        (false, true) => "Off",
        (false, false) => "Only for the computers listed",
    };
    let mut lines = vec![(first.to_string(), state.to_string())];
    for host in allow {
        lines.push((each.to_string(), host_line(host)));
    }
    lines
}

/// one allowed computer in a list: its name and mac, and until when.
pub fn host_line(host: &AllowedHost) -> String {
    let who = match &host.name {
        Some(name) => format!("{name} ({})", host.mac),
        None => host.mac.clone(),
    };
    let until = host.until.map_or_else(
        || "always".to_string(),
        |t| format!("until {}", format_time(t)),
    );
    format!("{who}, {until}")
}

/// read the switch `key` and the list `key_allow` from a network's entry; both may be missing.
pub(crate) fn read_grants(
    item: &Mapping,
    key: &str,
    path: &str,
) -> Result<(bool, Vec<AllowedHost>)> {
    let enabled = match item.get(key) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(on)) => *on,
        Some(_) => bail!("{path}.{key} must be true or false"),
    };
    let list = format!("{key}_allow");
    let allow = match item.get(list.as_str()) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Sequence(items)) => items
            .iter()
            .enumerate()
            .map(|(i, v)| host_from(v, &format!("{path}.{list}[{i}]")))
            .collect::<Result<_>>()?,
        Some(_) => bail!("{path}.{list} must be a list"),
    };
    Ok((enabled, allow))
}

/// write what differs from the defaults of the switch `key` and its list into a network's entry.
pub(crate) fn write_grants(entry: &mut Mapping, key: &str, enabled: bool, allow: &[AllowedHost]) {
    if enabled {
        entry.insert(Value::String(key.into()), Value::Bool(true));
    }
    if !allow.is_empty() {
        entry.insert(Value::String(format!("{key}_allow")), hosts_value(allow));
    }
}

/// the grants as the yaml list a network's entry holds.
pub(crate) fn hosts_value(allow: &[AllowedHost]) -> Value {
    let key = |s: &str| Value::String(s.into());
    let list = allow
        .iter()
        .map(|host| {
            let mut map = Mapping::new();
            map.insert(key("mac"), key(&host.mac));
            if let Some(name) = &host.name {
                map.insert(key("name"), key(name));
            }
            if let Some(until) = host.until {
                map.insert(key("until"), key(&format_rfc3339(until)));
            }
            Value::Mapping(map)
        })
        .collect();
    Value::Sequence(list)
}

pub(crate) fn host_from(value: &Value, path: &str) -> Result<AllowedHost> {
    let map = value
        .as_mapping()
        .ok_or_else(|| anyhow!("{path} must be a mapping with a mac"))?;
    let text = |name: &str| match map.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(anyhow!("{path}.{name} must be text")),
    };
    let mac = text("mac")?.ok_or_else(|| anyhow!("{path}.mac is missing"))?;
    let until = match text("until")? {
        None => None,
        Some(t) => Some(parse_rfc3339(&t).ok_or_else(|| {
            anyhow!("{path}.until must look like 2030-01-31T09:00:00Z (utc), not {t:?}")
        })?),
    };
    Ok(AllowedHost {
        mac: mac.to_ascii_lowercase(),
        name: text("name")?,
        until,
    })
}

/// seconds since 1970 as `2030-01-31T09:00:00Z`.
pub fn format_rfc3339(time: i64) -> String {
    chrono::DateTime::from_timestamp(time, 0)
        .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default()
}

/// seconds since 1970 as `2030-01-31 09:00 UTC`, for people.
pub fn format_time(time: i64) -> String {
    chrono::DateTime::from_timestamp(time, 0)
        .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_default()
}

/// `2030-01-31T09:00:00Z` as seconds since 1970.
pub fn parse_rfc3339(text: &str) -> Option<i64> {
    chrono::NaiveDateTime::parse_from_str(text.trim().strip_suffix('Z')?, "%Y-%m-%dT%H:%M:%S")
        .ok()
        .map(|t| t.and_utc().timestamp())
}

/// `1h`, `30m`, `2d` or `1d12h`, as seconds. at least a minute, at most a year.
pub fn parse_duration(text: &str) -> Result<i64> {
    let bad = || anyhow!("{text:?} is not a duration like 30m, 1h, 1d or 1d12h");
    let mut total: i64 = 0;
    let mut digits = String::new();
    for c in text.trim().to_ascii_lowercase().chars() {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        let unit = match c {
            'm' => 60,
            'h' => 3600,
            'd' => 86_400,
            _ => return Err(bad()),
        };
        let n: i64 = digits.parse().map_err(|_| bad())?;
        digits.clear();
        total = total.saturating_add(n.saturating_mul(unit));
    }
    if !digits.is_empty() || total == 0 {
        return Err(bad());
    }
    if !(60..=366 * 86_400).contains(&total) {
        bail!("a temporary grant lasts from a minute to a year");
    }
    Ok(total)
}

/// `2030-01-31 09:00` in this computer's time zone, as seconds since 1970. for the command line;
/// the server and the settings only ever see the utc time.
pub fn parse_until_local(text: &str) -> Result<i64> {
    use chrono::TimeZone;
    let naive = chrono::NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%d %H:%M")
        .map_err(|_| anyhow!("{text:?} is not a time like '2030-01-31 09:00'"))?;
    chrono::Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|t| t.timestamp())
        .ok_or_else(|| anyhow!("{text} does not exist in this time zone"))
}

/// how long a grant lasts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Expiry {
    Never,
    /// until this moment, in seconds since 1970 (utc).
    At(i64),
}

/// one change to the internet settings.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Change {
    /// switch the whole network on or off.
    Network { network: String, on: bool },
    /// let one computer go out; a grant for the same mac is replaced.
    Allow {
        network: String,
        mac: String,
        name: Option<String>,
        expiry: Expiry,
    },
    /// take one computer's grant away.
    Deny { network: String, mac: String },
}

/// the words that say what a grant is for, in the sentences that describe a change.
pub(crate) struct Wording {
    /// "turned {} on for NETWORK".
    pub switch: &'static str,
    /// "allowed MAC {} from NETWORK".
    pub allow: &'static str,
    /// "took {} access away from MAC".
    pub access: &'static str,
}

impl Change {
    pub fn describe(&self) -> String {
        self.describe_as(&Wording {
            switch: "internet",
            allow: "out to the internet",
            access: "internet",
        })
    }

    /// say what the change does, in the words of the grant it is for.
    pub(crate) fn describe_as(&self, words: &Wording) -> String {
        match self {
            Change::Network { network, on } => format!(
                "turned {} {} for {network}",
                words.switch,
                if *on { "on" } else { "off" }
            ),
            Change::Allow {
                network,
                mac,
                expiry,
                ..
            } => match expiry {
                Expiry::Never => format!("allowed {mac} {} from {network}", words.allow),
                Expiry::At(t) => format!(
                    "allowed {mac} {} from {network} until {}",
                    words.allow,
                    format_time(*t)
                ),
            },
            Change::Deny { network, mac } => {
                format!("took {} access away from {mac} on {network}", words.access)
            }
        }
    }

    /// check the arguments on their own, for the operation's early feedback; `now` is when the
    /// request is made.
    pub fn check(&self, now: i64) -> Result<(), (String, String)> {
        let bad = |field: &str, message: &str| Err((field.to_string(), message.to_string()));
        match self {
            Change::Network { network, .. } | Change::Deny { network, .. }
                if !crate::settings::valid_label(network) =>
            {
                bad("network", "must be a plain name")
            }
            Change::Allow { network, .. } if !crate::settings::valid_label(network) => {
                bad("network", "must be a plain name")
            }
            Change::Allow { mac, .. } | Change::Deny { mac, .. } if !valid_mac(mac) => {
                bad("mac", "must look like aa:bb:cc:dd:ee:ff")
            }
            Change::Allow {
                name: Some(name), ..
            } if !valid_name(name) => bad("name", "must be 1 to 64 plain characters"),
            Change::Allow {
                expiry: Expiry::At(t),
                ..
            } if *t <= now => bad("until", "is already past"),
            _ => Ok(()),
        }
    }
}

pub(crate) fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.chars().count()) && !name.chars().any(char::is_control)
}

pub(crate) fn missing(name: &str) -> anyhow::Error {
    anyhow!("there is no network called {name}")
}

/// apply one change to a list of networks. `now` is when it is made.
pub fn apply_change(list: &mut [LanNetwork], change: &Change, now: i64) -> Result<()> {
    apply_grant(list, change, now, "internet", |n| {
        (&mut n.internet.enabled, &mut n.internet.allow)
    })
}

/// apply a change to the switch and the list that `select` picks out of a network; `what` names
/// the grant in the messages.
pub(crate) fn apply_grant(
    list: &mut [LanNetwork],
    change: &Change,
    now: i64,
    what: &str,
    select: impl for<'a> Fn(&'a mut LanNetwork) -> (&'a mut bool, &'a mut Vec<AllowedHost>),
) -> Result<()> {
    change.check(now).map_err(|(f, m)| anyhow!("{f} {m}"))?;
    let (Change::Network { network, .. }
    | Change::Allow { network, .. }
    | Change::Deny { network, .. }) = change;
    let net = list
        .iter_mut()
        .find(|n| n.name.eq_ignore_ascii_case(network))
        .ok_or_else(|| missing(network))?;
    let label = net.name.clone();
    let (enabled, allow) = select(net);
    match change {
        Change::Network { on, .. } => *enabled = *on,
        Change::Allow {
            mac, name, expiry, ..
        } => {
            let mac = mac.to_ascii_lowercase();
            // a grant for the same mac is replaced, keeping its label when none is given
            let old = allow.iter().find(|h| h.mac == mac);
            let name = name.clone().or_else(|| old.and_then(|h| h.name.clone()));
            allow.retain(|h| h.mac != mac);
            allow.push(AllowedHost {
                mac,
                name,
                until: match expiry {
                    Expiry::Never => None,
                    Expiry::At(t) => Some(*t),
                },
            });
        }
        Change::Deny { mac, .. } => {
            let mac = mac.to_ascii_lowercase();
            let before = allow.len();
            allow.retain(|h| h.mac != mac);
            if allow.len() == before {
                bail!("{label} has no {what} grant for {mac}");
            }
        }
    }
    Ok(())
}

/// take the grants that have ended at `now` out of the networks; how many went.
pub fn prune(list: &mut [LanNetwork], now: i64) -> usize {
    let mut gone = 0;
    for net in list {
        let before = net.internet.allow.len();
        net.internet
            .allow
            .retain(|h| h.until.is_none_or(|until| until > now));
        gone += before - net.internet.allow.len();
        // the ssh grants end the same way
        let before = net.ssh.allow.len();
        net.ssh
            .allow
            .retain(|h| h.until.is_none_or(|until| until > now));
        gone += before - net.ssh.allow.len();
    }
    gone
}

/// do any networks have a grant that ends?
pub fn has_timed(list: &[LanNetwork]) -> bool {
    list.iter().any(|n| {
        n.internet
            .allow
            .iter()
            .chain(&n.ssh.allow)
            .any(|h| h.until.is_some())
    })
}

/// what is wrong with a network's internet settings; empty means fine.
pub fn errors(network: &LanNetwork, path: &str) -> Vec<ValidationError> {
    grant_errors(&network.internet.allow, &format!("{path}.internet_allow"))
}

/// what is wrong with a list of grants, found at `path` in the settings.
pub(crate) fn grant_errors(allow: &[AllowedHost], path: &str) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (i, host) in allow.iter().enumerate() {
        let here = format!("{path}[{i}]");
        let message = if !valid_mac(&host.mac) {
            "the mac must look like aa:bb:cc:dd:ee:ff".to_string()
        } else if !seen.insert(host.mac.to_ascii_lowercase()) {
            format!("{} is listed twice", host.mac)
        } else if host.name.as_deref().is_some_and(|n| !valid_name(n)) {
            "the name must be 1 to 64 plain characters".to_string()
        } else {
            continue;
        };
        errors.push(ValidationError {
            path: here,
            message,
        });
    }
    errors
}

/// the settings text after `change`, checked, with the networks written as a `networks:` list.
pub fn edit_settings(plain: &str, change: &Change, now: i64) -> Result<String> {
    edit_list(plain, |list| apply_change(list, change, now))
}

/// the settings text with the grants that ended at `now` taken out, and how many went.
pub fn prune_settings(plain: &str, now: i64) -> Result<(String, usize)> {
    let mut gone = 0;
    let text = edit_list(plain, |list| {
        gone = prune(list, now);
        Ok(())
    })?;
    // nothing to take out: the text stays exactly as it was
    Ok((if gone == 0 { plain.to_string() } else { text }, gone))
}

pub(crate) fn edit_list(
    plain: &str,
    edit: impl FnOnce(&mut Vec<LanNetwork>) -> Result<()>,
) -> Result<String> {
    let mut settings = Settings::parse(plain)?;
    let Networks { mut list, .. } = lan_networks::read(&settings)?;
    edit(&mut list)?;
    let problems = errors_of(&list);
    if !problems.is_empty() {
        bail!("{}", problems.join("; "));
    }
    lan_networks::write(&mut settings, &list);
    // the docker module keeps the firewall, so the section has to exist
    if settings.section("docker").is_none() {
        settings.set_section("docker", Value::Mapping(Mapping::new()));
    }
    Ok(settings.text())
}

pub(crate) fn errors_of(list: &[LanNetwork]) -> Vec<String> {
    list.iter()
        .enumerate()
        .flat_map(|(i, n)| {
            let path = format!("networks[{i}]");
            errors(n, &path)
                .into_iter()
                .chain(crate::ssh_access::errors(n, &path))
        })
        .map(|e| e.to_string())
        .collect()
}

/// format step 3 to 4: networks gain the optional `internet` and `internet_allow` keys. a file
/// without them already means "no internet", so nothing is rewritten; the step exists so that an
/// older lanlord, which would drop the keys when it saves, refuses a file that is in this format.
pub fn migrate_networks(
    _: &mut crate::settings::Settings,
    _: &crate::format::Context,
) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_900_000_000;
    const MAC: &str = "02:00:00:00:00:01";

    const SETTINGS: &str = "docker: {}\nnetworks:\n  - name: lab\n    interface: eth2\n    address: 192.0.2.1/24\n    dhcp:\n      range: [192.0.2.100, 192.0.2.200]\n  - name: other\n    interface: eth3\n    address: 198.51.100.1/24\n";

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
    fn a_network_without_the_keys_has_no_internet() {
        let list = list_of(SETTINGS);
        assert!(list.iter().all(|n| n.internet == Internet::default()));
        assert!(!list[0].internet.any());
        // and writing it back adds nothing
        let text = edit_settings(SETTINGS, &allow("lab", MAC, Expiry::Never), NOW).unwrap();
        assert!(!text.contains("internet:"), "{text}");
        assert_eq!(list_of(&text)[1].internet, Internet::default());
    }

    #[test]
    fn the_keys_round_trip() {
        let mut text = edit_settings(
            SETTINGS,
            &Change::Network {
                network: "LAB".into(),
                on: true,
            },
            NOW,
        )
        .unwrap();
        text = edit_settings(
            &text,
            &Change::Allow {
                network: "other".into(),
                mac: "02:00:00:00:00:0A".into(),
                name: Some("dana-pc".into()),
                expiry: Expiry::At(NOW + 3600),
            },
            NOW,
        )
        .unwrap();
        text = edit_settings(&text, &allow("other", MAC, Expiry::Never), NOW).unwrap();
        assert!(text.contains("until: 2030-03-17T18:46:40Z"), "{text}");
        let list = list_of(&text);
        assert!(list[0].internet.enabled && list[0].internet.allow.is_empty());
        assert_eq!(
            list[1].internet.allow,
            [
                AllowedHost {
                    mac: "02:00:00:00:00:0a".into(),
                    name: Some("dana-pc".into()),
                    until: Some(NOW + 3600)
                },
                AllowedHost {
                    mac: MAC.into(),
                    name: None,
                    until: None
                },
            ]
        );
    }

    #[test]
    fn a_grant_is_replaced_by_mac_and_keeps_its_label() {
        let named = Change::Allow {
            network: "lab".into(),
            mac: MAC.into(),
            name: Some("dana-pc".into()),
            expiry: Expiry::Never,
        };
        let one = edit_settings(SETTINGS, &named, NOW).unwrap();
        let two = edit_settings(&one, &allow("lab", MAC, Expiry::At(NOW + 60)), NOW).unwrap();
        let allowed = list_of(&two)[0].internet.allow.clone();
        assert_eq!(allowed.len(), 1);
        let host = &allowed[0];
        assert_eq!(
            (host.name.as_deref(), host.until),
            (Some("dana-pc"), Some(NOW + 60))
        );
    }

    #[test]
    fn deny_removes_and_complains_about_a_missing_grant() {
        let deny = Change::Deny {
            network: "lab".into(),
            mac: MAC.into(),
        };
        let one = edit_settings(SETTINGS, &allow("lab", MAC, Expiry::Never), NOW).unwrap();
        let two = edit_settings(&one, &deny, NOW).unwrap();
        assert!(list_of(&two)[0].internet.allow.is_empty());
        assert!(edit_settings(&two, &deny, NOW).is_err());
    }

    #[test]
    fn a_bad_change_is_refused() {
        let past = allow("lab", MAC, Expiry::At(NOW - 1));
        assert!(edit_settings(SETTINGS, &past, NOW)
            .unwrap_err()
            .to_string()
            .contains("already past"));
        assert!(edit_settings(SETTINGS, &allow("lab", "nope", Expiry::Never), NOW).is_err());
        assert!(
            edit_settings(SETTINGS, &allow("none", MAC, Expiry::Never), NOW)
                .unwrap_err()
                .to_string()
                .contains("no network called none")
        );
    }

    #[test]
    fn pruning_takes_out_only_ended_grants_and_leaves_other_text_alone() {
        let mut text =
            edit_settings(SETTINGS, &allow("lab", MAC, Expiry::At(NOW + 60)), NOW).unwrap();
        text = edit_settings(
            &text,
            &allow("lab", "02:00:00:00:00:02", Expiry::Never),
            NOW,
        )
        .unwrap();
        // nothing ended yet: byte for byte the same
        assert_eq!(prune_settings(&text, NOW).unwrap(), (text.clone(), 0));
        let (after, gone) = prune_settings(&text, NOW + 60).unwrap();
        assert_eq!(gone, 1);
        let allow = &list_of(&after)[0].internet.allow;
        assert_eq!(allow.len(), 1);
        assert_eq!(allow[0].mac, "02:00:00:00:00:02");
        assert!(!has_timed(&list_of(&after)));
        assert!(has_timed(&list_of(&text)));
    }

    #[test]
    fn live_skips_ended_grants() {
        let internet = Internet {
            enabled: false,
            allow: vec![
                AllowedHost {
                    mac: MAC.into(),
                    name: None,
                    until: Some(NOW),
                },
                AllowedHost {
                    mac: "02:00:00:00:00:02".into(),
                    name: None,
                    until: None,
                },
            ],
        };
        assert_eq!(internet.live(NOW).count(), 1);
        assert_eq!(internet.live(NOW - 1).count(), 2);
    }

    #[test]
    fn durations_and_times_parse() {
        assert_eq!(parse_duration("1h").unwrap(), 3600);
        assert_eq!(parse_duration("1d12h").unwrap(), 129_600);
        assert_eq!(parse_duration("90M").unwrap(), 5400);
        for bad in ["", "h", "1", "5s", "0m", "1x", "400d", "-1h"] {
            assert!(parse_duration(bad).is_err(), "{bad}");
        }
        assert_eq!(parse_rfc3339("2030-03-17T17:46:40Z"), Some(NOW));
        assert_eq!(parse_rfc3339("2030-03-17 17:46"), None);
        assert_eq!(format_rfc3339(NOW), "2030-03-17T17:46:40Z");
        assert_eq!(format_time(NOW), "2030-03-17 17:46 UTC");
        assert!(parse_until_local("2030-01-31 09:00").is_ok());
        assert!(parse_until_local("tomorrow").is_err());
    }

    #[test]
    fn the_window_lines_say_who_is_allowed_and_until_when() {
        let mut internet = Internet::default();
        assert_eq!(
            internet.details(),
            [("Internet access".to_string(), "Off".to_string())]
        );
        internet.allow = vec![
            AllowedHost {
                mac: MAC.into(),
                name: Some("dana-pc".into()),
                until: Some(NOW),
            },
            AllowedHost {
                mac: "02:00:00:00:00:02".into(),
                name: None,
                until: None,
            },
        ];
        let lines = internet.details();
        assert_eq!(lines[0].1, "Only for the computers listed");
        assert_eq!(
            lines[1].1,
            "dana-pc (02:00:00:00:00:01), until 2030-03-17 17:46 UTC"
        );
        assert_eq!(lines[2].1, "02:00:00:00:00:02, always");
        internet.enabled = true;
        assert_eq!(internet.details()[0].1, "On for the whole network");
    }

    #[test]
    fn bad_entries_in_the_settings_are_named() {
        let e = lan_networks::read(
            &Settings::parse(
                "networks:\n  - name: lab\n    interface: eth2\n    address: 192.0.2.1/24\n    internet: maybe\n",
            )
            .unwrap(),
        )
        .unwrap_err();
        assert!(e.to_string().contains("networks[0].internet"), "{e}");
        let e = lan_networks::read(
            &Settings::parse(
                "networks:\n  - name: lab\n    interface: eth2\n    address: 192.0.2.1/24\n    internet_allow:\n      - {mac: \"02:00:00:00:00:01\", until: soon}\n",
            )
            .unwrap(),
        )
        .unwrap_err();
        assert!(e.to_string().contains("internet_allow[0].until"), "{e}");
    }
}
