//! this machine's clock: its time zone, internet time (ntp) and wall clock. the same behaviour the
//! shell scripts in `crate::clock` have, without a shell: timedate1 over d-bus where it has the
//! call, files written atomically, and commands run with an argument list.

use std::io::Write;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

use lanlord_model::clock::{
    parse_offset, valid_server, valid_time, valid_zone, Change, ClockState, Sync, ZoneNow,
};

/// where the ntp servers are written; the whole file belongs to lanlord.
pub const TIMESYNCD_FILE: &str = "/etc/systemd/timesyncd.conf.d/lanlord.conf";

/// the units that run chrony: `chronyd` on arch and fedora, `chrony` on debian.
const CHRONY_UNITS: [&str; 2] = ["chronyd", "chrony"];

const CONTAINER: &str = "this server is a container; its clock is set by the host";

/// what is true of this machine, read once before planning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    /// a container takes its clock from the machine it runs on.
    pub container: bool,
    /// what keeps the clock right.
    pub sync: Sync,
    /// the chrony units this distribution has, in the order they are tried.
    pub chrony_units: Vec<String>,
}

/// one thing to do. `plan` returns them in the order they run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// write the servers to lanlord's timesyncd file; no servers removes the file.
    SetServers(Vec<String>),
    /// turn internet time on or off; chrony hosts do it through these units.
    SetNtp {
        enabled: bool,
        chrony_units: Vec<String>,
    },
    /// set the wall clock to this local time (`YYYY-MM-DD HH:MM:SS`).
    SetTime(String),
    SetZone(String),
}

/// what to do to get from `now` to `want`. checks everything, changes nothing. internet time is
/// handled first (turning it on stops manual time), then the time, then the zone.
pub fn plan(now: &ClockState, want: &Change, facts: &Facts) -> Result<Vec<Step>> {
    let ntp_on = want.ntp.as_ref().is_some_and(|n| n.enabled);
    if want.time.is_some() && ntp_on {
        bail!("a time can only be set while automatic synchronization is off");
    }
    let mut steps = Vec::new();
    if let Some(ntp) = &want.ntp {
        if ntp.servers.len() > 8 || ntp.servers.iter().any(|s| !valid_server(s)) {
            bail!("the time servers must be host names or addresses, at most eight");
        }
    }
    let time = want.time.as_deref().map(valid_time).transpose()?;
    if let Some(zone) = &want.timezone {
        if !valid_zone(zone) {
            bail!("{zone} is not a time zone name");
        }
    }
    if let Some(ntp) = &want.ntp {
        if facts.container {
            bail!(CONTAINER);
        }
        steps.push(Step::SetServers(ntp.servers.clone()));
        steps.push(Step::SetNtp {
            enabled: ntp.enabled,
            chrony_units: facts.chrony_units.clone(),
        });
    }
    if let Some(time) = time {
        if facts.container {
            bail!(CONTAINER);
        }
        // the ntp part runs first, so what counts is the state it leaves
        if want.ntp.as_ref().map_or(now.ntp, |n| n.enabled) {
            bail!("turn off automatic synchronization first");
        }
        steps.push(Step::SetTime(time));
    }
    if let Some(zone) = &want.timezone {
        steps.push(Step::SetZone(zone.clone()));
    }
    Ok(steps)
}

/// the text of a timesyncd drop-in that sets `servers`.
fn dropin(servers: &[String]) -> String {
    format!("# written by lanlord\n[Time]\nNTP={}\n", servers.join(" "))
}

