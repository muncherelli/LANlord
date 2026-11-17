//! the server's clock: its time, its time zone and its internet time (ntp). the time zone and
//! the ntp servers are also kept in the `gateway` settings so a later apply does not undo them;
//! a time set by hand is a one-off and is not kept.

use anyhow::{bail, Context, Result};

use crate::bootstrap::{capture_as_root, AskPassword};
use crate::bridge;
use crate::config;
use crate::crypto::Identity;
use crate::log::{emit, Kind};
use crate::settings::{self, Settings};
use crate::ssh::Session;
use lanlord_model::ops::{Op, Reply};

pub use lanlord_model::clock::*;

/// where the ntp servers are written; the whole file belongs to lanlord.
const TIMESYNCD_FILE: &str = "/etc/systemd/timesyncd.conf.d/lanlord.conf";

/// what the server says about its clock; needs no sudo.
const READ: &str = r#"timedatectl show 2>/dev/null
echo "Container=$(systemd-detect-virt --container 2>/dev/null || true)"
echo "Epoch=$(date -u +%s)"
echo "Offset=$(date +%z)"
echo "Zone=$(readlink -f /etc/localtime 2>/dev/null | sed 's#.*/zoneinfo/##')"
if systemctl is-active --quiet systemd-timesyncd 2>/dev/null; then
  echo "Sync=timesyncd"
elif systemctl is-active --quiet chronyd 2>/dev/null || systemctl is-active --quiet chrony 2>/dev/null; then
  echo "Sync=chrony"
else
  echo "Sync=none"
fi
echo "Servers=$(sed -n 's/^NTP=//p' @FILE@ 2>/dev/null | head -n 1)"
timedatectl timesync-status 2>/dev/null | sed 's/^ *//; s/^/Status=/' | head -n 8
"#;

/// the zone names the server knows.
const ZONES: &str = "timedatectl list-timezones 2>/dev/null\n";

/// what `zone` looks like now: its offset and abbreviation. the zone must exist on the server.
const ZONE_NOW: &str = r#"zone='@ZONE@'
if [ ! -f "/usr/share/zoneinfo/$zone" ]; then echo "unknown time zone: $zone" >&2; exit 2; fi
TZ="$zone" date +'%z %Z'
"#;

/// stop when the clock is the host's.
const NOT_A_CONTAINER: &str = r#"virt=$(systemd-detect-virt --container 2>/dev/null || true)
case "$virt" in
  ""|none) ;;
  *) echo "this server is a container; its clock is set by the host" >&2; exit 4 ;;
esac
"#;

const SET_ZONE: &str = r#"set -eu
zone='@ZONE@'
if ! timedatectl list-timezones 2>/dev/null | grep -Fxq -- "$zone"; then
  if [ ! -f "/usr/share/zoneinfo/$zone" ]; then echo "unknown time zone: $zone" >&2; exit 2; fi
fi
if ! timedatectl set-timezone "$zone" 2>/dev/null; then
  ln -sf "/usr/share/zoneinfo/$zone" /etc/localtime
  if [ -f /etc/timezone ]; then echo "$zone" > /etc/timezone; fi
fi
echo "time zone is $zone"
"#;

const SET_TIME: &str = r#"set -eu
@NOT_A_CONTAINER@
if [ "$(timedatectl show --property=NTP --value 2>/dev/null)" = yes ]; then
  echo "turn off automatic synchronization first" >&2
  exit 3
fi
timedatectl set-time '@TIME@'
echo "time is @TIME@"
"#;

const SET_NTP: &str = r#"set -eu
@NOT_A_CONTAINER@
enabled=@ENABLED@
servers='@SERVERS@'
install -d -m 0755 "$(dirname @FILE@)"
if [ -n "$servers" ]; then
  printf '%s\n' '# written by lanlord' '[Time]' "NTP=$servers" > @FILE@
else
  rm -f @FILE@
fi
if ! timedatectl set-ntp "$enabled" 2>/dev/null; then
  for unit in chronyd chrony; do
    if systemctl cat "$unit" >/dev/null 2>&1; then
      if [ "$enabled" = true ]; then systemctl enable --now "$unit"; else systemctl disable --now "$unit"; fi
    fi
  done
fi
if [ "$enabled" = true ]; then systemctl restart systemd-timesyncd 2>/dev/null || true; fi
echo "internet time is $enabled"
"#;

