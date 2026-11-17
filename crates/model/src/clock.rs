//! the server's clock: its time, its time zone and its internet time (ntp). the time zone and
//! the ntp servers are also kept in the `gateway` settings so a later apply does not undo them;
//! a time set by hand is a one-off and is not kept.

use anyhow::{bail, Context, Result};
use chrono::NaiveDateTime;
use serde_yaml_ng::{Mapping, Value};

use crate::settings::Settings;

/// what keeps the clock right on the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Sync {
    Timesyncd,
    Chrony,
    None,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClockState {
    pub timezone: String,
    /// seconds since 1970 (utc) when the server was asked.
    pub utc_secs: i64,
    /// the server's offset from utc in seconds, in its own time zone.
    pub offset_secs: i32,
    /// is automatic synchronization on?
    pub ntp: bool,
    /// has the clock been synchronized?
    pub synchronized: bool,
    /// a container (lxc, docker...) takes its clock from the machine it runs on.
    pub container: bool,
    pub sync: Sync,
    /// the servers lanlord configured, empty for the system default.
    pub servers: Vec<String>,
    /// the lines timesyncd reports about its last exchange.
    pub status: Vec<String>,
}

impl ClockState {
    /// the server's local time `secs_later` seconds after it was read, as `YYYY-MM-DD HH:MM:SS`.
    pub fn local_text(&self, secs_later: i64) -> String {
        local_text(self.utc_secs + secs_later, self.offset_secs)
    }
}

/// the local time at `utc_secs` in a zone `offset_secs` from utc.
pub fn local_text(utc_secs: i64, offset_secs: i32) -> String {
    chrono::DateTime::from_timestamp(utc_secs + i64::from(offset_secs), 0)
        .map_or_else(String::new, |t| t.format("%Y-%m-%d %H:%M:%S").to_string())
}

/// an offset like `+01:00`.
pub fn offset_text(offset_secs: i32) -> String {
    let sign = if offset_secs < 0 { '-' } else { '+' };
    let abs = offset_secs.abs();
    format!("{sign}{:02}:{:02}", abs / 3600, abs % 3600 / 60)
}

/// the clock state as lines of text, for the command line.
pub fn describe(state: &ClockState) -> String {
    let mut lines = vec![
        format!("time zone:     {}", state.timezone),
        format!(
            "local time:    {} (UTC{})",
            state.local_text(0),
            offset_text(state.offset_secs)
        ),
    ];
    let internet = match (state.ntp, state.synchronized) {
        (true, true) => "on, synchronized",
        (true, false) => "on, not synchronized yet",
        (false, _) => "off",
    };
    let by = match state.sync {
        Sync::Timesyncd => "systemd-timesyncd",
        Sync::Chrony => "chrony",
        Sync::None => "no time service running",
    };
    lines.push(format!("internet time: {internet} ({by})"));
    if !state.servers.is_empty() {
        lines.push(format!("time servers:  {}", state.servers.join(" ")));
    }
    if state.container {
        lines.push(format!("note:          {CONTAINER_NOTE}"));
    }
    lines.extend(state.status.iter().map(|l| format!("               {l}")));
    lines.join("\n")
}

/// what a time zone looks like right now on the server.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ZoneNow {
    pub offset_secs: i32,
    pub abbrev: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NtpChange {
    pub enabled: bool,
    pub servers: Vec<String>,
}

/// what to change. every part is optional.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Change {
    pub timezone: Option<String>,
    /// local time on the server, `YYYY-MM-DD HH:MM:SS`.
    pub time: Option<String>,
    pub ntp: Option<NtpChange>,
}

/// the container message shown wherever the time or ntp cannot be set.
pub const CONTAINER_NOTE: &str =
    "This server is a container; its clock is set by the host. You can still change the time zone.";

/// a zone name such as `Europe/Berlin`: letters, digits and `_ + -` in `/` separated parts.
pub fn valid_zone(zone: &str) -> bool {
    !zone.is_empty()
        && zone.len() <= 64
        && zone
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && zone
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-' | '/'))
}

/// a host name or an ip address; nothing that could be more than that.
pub fn valid_server(server: &str) -> bool {
    let host = !server.is_empty()
        && server.len() <= 253
        && server.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
    let v6 = server.contains(':') && server.parse::<std::net::Ipv6Addr>().is_ok();
    host || v6
}