/// the servers lanlord wrote, from the text of the drop-in.
fn servers_in(text: &str) -> Vec<String> {
    text.lines()
        .find_map(|l| l.strip_prefix("NTP="))
        .map(|v| v.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// write `text` to `path` so a reader sees the old file or the new one, never half of it.
pub fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let dir = path.parent().context("the file has no folder")?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".lanlord-{}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        // no chmod: the file gets the process umask like the shell's `>` did (0644 under sudo's
        // 022), so services can read it and nobody else can write it
        let _ = std::fs::remove_file(&tmp);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.with_context(|| format!("could not write {}", path.display()))
}

/// run a program with arguments (no shell); its stdout when it succeeds.
fn output(program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("could not run {program}"))?;
    if !out.status.success() {
        bail!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn succeeds(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn unit_active(unit: &str) -> bool {
    succeeds("systemctl", &["is-active", "--quiet", unit])
}

/// read what is true of this machine.
pub fn facts() -> Facts {
    let virt = output("systemd-detect-virt", &["--container"]).unwrap_or_default();
    let chrony_units: Vec<String> = CHRONY_UNITS
        .iter()
        .filter(|u| succeeds("systemctl", &["cat", u]))
        .map(ToString::to_string)
        .collect();
    let sync = if unit_active("systemd-timesyncd") {
        Sync::Timesyncd
    } else if CHRONY_UNITS.iter().any(|u| unit_active(u)) {
        Sync::Chrony
    } else {
        Sync::None
    };
    Facts {
        container: !matches!(virt.trim(), "" | "none"),
        sync,
        chrony_units,
    }
}

#[cfg(target_os = "linux")]
mod bus {
    use anyhow::Result;
    use zbus::blocking::Connection;

    #[zbus::proxy(
        interface = "org.freedesktop.timedate1",
        default_service = "org.freedesktop.timedate1",
        default_path = "/org/freedesktop/timedate1",
        gen_async = false
    )]
    pub trait Timedate {
        fn set_timezone(&self, tz: &str, interactive: bool) -> zbus::Result<()>;
        fn set_ntp(&self, enable: bool, interactive: bool) -> zbus::Result<()>;
        fn list_timezones(&self) -> zbus::Result<Vec<String>>;
        #[zbus(property)]
        fn timezone(&self) -> zbus::Result<String>;
        #[zbus(property, name = "NTP")]
        fn ntp(&self) -> zbus::Result<bool>;
        #[zbus(property, name = "NTPSynchronized")]
        fn ntp_synchronized(&self) -> zbus::Result<bool>;
    }

    pub fn timedate() -> Result<TimedateProxy<'static>> {
        Ok(TimedateProxy::new(&Connection::system()?)?)
    }
}

#[cfg(not(target_os = "linux"))]
mod bus {
    use anyhow::{bail, Result};
    use std::io;

    /// the same calls as the linux proxy; there is no timedate1 here, so each one fails.
    pub struct TimedateProxy<'a>(std::marker::PhantomData<&'a ()>);

    fn none<T>() -> io::Result<T> {
        Err(io::Error::other("this machine has no timedate1"))
    }

    impl TimedateProxy<'_> {
        pub fn set_timezone(&self, _: &str, _: bool) -> io::Result<()> {
            none()
        }
        pub fn set_ntp(&self, _: bool, _: bool) -> io::Result<()> {
            none()
        }
        pub fn list_timezones(&self) -> io::Result<Vec<String>> {
            none()
        }
        pub fn timezone(&self) -> io::Result<String> {
            none()
        }
        pub fn ntp(&self) -> io::Result<bool> {
            none()
        }
        pub fn ntp_synchronized(&self) -> io::Result<bool> {
            none()
        }
    }

    pub fn timedate() -> Result<TimedateProxy<'static>> {
        bail!("this machine has no timedate1")
    }
}

/// the time zone from the `/etc/localtime` link, for machines timedated does not answer on.
fn zone_from_link() -> String {
    std::fs::canonicalize("/etc/localtime")
        .ok()
        .and_then(|p| {
            let p = p.to_string_lossy().into_owned();
            p.split_once("/zoneinfo/").map(|(_, z)| z.to_string())
        })
        .unwrap_or_default()
}

/// the lines `timedatectl timesync-status` prints, at most eight, without leading blanks.
fn timesync_status() -> Vec<String> {
    output("timedatectl", &["timesync-status"])
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .take(8)
        .collect()
}

/// this machine's clock; needs no privilege.
pub fn read() -> Result<ClockState> {
    let facts = facts();
    let td = bus::timedate().ok();
    let timezone = td
        .as_ref()
        .and_then(|td| td.timezone().ok())
        .filter(|z| !z.is_empty() && z != "n/a")
        .unwrap_or_else(zone_from_link);
    let offset = output("date", &["+%z"])
        .ok()
        .and_then(|o| parse_offset(&o))
        .unwrap_or(0);
    let utc_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("the clock is before 1970")?
        .as_secs();
    Ok(ClockState {
        timezone: if timezone.is_empty() {
            "UTC".into()
        } else {
            timezone
        },
        utc_secs: i64::try_from(utc_secs)?,
        offset_secs: offset,
        ntp: td.as_ref().and_then(|td| td.ntp().ok()).unwrap_or(false),
        synchronized: td
            .as_ref()
            .and_then(|td| td.ntp_synchronized().ok())
            .unwrap_or(false),
        container: facts.container,
        sync: facts.sync,
        servers: std::fs::read_to_string(TIMESYNCD_FILE)
            .map(|t| servers_in(&t))
            .unwrap_or_default(),
        status: if facts.sync == Sync::Timesyncd {
            timesync_status()
        } else {
            Vec::new()
        },
    })
}