const UPDATE_NOW: &str = r#"set -eu
@NOT_A_CONTAINER@
if ! systemctl restart systemd-timesyncd 2>/dev/null && ! systemctl restart chronyd 2>/dev/null && ! systemctl restart chrony 2>/dev/null; then
  echo "there is no time service to restart" >&2
  exit 5
fi
sleep 2
timedatectl timesync-status 2>/dev/null || true
"#;

/// a script with its placeholders filled in; `file` is where the ntp servers live.
fn render_in(template: &str, file: &str, pairs: &[(&str, &str)]) -> String {
    let mut text = template
        .replace("@NOT_A_CONTAINER@", NOT_A_CONTAINER)
        .replace("@FILE@", file);
    for (key, value) in pairs {
        text = text.replace(&format!("@{key}@"), value);
    }
    text
}

fn render(template: &str, pairs: &[(&str, &str)]) -> String {
    render_in(template, TIMESYNCD_FILE, pairs)
}

/// the bridge's answer to `op`, or `None` when the gateway has no bridge and the shell scripts
/// below must do it. `pick` takes the one reply kind `op` may get; any other kind is an error,
/// never a reason to fall back (the bridge may already have made the change).
fn via_bridge<T>(
    session: &Session,
    op: &Op,
    ask: AskPassword,
    pick: fn(&Reply) -> Option<T>,
) -> Result<Option<T>> {
    match bridge::try_call(session, op, ask)? {
        Some(reply) => match pick(&reply) {
            Some(value) => Ok(Some(value)),
            None => bail!("the bridge answered {reply:?} to {op:?}"),
        },
        None => Ok(None),
    }
}

fn lines_of(reply: &Reply) -> Option<Vec<String>> {
    match reply {
        Reply::Lines(lines) => Some(lines.clone()),
        _ => None,
    }
}

/// for ops that need no root: nothing to ask.
fn no_root(_: &str) -> Result<String> {
    bail!("this operation needs no password")
}

/// the server's clock; asks for nothing.
pub fn read(session: &Session) -> Result<ClockState> {
    let pick = |r: &Reply| match r {
        Reply::Clock(state) => Some(state.clone()),
        _ => None,
    };
    if let Some(state) = via_bridge(session, &Op::ClockState, &mut no_root, pick)? {
        return Ok(state);
    }
    let script = render(READ, &[]);
    parse_state(&session.capture(&["sh".into(), "-c".into(), script])?)
}

/// the time zones the server knows, sorted as it lists them.
pub fn zones(session: &Session) -> Result<Vec<String>> {
    let pick = |r: &Reply| match r {
        Reply::Names(zones) => Some(zones.clone()),
        _ => None,
    };
    if let Some(zones) = via_bridge(session, &Op::ClockZones, &mut no_root, pick)? {
        return Ok(zones);
    }
    let out = session.capture(&["sh".into(), "-c".into(), ZONES.into()])?;
    let zones: Vec<String> = out
        .lines()
        .map(str::trim)
        .filter(|z| valid_zone(z))
        .map(str::to_string)
        .collect();
    if zones.is_empty() {
        bail!("the server could not list its time zones");
    }
    Ok(zones)
}

/// what `zone` looks like on the server right now.
pub fn zone_now(session: &Session, zone: &str) -> Result<ZoneNow> {
    if !valid_zone(zone) {
        bail!("{zone} is not a time zone name");
    }
    let op = Op::ClockZoneNow {
        zone: zone.to_string(),
    };
    let pick = |r: &Reply| match r {
        Reply::ZoneNow(now) => Some(now.clone()),
        _ => None,
    };
    if let Some(now) = via_bridge(session, &op, &mut no_root, pick)? {
        return Ok(now);
    }
    let script = render(ZONE_NOW, &[("ZONE", zone)]);
    let out = session.capture(&["sh".into(), "-c".into(), script])?;
    let (offset, abbrev) = out.trim().split_once(' ').unwrap_or((out.trim(), ""));
    Ok(ZoneNow {
        offset_secs: parse_offset(offset).context("the server gave no offset")?,
        abbrev: abbrev.to_string(),
    })
}

fn root(session: &Session, script: String, ask_password: AskPassword) -> Result<String> {
    let admin = config::admin(session)?;
    capture_as_root(session, &admin, script, ask_password)
}

