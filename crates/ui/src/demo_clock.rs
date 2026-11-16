//! a server's clock answered from memory, for looking at the Date/Time window without a
//! server: set `LANLORD_FAKE_CLOCK=1`, or `LANLORD_FAKE_CLOCK=container` to see how a container
//! (whose clock is its host's) looks. nothing is sent anywhere.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use lanlord_core::clock::{Change, ClockState, Stored, Sync, ZoneNow};

static STATE: Mutex<Option<ClockState>> = Mutex::new(None);

/// `Some(container)` when the demo is on.
pub fn mode() -> Option<bool> {
    std::env::var("LANLORD_FAKE_CLOCK")
        .ok()
        .map(|v| v == "container")
}

const ZONES: [(&str, i32, &str); 8] = [
    ("Africa/Cairo", 7200, "EET"),
    ("America/Argentina/Buenos_Aires", -10_800, "-03"),
    ("America/New_York", -18_000, "EST"),
    ("Asia/Kolkata", 19_800, "IST"),
    ("Australia/Sydney", 39_600, "AEDT"),
    ("Europe/Berlin", 3600, "CET"),
    ("Europe/London", 0, "GMT"),
    ("UTC", 0, "UTC"),
];

fn offset_of(zone: &str) -> i32 {
    ZONES
        .iter()
        .find(|(name, _, _)| *name == zone)
        .map_or(0, |(_, offset, _)| *offset)
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// the demo server's clock now: real time, in whatever zone it was last set to.
pub fn state(container: bool) -> ClockState {
    let mut held = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let kept = held.get_or_insert_with(|| ClockState {
        timezone: "America/New_York".into(),
        utc_secs: 0,
        offset_secs: -18_000,
        ntp: !container,
        synchronized: !container,
        container,
        sync: if container {
            Sync::None
        } else {
            Sync::Timesyncd
        },
        servers: Vec::new(),
        status: if container {
            Vec::new()
        } else {
            vec![
                "Server: 162.159.200.1 (pool.ntp.org)".into(),
                "Poll interval: 17min 4s (min: 32s; max 34min 8s)".into(),
            ]
        },
    });
    kept.utc_secs = now_secs();
    kept.offset_secs = offset_of(&kept.timezone);
    kept.clone()
}

pub fn zones() -> Vec<String> {
    ZONES
        .iter()
        .map(|(name, _, _)| (*name).to_string())
        .collect()
}

pub fn zone_now(zone: &str) -> ZoneNow {
    let abbrev = ZONES
        .iter()
        .find(|(name, _, _)| *name == zone)
        .map_or("UTC", |(_, _, abbrev)| abbrev);
    ZoneNow {
        offset_secs: offset_of(zone),
        abbrev: abbrev.into(),
    }
}

/// what the demo server does with a change: the zone and internet time stick, a time is only said.
pub fn apply(change: &Change, container: bool) -> (Vec<String>, Stored) {
    let mut said = Vec::new();
    let mut current = state(container);
    if let Some(ntp) = &change.ntp {
        current.ntp = ntp.enabled;
        current.synchronized = ntp.enabled;
        current.servers.clone_from(&ntp.servers);
        said.push(format!("internet time is {}", ntp.enabled));
    }
    if let Some(time) = &change.time {
        said.push(format!("time is {time}"));
    }
    if let Some(zone) = &change.timezone {
        current.timezone.clone_from(zone);
        said.push(format!("time zone is {zone}"));
    }
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = Some(current);
    (said, Stored::Yes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanlord_core::clock::NtpChange;

    #[test]
    fn the_demo_clock_remembers_what_it_was_given() {
        std::env::set_var("LANLORD_FAKE_CLOCK", "1");
        assert_eq!(mode(), Some(false));
        let first = state(false);
        assert_eq!(first.timezone, "America/New_York");
        assert_eq!(first.offset_secs, -18_000);
        assert!(first.ntp && !first.container && first.utc_secs > 1_700_000_000);
        assert_eq!(zones().len(), 8);
        assert_eq!(zone_now("Europe/Berlin").abbrev, "CET");
        assert_eq!(zone_now("Nowhere/Land").offset_secs, 0);
        let (said, stored) = apply(
            &Change {
                timezone: Some("Europe/Berlin".into()),
                time: Some("2026-11-15 17:30:00".into()),
                ntp: Some(NtpChange {
                    enabled: false,
                    servers: vec!["a.org".into()],
                }),
            },
            false,
        );
        assert_eq!(said.len(), 3);
        assert_eq!(stored, Stored::Yes);
        let now = state(false);
        assert_eq!(
            (now.timezone.as_str(), now.offset_secs),
            ("Europe/Berlin", 3600)
        );
        assert!(!now.ntp);
        assert_eq!(now.servers, ["a.org"]);
        std::env::set_var("LANLORD_FAKE_CLOCK", "container");
        assert_eq!(mode(), Some(true));
        std::env::remove_var("LANLORD_FAKE_CLOCK");
        assert_eq!(mode(), None);
    }
}