/// the zone names this machine knows.
pub fn zones() -> Result<Vec<String>> {
    let zones: Vec<String> = bus::timedate()
        .and_then(|td| Ok(td.list_timezones()?))
        .unwrap_or_default()
        .into_iter()
        .filter(|z| valid_zone(z))
        .collect();
    if zones.is_empty() {
        bail!("the server could not list its time zones");
    }
    Ok(zones)
}

fn zone_file(zone: &str) -> std::path::PathBuf {
    Path::new("/usr/share/zoneinfo").join(zone)
}

/// what `zone` looks like on this machine right now.
pub fn zone_now(zone: &str) -> Result<ZoneNow> {
    if !valid_zone(zone) {
        bail!("{zone} is not a time zone name");
    }
    if !zone_file(zone).is_file() {
        bail!("unknown time zone: {zone}");
    }
    let out = Command::new("date")
        .arg("+%z %Z")
        .env("TZ", zone)
        .output()
        .context("could not run date")?;
    let out = String::from_utf8_lossy(&out.stdout);
    let (offset, abbrev) = out.trim().split_once(' ').unwrap_or((out.trim(), ""));
    Ok(ZoneNow {
        offset_secs: parse_offset(offset).context("the server gave no offset")?,
        abbrev: abbrev.to_string(),
    })
}

fn set_zone(zone: &str) -> Result<()> {
    let td = bus::timedate();
    let known = td
        .as_ref()
        .ok()
        .and_then(|td| td.list_timezones().ok())
        .is_some_and(|zones| zones.iter().any(|z| z == zone));
    if !known && !zone_file(zone).is_file() {
        bail!("unknown time zone: {zone}");
    }
    if td.and_then(|td| Ok(td.set_timezone(zone, false)?)).is_err() {
        // timedated does not answer here; do what it would do
        let link = Path::new("/etc/localtime");
        let _ = std::fs::remove_file(link);
        std::os::unix::fs::symlink(zone_file(zone), link)?;
        if Path::new("/etc/timezone").is_file() {
            write_atomic(Path::new("/etc/timezone"), &format!("{zone}\n"))?;
        }
    }
    Ok(())
}

fn set_ntp(enabled: bool, chrony_units: &[String]) -> Result<()> {
    if bus::timedate()
        .and_then(|td| Ok(td.set_ntp(enabled, false)?))
        .is_err()
    {
        for unit in chrony_units {
            let verb = if enabled { "enable" } else { "disable" };
            output("systemctl", &[verb, "--now", unit])?;
        }
    }
    if enabled {
        // best effort: only timesyncd hosts have it
        let _ = succeeds("systemctl", &["restart", "systemd-timesyncd"]);
    }
    Ok(())
}

/// carry out one step; the line it says, if it says one (the same words the scripts used).
fn run(step: &Step) -> Result<Option<String>> {
    Ok(match step {
        Step::SetServers(servers) if servers.is_empty() => {
            match std::fs::remove_file(TIMESYNCD_FILE) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).context("could not remove the time servers file"),
            }
            None
        }
        Step::SetServers(servers) => {
            write_atomic(Path::new(TIMESYNCD_FILE), &dropin(servers))?;
            None
        }
        Step::SetNtp {
            enabled,
            chrony_units,
        } => {
            set_ntp(*enabled, chrony_units)?;
            Some(format!("internet time is {enabled}"))
        }
        Step::SetTime(time) => {
            output("timedatectl", &["set-time", time])?;
            Some(format!("time is {time}"))
        }
        Step::SetZone(zone) => {
            set_zone(zone)?;
            Some(format!("time zone is {zone}"))
        }
    })
}

/// make the changes on this machine; what it said, line by line. needs root.
pub fn apply(change: &Change, log: &mut dyn FnMut(&str)) -> Result<Vec<String>> {
    let steps = plan(&read()?, change, &facts())?;
    let mut said = Vec::new();
    for step in &steps {
        log(&format!("{step:?}"));
        said.extend(run(step)?);
    }
    Ok(said)
}