fn set_zone_script(zone: &str) -> Result<String> {
    if !valid_zone(zone) {
        bail!("{zone} is not a time zone name");
    }
    Ok(render(SET_ZONE, &[("ZONE", zone)]))
}

fn set_time_script(time: &str) -> Result<String> {
    Ok(render(SET_TIME, &[("TIME", &valid_time(time)?)]))
}

fn set_ntp_script(ntp: &NtpChange) -> Result<String> {
    if ntp.servers.len() > 8 || ntp.servers.iter().any(|s| !valid_server(s)) {
        bail!("the time servers must be host names or addresses, at most eight");
    }
    Ok(render(
        SET_NTP,
        &[
            ("ENABLED", if ntp.enabled { "true" } else { "false" }),
            ("SERVERS", &ntp.servers.join(" ")),
        ],
    ))
}

/// make the changes on the server, internet time first (turning it on stops manual time), then
/// the time, then the zone. returns what the server said.
pub fn apply(session: &Session, change: &Change, ask_password: AskPassword) -> Result<Vec<String>> {
    let ntp_on = change.ntp.as_ref().is_some_and(|n| n.enabled);
    if change.time.is_some() && ntp_on {
        bail!("a time can only be set while automatic synchronization is off");
    }
    let op = Op::ClockApply {
        change: change.clone(),
    };
    if let Some(lines) = via_bridge(session, &op, ask_password, lines_of)? {
        return Ok(lines);
    }
    // check everything before anything changes
    let ntp = change.ntp.as_ref().map(set_ntp_script).transpose()?;
    let time = change.time.as_deref().map(set_time_script).transpose()?;
    let zone = change
        .timezone
        .as_deref()
        .map(set_zone_script)
        .transpose()?;
    let mut said = Vec::new();
    for script in [ntp, time, zone].into_iter().flatten() {
        let out = root(session, script, ask_password)?;
        said.extend(out.lines().map(str::to_string));
    }
    Ok(said)
}

/// ask the time service to synchronize now and say how it went.
pub fn update_now(session: &Session, ask_password: AskPassword) -> Result<Vec<String>> {
    // one question for the bridge attempt and the fallback together
    let mut ask = crate::users::cached(ask_password);
    if let Some(lines) = via_bridge(session, &Op::ClockUpdateNow, &mut ask, lines_of)? {
        return Ok(lines);
    }
    let out = root(session, render(UPDATE_NOW, &[]), &mut ask)?;
    Ok(out.lines().map(str::to_string).collect())
}

/// make `change` on the server and keep its zone and ntp parts in the settings, so a later apply
/// does not undo them. the change itself comes first and is reported as done; keeping it is
/// second, only when lanlord is installed, and a failure there is a warning (`Stored::Failed`),
/// never an error that hides the change.
pub fn apply_stored(
    session: &Session,
    host: &str,
    identity: Option<&Identity>,
    change: &Change,
    ask_password: AskPassword,
) -> Result<(Vec<String>, Stored)> {
    let mut ask = crate::users::cached(ask_password);
    let said = apply(session, change, &mut ask)?;
    if change.timezone.is_none() && change.ntp.is_none() {
        return Ok((said, Stored::Nothing));
    }
    match config::is_installed(session) {
        Ok(true) => {}
        Ok(false) => return Ok((said, Stored::NotInstalled)),
        Err(err) => return Ok((said, Stored::Failed(err.to_string()))),
    }
    let Some(identity) = identity else {
        return Ok((said, Stored::NoKey));
    };
    match keep(session, host, identity, change, &mut ask) {
        Ok(()) => {
            emit(Kind::Info, format!("kept the clock settings for {host}"));
            Ok((said, Stored::Yes))
        }
        Err(err) => Ok((said, Stored::Failed(err.to_string()))),
    }
}