/// the servers in a text field: separated by spaces or commas, at most eight.
pub fn parse_servers(text: &str) -> Result<Vec<String>> {
    let servers: Vec<String> = text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if servers.len() > 8 {
        bail!("use at most eight time servers");
    }
    if let Some(bad) = servers.iter().find(|s| !valid_server(s)) {
        bail!("{bad} is not a valid server name or address");
    }
    Ok(servers)
}

/// `YYYY-MM-DD HH:MM:SS`, checked as a real date and time.
pub fn valid_time(text: &str) -> Result<String> {
    NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%d %H:%M:%S")
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .map_err(|_| anyhow::anyhow!("the time must look like 2026-11-15 17:30:00"))
}

#[doc(hidden)]
pub fn parse_offset(text: &str) -> Option<i32> {
    let text = text.trim();
    let sign = match text.chars().next()? {
        '+' => 1,
        '-' => -1,
        _ => return None,
    };
    let digits = &text[1..];
    if digits.len() != 4 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (h, m) = (
        digits[..2].parse::<i32>().ok()?,
        digits[2..].parse::<i32>().ok()?,
    );
    Some(sign * (h * 3600 + m * 60))
}

/// the clock state from the lines the read script prints.
pub fn parse_state(text: &str) -> Result<ClockState> {
    let mut state = ClockState {
        timezone: String::new(),
        utc_secs: 0,
        offset_secs: 0,
        ntp: false,
        synchronized: false,
        container: false,
        sync: Sync::None,
        servers: Vec::new(),
        status: Vec::new(),
    };
    let mut fallback_zone = String::new();
    let mut got_epoch = false;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key {
            "Timezone" => state.timezone = value.to_string(),
            "Zone" => fallback_zone = value.to_string(),
            "NTP" => state.ntp = value == "yes",
            "NTPSynchronized" => state.synchronized = value == "yes",
            "Container" => state.container = !value.is_empty() && value != "none",
            "Epoch" => {
                state.utc_secs = value.parse().context("the server gave no time")?;
                got_epoch = true;
            }
            "Offset" => state.offset_secs = parse_offset(value).unwrap_or(0),
            "Sync" => {
                state.sync = match value {
                    "timesyncd" => Sync::Timesyncd,
                    "chrony" => Sync::Chrony,
                    _ => Sync::None,
                }
            }
            "Servers" => state.servers = value.split_whitespace().map(str::to_string).collect(),
            "Status" if !value.is_empty() => state.status.push(value.to_string()),
            _ => {}
        }
    }
    if !got_epoch {
        bail!("the server did not report its time");
    }
    if state.timezone.is_empty() || state.timezone == "n/a" {
        state.timezone = fallback_zone;
    }
    if state.timezone.is_empty() {
        state.timezone = "UTC".into();
    }
    Ok(state)
}

/// the clock settings kept in the `gateway` section: the time zone, the ntp servers and
/// whether internet time is on.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Kept {
    pub timezone: Option<String>,
    pub ntp: Option<Vec<String>>,
    pub ntp_enabled: Option<bool>,
}

impl Kept {
    pub fn from_settings(settings: &Settings) -> Self {
        let Some(Value::Mapping(map)) = settings.section("gateway") else {
            return Self::default();
        };
        Self {
            timezone: map
                .get("timezone")
                .and_then(Value::as_str)
                .map(str::to_string),
            ntp: map.get("ntp").and_then(Value::as_sequence).map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            }),
            ntp_enabled: map.get("ntp_enabled").and_then(Value::as_bool),
        }
    }
}

/// write what `change` sets into the `gateway` section; its other keys stay.
pub fn keep_change(change: &Change, settings: &mut Settings) {
    let mut map = match settings.section("gateway") {
        Some(Value::Mapping(map)) => map.clone(),
        _ => Mapping::new(),
    };
    if let Some(zone) = &change.timezone {
        map.insert("timezone".into(), zone.as_str().into());
    }
    if let Some(ntp) = &change.ntp {
        let servers = ntp.servers.iter().map(|s| s.as_str().into()).collect();
        map.insert("ntp".into(), Value::Sequence(servers));
        map.insert("ntp_enabled".into(), ntp.enabled.into());
    }
    settings.set_section("gateway", Value::Mapping(map));
}

/// whether a change was also kept in the settings.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Stored {
    Yes,
    /// nothing in the change is kept (only a time was set).
    Nothing,
    /// there is no key to store the settings with.
    NoKey,
    /// lanlord is not installed on the server, so there are no settings to keep it in.
    NotInstalled,
    /// the server took the change, but keeping it in the settings failed; the text says why.
    Failed(String),
}