/// ask the time service to synchronize now; what timesyncd says about it. needs root.
pub fn update_now() -> Result<Vec<String>> {
    if facts().container {
        bail!(CONTAINER);
    }
    let restarted = ["systemd-timesyncd", "chronyd", "chrony"]
        .iter()
        .any(|unit| succeeds("systemctl", &["restart", unit]));
    if !restarted {
        bail!("there is no time service to restart");
    }
    std::thread::sleep(std::time::Duration::from_secs(2));
    Ok(output("timedatectl", &["timesync-status"])
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanlord_model::clock::NtpChange;

    /// name, state now, what is wanted, facts, and the steps or a part of the error.
    type Case = (
        &'static str,
        ClockState,
        Change,
        Facts,
        Result<Vec<Step>, &'static str>,
    );

    fn state(ntp: bool) -> ClockState {
        ClockState {
            timezone: "Europe/Berlin".into(),
            utc_secs: 0,
            offset_secs: 3600,
            ntp,
            synchronized: ntp,
            container: false,
            sync: Sync::Timesyncd,
            servers: vec![],
            status: vec![],
        }
    }

    fn facts_of(container: bool, units: &[&str]) -> Facts {
        Facts {
            container,
            sync: if units.is_empty() {
                Sync::Timesyncd
            } else {
                Sync::Chrony
            },
            chrony_units: units.iter().map(ToString::to_string).collect(),
        }
    }

    fn ntp(enabled: bool, servers: &[&str]) -> Option<NtpChange> {
        Some(NtpChange {
            enabled,
            servers: servers.iter().map(ToString::to_string).collect(),
        })
    }

    fn zone(z: &str) -> Step {
        Step::SetZone(z.into())
    }

    #[test]
    fn plan_table() {
        let host = facts_of(false, &[]);
        let arch = facts_of(false, &["chronyd"]);
        let debian = facts_of(false, &["chrony"]);
        let lxc = facts_of(true, &[]);
        let servers = ["0.pool.example", "192.0.2.7"];
        let on = |units: &[&str]| Step::SetNtp {
            enabled: true,
            chrony_units: units.iter().map(ToString::to_string).collect(),
        };
        let off = Step::SetNtp {
            enabled: false,
            chrony_units: vec![],
        };
        let list = Step::SetServers(servers.iter().map(ToString::to_string).collect());
        let cases: Vec<Case> = vec![
            (
                "nothing asked",
                state(true),
                Change::default(),
                host.clone(),
                Ok(vec![]),
            ),
            (
                "zone only",
                state(true),
                Change {
                    timezone: Some("UTC".into()),
                    ..Change::default()
                },
                host.clone(),
                Ok(vec![zone("UTC")]),
            ),
            (
                "zone is allowed in a container",
                state(true),
                Change {
                    timezone: Some("UTC".into()),
                    ..Change::default()
                },
                lxc.clone(),
                Ok(vec![zone("UTC")]),
            ),
            (
                "servers on timesyncd",
                state(false),
                Change {
                    ntp: ntp(true, &servers),
                    ..Change::default()
                },
                host.clone(),
                Ok(vec![list.clone(), on(&[])]),
            ),
            (
                "ntp on carries the arch chrony unit",
                state(false),
                Change {
                    ntp: ntp(true, &servers),
                    ..Change::default()
                },
                arch,
                Ok(vec![list, on(&["chronyd"])]),
            ),
            (
                "ntp on carries the debian chrony unit",
                state(false),
                Change {
                    ntp: ntp(true, &[]),
                    ..Change::default()
                },
                debian,
                Ok(vec![Step::SetServers(vec![]), on(&["chrony"])]),
            ),
            (
                "ntp off then the time then the zone, in that order",
                state(true),
                Change {
                    timezone: Some("Asia/Kolkata".into()),
                    time: Some("2026-11-15 17:30:00".into()),
                    ntp: ntp(false, &[]),
                },
                host.clone(),
                Ok(vec![
                    Step::SetServers(vec![]),
                    off,
                    Step::SetTime("2026-11-15 17:30:00".into()),
                    zone("Asia/Kolkata"),
                ]),
            ),
            (
                "time while ntp stays on",
                state(true),
                Change {
                    time: Some("2026-11-15 17:30:00".into()),
                    ..Change::default()
                },
                host.clone(),
                Err("turn off automatic synchronization first"),
            ),
            (
                "time while ntp is off",
                state(false),
                Change {
                    time: Some("2026-11-15 17:30:00".into()),
                    ..Change::default()
                },
                host.clone(),
                Ok(vec![Step::SetTime("2026-11-15 17:30:00".into())]),
            ),
            (
                "time and ntp on together",
                state(false),
                Change {
                    time: Some("2026-11-15 17:30:00".into()),
                    ntp: ntp(true, &[]),
                    ..Change::default()
                },
                host.clone(),
                Err("only be set while automatic synchronization is off"),
            ),
            (
                "time in a container",
                state(false),
                Change {
                    time: Some("2026-11-15 17:30:00".into()),
                    ..Change::default()
                },
                lxc.clone(),
                Err("this server is a container"),
            ),
            (
                "ntp in a container",
                state(false),
                Change {
                    ntp: ntp(false, &[]),
                    ..Change::default()
                },
                lxc,
                Err("this server is a container"),
            ),
            (
                "bad zone",
                state(true),
                Change {
                    timezone: Some("../etc".into()),
                    ..Change::default()
                },
                host.clone(),
                Err("not a time zone name"),
            ),
            (
                "bad time",
                state(false),
                Change {
                    time: Some("soon".into()),
                    ..Change::default()
                },
                host.clone(),
                Err("must look like"),
            ),
            (
                "a server that could add a config line",
                state(true),
                Change {
                    ntp: ntp(true, &["a.example\nNTP=evil"]),
                    ..Change::default()
                },
                host.clone(),
                Err("host names or addresses"),
            ),
            (
                "nine servers",
                state(true),
                Change {
                    ntp: ntp(true, &["a.example"; 9]),
                    ..Change::default()
                },
                host,
                Err("at most eight"),
            ),
        ];
        for (name, now, want, facts, expect) in cases {
            match (plan(&now, &want, &facts), expect) {
                (Ok(steps), Ok(expect)) => assert_eq!(steps, expect, "{name}"),
                (Err(e), Err(part)) => assert!(e.to_string().contains(part), "{name}: {e}"),
                (got, expect) => panic!("{name}: got {got:?}, wanted {expect:?}"),
            }
        }
    }

    #[test]
    fn this_machine_can_be_read_and_an_empty_change_does_nothing() {
        let state = read().unwrap();
        assert!(!state.timezone.is_empty());
        assert!(state.utc_secs > 1_700_000_000);
        let facts = facts();
        assert!(facts
            .chrony_units
            .iter()
            .all(|u| CHRONY_UNITS.contains(&u.as_str())));
        let said = apply(&Change::default(), &mut |_| panic!("no step to log")).unwrap();
        assert!(said.is_empty());
        // listing zones needs timedated; without it the answer is an error, never an empty list
        if let Ok(zones) = zones() {
            assert!(!zones.is_empty());
        }
    }

    #[test]
    fn a_zone_is_looked_up_in_the_zoneinfo_folder() {
        if zone_file("UTC").is_file() {
            let now = zone_now("UTC").unwrap();
            assert_eq!(now.offset_secs, 0);
            assert!(!now.abbrev.is_empty());
        }
        assert!(zone_now("../etc/passwd")
            .unwrap_err()
            .to_string()
            .contains("not a time zone"));
        assert!(zone_now("Nowhere/Land")
            .unwrap_err()
            .to_string()
            .contains("unknown time zone"));
    }

    #[test]
    fn running_a_command_reports_failure_and_missing_programs() {
        assert!(output("true", &[]).is_ok());
        assert!(output("false", &[])
            .unwrap_err()
            .to_string()
            .contains("failed"));
        assert!(output("lanlord-no-such-program", &[]).is_err());
        assert!(succeeds("true", &[]) && !succeeds("false", &[]));
        assert!(!succeeds("lanlord-no-such-program", &[]));
        assert!(!unit_active("lanlord-no-such.service"));
    }

    #[test]
    fn the_dropin_round_trips() {
        let servers = vec!["0.pool.example".to_string(), "192.0.2.7".to_string()];
        let text = dropin(&servers);
        assert_eq!(
            text,
            "# written by lanlord\n[Time]\nNTP=0.pool.example 192.0.2.7\n"
        );
        assert_eq!(servers_in(&text), servers);
        assert!(servers_in("[Time]\n").is_empty());
    }

    #[test]
    fn files_are_written_whole_and_replaced() {
        let dir = std::env::temp_dir().join(format!("lanlord-local-clock-{}", std::process::id()));
        let file = dir.join("sub/lanlord.conf");
        write_atomic(&file, "one\n").unwrap();
        write_atomic(&file, "two\n").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "two\n");
        let left: Vec<_> = std::fs::read_dir(file.parent().unwrap()).unwrap().collect();
        assert_eq!(left.len(), 1, "no temporary file stays");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