fn keep(
    session: &Session,
    host: &str,
    identity: &Identity,
    change: &Change,
    ask: AskPassword,
) -> Result<()> {
    let stored = settings::load_document_with(session, identity, ask)?;
    let mut parsed = Settings::parse(&stored.plain)?;
    keep_change(change, &mut parsed);
    settings::save_document_with(session, host, identity, &parsed.text(), &[], ask)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOW: &str = "Timezone=Europe/Berlin\nLocalRTC=no\nCanNTP=yes\nNTP=yes\nNTPSynchronized=yes\nTimeUSec=Sun 2026-11-15 17:12:03 CET\n";

    fn sample(container: &str) -> String {
        format!(
            "{SHOW}Container={container}\nEpoch=1794759123\nOffset=+0100\nZone=Europe/Berlin\nSync=timesyncd\nServers=0.pool.ntp.org 1.pool.ntp.org\nStatus=Server: 1.2.3.4 (0.pool.ntp.org)\nStatus=Poll interval: 34min 8s\n"
        )
    }

    #[test]
    fn the_state_is_read_from_what_the_server_prints() {
        let state = parse_state(&sample("none")).unwrap();
        assert_eq!(state.timezone, "Europe/Berlin");
        assert_eq!(state.utc_secs, 1_794_759_123);
        assert_eq!(state.offset_secs, 3600);
        assert!(state.ntp && state.synchronized && !state.container);
        assert_eq!(state.sync, Sync::Timesyncd);
        assert_eq!(state.servers, ["0.pool.ntp.org", "1.pool.ntp.org"]);
        assert_eq!(state.status.len(), 2);
        assert!(parse_state(&sample("lxc")).unwrap().container);
    }

    #[test]
    fn the_local_time_and_offset_are_formatted() {
        let state = parse_state(&sample("none")).unwrap();
        assert_eq!(state.local_text(0), "2026-11-15 17:12:03");
        assert_eq!(state.local_text(60), "2026-11-15 17:13:03");
        assert_eq!(local_text(0, 19_800), "1970-01-01 05:30:00");
        assert_eq!(offset_text(3600), "+01:00");
        assert_eq!(offset_text(-(5 * 3600 + 30 * 60)), "-05:30");
        assert_eq!(offset_text(0), "+00:00");
    }

    #[test]
    fn the_description_says_what_matters() {
        let text = describe(&parse_state(&sample("none")).unwrap());
        assert!(text.contains("time zone:     Europe/Berlin"), "{text}");
        assert!(text.contains("2026-11-15 17:12:03 (UTC+01:00)"), "{text}");
        assert!(
            text.contains("on, synchronized (systemd-timesyncd)"),
            "{text}"
        );
        assert!(text.contains("0.pool.ntp.org 1.pool.ntp.org"), "{text}");
        assert!(!text.contains("container"));
        let lxc = describe(&parse_state("Container=lxc\nEpoch=0\nNTP=no\n").unwrap());
        assert!(
            lxc.contains("internet time: off (no time service running)"),
            "{lxc}"
        );
        assert!(lxc.contains("This server is a container"), "{lxc}");
        let waiting = describe(&parse_state("Epoch=0\nNTP=yes\nSync=chrony\n").unwrap());
        assert!(
            waiting.contains("on, not synchronized yet (chrony)"),
            "{waiting}"
        );
    }

    #[test]
    fn a_missing_zone_falls_back_and_a_missing_time_is_an_error() {
        let text =
            "Container=lxc\nEpoch=100\nOffset=-0530\nZone=America/St_Johns\nSync=chrony\nNTP=no\n";
        let state = parse_state(text).unwrap();
        assert_eq!(state.timezone, "America/St_Johns");
        assert_eq!(state.offset_secs, -(5 * 3600 + 30 * 60));
        assert_eq!((state.sync, state.ntp), (Sync::Chrony, false));
        assert_eq!(parse_state("Epoch=1\n").unwrap().timezone, "UTC");
        assert!(parse_state("Timezone=UTC\n").is_err());
        assert!(parse_state("Epoch=soon\n").is_err());
        assert_eq!(parse_offset("+05"), None);
        assert_eq!(parse_offset("0100"), None);
    }

    #[test]
    fn zones_servers_and_times_are_checked_strictly() {
        for zone in [
            "UTC",
            "Europe/Berlin",
            "America/Argentina/Buenos_Aires",
            "Etc/GMT+5",
        ] {
            assert!(valid_zone(zone), "{zone}");
        }
        for zone in [
            "",
            "/etc/passwd",
            "../x",
            "a//b",
            "a b",
            "a;b",
            "a'b",
            "x/./y",
            &"a".repeat(65),
        ] {
            assert!(!valid_zone(zone), "{zone}");
        }
        for server in ["pool.ntp.org", "10.0.0.1", "ntp1", "2001:db8::1"] {
            assert!(valid_server(server), "{server}");
        }
        for server in [
            "", "a b", "-x.org", "x-.org", "a..b", "a;b", "$(x)", "a'b", ":::",
        ] {
            assert!(!valid_server(server), "{server}");
        }
        assert_eq!(parse_servers("a.org, b.org  c.org").unwrap().len(), 3);
        assert!(parse_servers("a.org bad;name").is_err());
        assert!(parse_servers(&"a.org ".repeat(9)).is_err());
        assert!(parse_servers("").unwrap().is_empty());
        assert_eq!(
            valid_time(" 2026-11-15 17:30:00 ").unwrap(),
            "2026-11-15 17:30:00"
        );
        for time in [
            "2026-13-01 00:00:00",
            "2026-02-30 00:00:00",
            "tomorrow",
            "2026-11-15",
        ] {
            assert!(valid_time(time).is_err(), "{time}");
        }
    }

    #[test]
    fn every_script_is_valid_sh() {
        let ntp = NtpChange {
            enabled: true,
            servers: vec!["pool.ntp.org".into()],
        };
        let scripts = [
            render(READ, &[]),
            render(ZONE_NOW, &[("ZONE", "Europe/Berlin")]),
            set_zone_script("Europe/Berlin").unwrap(),
            set_time_script("2026-11-15 17:30:00").unwrap(),
            set_ntp_script(&ntp).unwrap(),
            render(UPDATE_NOW, &[]),
            ZONES.to_string(),
        ];
        for script in scripts {
            assert!(
                !script.contains('@'),
                "an unfilled placeholder in\n{script}"
            );
            let mut child = std::process::Command::new("sh")
                .arg("-n")
                .stdin(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            std::io::Write::write_all(&mut child.stdin.take().unwrap(), script.as_bytes()).unwrap();
            let out = child.wait_with_output().unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    #[test]
    fn bad_input_never_reaches_a_script() {
        assert!(set_zone_script("x'; rm -rf /; '").is_err());
        assert!(set_time_script("now").is_err());
        let evil = NtpChange {
            enabled: true,
            servers: vec!["a.org'; id; '".into()],
        };
        assert!(set_ntp_script(&evil).is_err());
        let many = NtpChange {
            enabled: true,
            servers: vec!["a.org".to_string(); 9],
        };
        assert!(set_ntp_script(&many).is_err());
    }

    #[cfg(unix)]
    mod scripts {
        use super::*;
        use std::path::{Path, PathBuf};
        use std::process::Command;

        /// shims for the commands the scripts call, in a temp directory.
        struct Fake {
            dir: PathBuf,
        }

        impl Fake {
            fn new(name: &str, virt: &str, ntp: &str) -> Self {
                let dir = std::env::temp_dir()
                    .join(format!("lanlord-clock-{}-{name}", std::process::id()));
                let _ = std::fs::remove_dir_all(&dir);
                std::fs::create_dir_all(dir.join("bin")).unwrap();
                let log = dir.join("log");
                let shims = [
                    ("systemd-detect-virt", format!("echo {virt}")),
                    (
                        "timedatectl",
                        format!(
                            r#"case "$1" in
  list-timezones) printf 'Europe/Berlin\nUTC\n' ;;
  show) echo {ntp} ;;
  *) echo "timedatectl $*" >> {} ;;
esac"#,
                            log.display()
                        ),
                    ),
                    (
                        "systemctl",
                        format!("echo \"systemctl $*\" >> {}", log.display()),
                    ),
                ];
                for (name, body) in shims {
                    let path = dir.join("bin").join(name);
                    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
                    Command::new("chmod").arg("+x").arg(&path).status().unwrap();
                }
                Self { dir }
            }

            fn run(&self, script: &str) -> (i32, String, String) {
                let path = format!(
                    "{}:{}",
                    self.dir.join("bin").display(),
                    std::env::var("PATH").unwrap()
                );
                let out = Command::new("sh")
                    .arg("-c")
                    .arg(script)
                    .env("PATH", path)
                    .output()
                    .unwrap();
                (
                    out.status.code().unwrap_or(-1),
                    String::from_utf8_lossy(&out.stdout).into_owned(),
                    String::from_utf8_lossy(&out.stderr).into_owned(),
                )
            }

            fn file(&self) -> PathBuf {
                self.dir.join("etc/timesyncd.conf.d/lanlord.conf")
            }

            fn log(&self) -> String {
                std::fs::read_to_string(self.dir.join("log")).unwrap_or_default()
            }
        }

        impl Drop for Fake {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }

        fn ntp_script(fake: &Fake, enabled: bool, servers: &[&str]) -> String {
            let ntp = NtpChange {
                enabled,
                servers: servers.iter().map(|s| s.to_string()).collect(),
            };
            let template = set_ntp_script(&ntp).unwrap();
            template.replace(TIMESYNCD_FILE, fake.file().to_str().unwrap())
        }

        #[test]
        fn the_time_zone_is_set_through_timedatectl() {
            let fake = Fake::new("zone", "none", "NTP=no");
            let (code, out, _) = fake.run(&set_zone_script("Europe/Berlin").unwrap());
            assert_eq!(code, 0);
            assert!(out.contains("time zone is Europe/Berlin"));
            assert!(fake
                .log()
                .contains("timedatectl set-timezone Europe/Berlin"));
            let (code, _, err) = fake.run(&set_zone_script("Nowhere/Land").unwrap());
            assert_eq!(code, 2, "{err}");
            assert!(err.contains("unknown time zone"));
        }

        #[test]
        fn the_time_can_only_be_set_when_ntp_is_off_and_not_in_a_container() {
            let off = Fake::new("time-off", "none", "NTP=no");
            let (code, out, _) = off.run(&set_time_script("2026-11-15 17:30:00").unwrap());
            assert_eq!(code, 0, "{out}");
            assert!(off
                .log()
                .contains("timedatectl set-time 2026-11-15 17:30:00"));
            let on = Fake::new("time-on", "none", "yes");
            let (code, _, err) = on.run(&set_time_script("2026-11-15 17:30:00").unwrap());
            assert_eq!(code, 3, "{err}");
            assert!(on.log().is_empty());
            let lxc = Fake::new("time-lxc", "lxc", "NTP=no");
            let (code, _, err) = lxc.run(&set_time_script("2026-11-15 17:30:00").unwrap());
            assert_eq!(code, 4, "{err}");
            assert!(err.contains("container"));
        }

        #[test]
        fn ntp_servers_are_written_to_the_drop_in_and_the_service_restarted() {
            let fake = Fake::new("ntp", "none", "NTP=no");
            let (code, out, err) =
                fake.run(&ntp_script(&fake, true, &["a.pool.org", "b.pool.org"]));
            assert_eq!(code, 0, "{err}");
            assert!(out.contains("internet time is true"));
            let written = std::fs::read_to_string(fake.file()).unwrap();
            assert!(
                written.contains("[Time]\nNTP=a.pool.org b.pool.org\n"),
                "{written}"
            );
            let log = fake.log();
            assert!(log.contains("timedatectl set-ntp true"), "{log}");
            assert!(log.contains("systemctl restart systemd-timesyncd"), "{log}");
            // switching it off with no servers removes the drop-in again
            let (code, _, _) = fake.run(&ntp_script(&fake, false, &[]));
            assert_eq!(code, 0);
            assert!(!Path::new(&fake.file()).exists());
            assert!(fake.log().contains("timedatectl set-ntp false"));
        }

        #[test]
        fn a_container_refuses_ntp_and_updates_but_keeps_the_zone() {
            let fake = Fake::new("lxc", "lxc", "NTP=no");
            let (code, _, err) = fake.run(&ntp_script(&fake, true, &["a.org"]));
            assert_eq!(code, 4, "{err}");
            assert!(!fake.file().exists());
            let (code, _, _) = fake.run(&render(UPDATE_NOW, &[]));
            assert_eq!(code, 4);
            let (code, _, _) = fake.run(&set_zone_script("UTC").unwrap());
            assert_eq!(code, 0);
        }

        #[test]
        fn updating_now_restarts_the_time_service() {
            let fake = Fake::new("update", "none", "NTP=yes");
            let (code, _, err) = fake.run(&render(UPDATE_NOW, &[]));
            assert_eq!(code, 0, "{err}");
            assert!(fake.log().contains("systemctl restart systemd-timesyncd"));
        }
    }

    // the fake gateway is a shell script, so these run where there is a shell
    #[cfg(unix)]
    mod session {
        #[cfg(unix)]
        #[test]
        fn a_reply_of_the_wrong_kind_is_an_error_not_a_fallback() {
            let session = Session::fake(
                r#"for a; do last=$a; done
case "$last" in
  "id -un") echo root ;;
  *) echo '{"proto":1,"version":"t"}'; read req
     echo '{"kind":"reply","id":1,"reply":{"reply":"lines","value":["done"]}}' ;;
esac"#,
            );
            session
                .no_bridge
                .store(false, std::sync::atomic::Ordering::Relaxed);
            let err = zones(&session).unwrap_err().to_string();
            assert!(err.contains("the bridge answered"), "{err}");
        }

        use super::*;
        use crate::testgw::{identity, no_password, Gateway};

        /// a gateway whose scripts print `reply`, recording what it was sent.
        fn serving(reply: &str, record: &std::path::Path) -> Session {
            Session::fake(&format!(
                r#"for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sudo -n true") exit 0 ;;
  *) cat >> '{record}'; printf '%s\n' '{reply}' ;;
esac"#,
                record = record.display()
            ))
        }

        #[test]
        fn read_zones_and_zone_now_parse_the_replies() {
            let record =
                std::env::temp_dir().join(format!("lanlord-clock-rec-{}", std::process::id()));
            let state = serving(&sample("none"), &record);
            assert_eq!(read(&state).unwrap().timezone, "Europe/Berlin");
            let zones = serving("Africa/Abidjan\nEurope/Berlin\nnot a zone\n", &record);
            assert_eq!(
                super::zones(&zones).unwrap(),
                ["Africa/Abidjan", "Europe/Berlin"]
            );
            assert!(super::zones(&serving("", &record)).is_err());
            let now = serving("+0530 IST", &record);
            assert_eq!(
                zone_now(&now, "Asia/Kolkata").unwrap(),
                ZoneNow {
                    offset_secs: 19_800,
                    abbrev: "IST".into()
                }
            );
            assert!(zone_now(&now, "bad zone").is_err());
            assert!(zone_now(&serving("nonsense", &record), "UTC").is_err());
            let _ = std::fs::remove_file(record);
        }

        #[test]
        fn apply_runs_ntp_then_time_then_zone_and_refuses_a_time_with_ntp_on() {
            let record =
                std::env::temp_dir().join(format!("lanlord-clock-ord-{}", std::process::id()));
            let _ = std::fs::remove_file(&record);
            let session = serving("ok", &record);
            let change = Change {
                timezone: Some("UTC".into()),
                time: Some("2026-11-15 17:30:00".into()),
                ntp: Some(NtpChange {
                    enabled: false,
                    servers: vec!["a.org".into()],
                }),
            };
            let said = apply(&session, &change, &mut no_password).unwrap();
            assert_eq!(said.len(), 3);
            let sent = std::fs::read_to_string(&record).unwrap();
            let (ntp, time, zone) = (
                sent.find("set-ntp").unwrap(),
                sent.find("set-time").unwrap(),
                sent.find("set-timezone").unwrap(),
            );
            assert!(ntp < time && time < zone, "{sent}");
            let _ = std::fs::remove_file(&record);
            let bad = Change {
                time: Some("2026-11-15 17:30:00".into()),
                ntp: Some(NtpChange {
                    enabled: true,
                    servers: vec![],
                }),
                ..Change::default()
            };
            assert!(apply(&session, &bad, &mut no_password).is_err());
            assert!(
                !record.exists(),
                "nothing may run when the change is refused"
            );
            let none = serving("pool", &record);
            assert!(update_now(&none, &mut no_password).is_ok());
            let _ = std::fs::remove_file(record);
        }

        /// a server where the zone script works but writing the settings fails like a missing sudo.
        fn broken_settings() -> Session {
            Session::fake(
                r#"for a; do last=$a; done
case "$last" in
  "id -un") echo root ;;
  "sudo -n true") exit 0 ;;
  "sh -c "*"getent passwd lanlord"*) echo yes ;;
  "sh -c "*) exit 0 ;;
  *)
    input=$(cat)
    case "$input" in
      *"tee /opt/lanlord/config/settings.yml"*) echo "sudo: command not found" >&2; exit 127 ;;
      *"timedatectl set-timezone"*) echo "zone changed" ;;
    esac ;;
esac"#,
            )
        }

        fn zone(name: &str) -> Change {
            Change {
                timezone: Some(name.into()),
                ..Change::default()
            }
        }

        #[test]
        fn the_zone_is_changed_even_when_keeping_it_fails_with_status_127() {
            let id = identity("clock-127");
            let (said, stored) = apply_stored(
                &broken_settings(),
                "gw",
                Some(&id),
                &zone("Europe/Berlin"),
                &mut no_password,
            )
            .expect("the change itself must not be an error");
            assert!(said.iter().any(|l| l.contains("zone changed")), "{said:?}");
            match stored {
                Stored::Failed(why) => assert!(why.contains("status 127"), "{why}"),
                other => panic!("expected a warning, got {other:?}"),
            }
        }

        #[test]
        fn a_server_without_lanlord_gets_the_change_and_a_plain_note() {
            let (gw, id) = (Gateway::not_installed("clock-bare"), identity("clock-bare"));
            let (_, stored) =
                apply_stored(&gw.session, "gw", Some(&id), &zone("UTC"), &mut no_password).unwrap();
            assert_eq!(stored, Stored::NotInstalled);
            assert!(
                gw.plain().is_none(),
                "nothing may be written without lanlord"
            );
            // a time alone has nothing to keep, installed or not
            let time = Change {
                time: Some("2026-11-15 17:30:00".into()),
                ..Change::default()
            };
            let (_, stored) =
                apply_stored(&gw.session, "gw", Some(&id), &time, &mut no_password).unwrap();
            assert_eq!(stored, Stored::Nothing);
        }

        #[test]
        fn the_zone_and_ntp_are_kept_in_the_gateway_settings() {
            let (gw, id) = (Gateway::new("clock"), identity("clock"));
            gw.seed_sealed(
                &id,
                "gateway:\n  motd: \"hi\\n\"\nnt4-domain:\n  mode: domain\n",
            );
            let change = Change {
                timezone: Some("Europe/Berlin".into()),
                time: None,
                ntp: Some(NtpChange {
                    enabled: true,
                    servers: vec!["a.org".into(), "b.org".into()],
                }),
            };
            let (_, stored) =
                apply_stored(&gw.session, "gw", Some(&id), &change, &mut no_password).unwrap();
            assert_eq!(stored, Stored::Yes);
            let kept = Kept::from_settings(&Settings::parse(&gw.plain().unwrap()).unwrap());
            assert_eq!(kept.timezone.as_deref(), Some("Europe/Berlin"));
            assert_eq!(
                kept.ntp,
                Some(vec!["a.org".to_string(), "b.org".to_string()])
            );
            assert_eq!(kept.ntp_enabled, Some(true));
            assert!(gw.plain().unwrap().contains("motd"), "other keys stay");
            // a time alone is not kept, and with no key nothing is stored
            let time = Change {
                time: Some("2026-11-15 17:30:00".into()),
                ..Change::default()
            };
            let (_, stored) =
                apply_stored(&gw.session, "gw", Some(&id), &time, &mut no_password).unwrap();
            assert_eq!(stored, Stored::Nothing);
            let zone = Change {
                timezone: Some("UTC".into()),
                ..Change::default()
            };
            let (_, stored) =
                apply_stored(&gw.session, "gw", None, &zone, &mut no_password).unwrap();
            assert_eq!(stored, Stored::NoKey);
            assert!(
                gw.plain().unwrap().contains("Europe/Berlin"),
                "no key, no change"
            );
        }

        #[test]
        fn a_server_that_refuses_keeps_the_settings_as_they_were() {
            let (gw, id) = (Gateway::new("clock-fail"), identity("clock-fail"));
            gw.seed_sealed(&id, "gateway:\n  motd: x\n");
            let failing = Session::fake(
                r#"for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sudo -n true") exit 0 ;;
  *) cat >/dev/null; echo refused >&2; exit 4 ;;
esac"#,
            );
            let change = Change {
                timezone: Some("UTC".into()),
                ..Change::default()
            };
            assert!(apply_stored(&failing, "gw", Some(&id), &change, &mut no_password).is_err());
            assert!(gw.plain().is_none());
        }

        #[test]
        fn kept_settings_without_a_gateway_section_are_empty() {
            let settings = Settings::parse("docker: {}\n").unwrap();
            assert_eq!(Kept::from_settings(&settings), Kept::default());
        }
    }
}
