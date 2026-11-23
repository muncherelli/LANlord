//! detecting iis on a domain machine, and the cache of what was found: one json file for every
//! machine, written whole so a reader never sees half of it.

use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Result};

use crate::engine::host::Host;
pub use lanlord_model::hosting::*;

/// where the facts of every machine are kept.
pub const FACTS_DIR: &str = "/var/lib/lanlord-operations/hosting";

fn path(dir: &str) -> String {
    format!("{dir}/facts.json")
}

/// the cache key of a machine: netbios names do not differ by case.
fn key(machine: &str) -> String {
    machine.to_ascii_uppercase()
}

fn load(host: &dyn Host, dir: &str) -> Result<BTreeMap<String, HostingFacts>> {
    match host.read(&path(dir))? {
        Some(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| anyhow!("the hosting facts are not readable: {e}")),
        None => Ok(BTreeMap::new()),
    }
}

/// the cached facts of `machine`, or `None` when it was never detected.
pub fn cached(host: &dyn Host, dir: &str, machine: &str) -> Result<Option<HostingFacts>> {
    Ok(load(host, dir)?.remove(&key(machine)))
}

/// keep `facts`, replacing the older ones of the same machine.
pub fn store(host: &dyn Host, dir: &str, facts: &HostingFacts) -> Result<()> {
    let mut all = load(host, dir)?;
    all.insert(key(&facts.machine), facts.clone());
    host.mkdir(dir, 0o700)?;
    host.write(
        &path(dir),
        serde_json::to_string_pretty(&all)?.as_bytes(),
        0o600,
    )
}

/// the facts as lines of text for a terminal: the services, then one line per site.
pub fn facts_lines(facts: &HostingFacts) -> Vec<String> {
    let services: Vec<String> = facts
        .services
        .iter()
        .map(|s| {
            format!(
                "{} {}",
                s.name,
                if s.running { "running" } else { "stopped" }
            )
        })
        .collect();
    let mut lines = vec![format!("{}: {}", facts.machine, services.join(", "))];
    if facts.sites.is_empty() && !facts.iis_seen() {
        lines.push("IIS not present, install the NT4 Option Pack".into());
    }
    for site in &facts.sites {
        let bindings: Vec<String> = site
            .bindings
            .iter()
            .map(|b| format!("{}:{}:{}", b.ip, b.port, b.host))
            .collect();
        lines.push(format!(
            "{} {}\t{}\t{}\t{}\t{}{}",
            site.kind.label(),
            site.id,
            site.state,
            site.comment,
            bindings.join(" "),
            site.home,
            if site.has_password {
                "\thas a password"
            } else {
                ""
            }
        ));
    }
    lines
}

/// what one run on the machine did.
pub struct Ran {
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub stdout: String,
}

/// detect iis on `machine`: `run` runs a preset of [`lanlord_model::nt4_exec`] on it. the service
/// listing may end non-zero (`findstr` says 1 when nothing matched); the dump may not.
pub fn detect(
    machine: &str,
    now: u64,
    run: &mut dyn FnMut(&str) -> Result<Ran>,
) -> Result<HostingFacts> {
    let services = run("services-status")?;
    if services.timed_out {
        bail!("{machine} did not answer in time");
    }
    let dump = run("iis-dump")?;
    if dump.timed_out || dump.exit_code != Some(0) {
        bail!("the IIS dump on {machine} did not finish; is the NT4 Option Pack installed?");
    }
    Ok(facts(machine, now, &services.stdout, &dump.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::host::FakeHost;

    fn ran(code: i32, out: &str) -> Ran {
        Ran {
            exit_code: Some(code),
            timed_out: false,
            stdout: out.into(),
        }
    }

    #[test]
    fn facts_are_kept_per_machine_and_read_back() {
        let host = FakeHost::new();
        assert_eq!(cached(&host, "/h", "web1").unwrap(), None);
        let one = facts(
            "WEB1",
            5,
            "   IIS Admin Service",
            "W3SVC/1/ServerComment=A\n",
        );
        let two = facts("WEB2", 6, "", "");
        store(&host, "/h", &one).unwrap();
        store(&host, "/h", &two).unwrap();
        assert_eq!(cached(&host, "/h", "web1").unwrap(), Some(one));
        assert_eq!(cached(&host, "/h", "WEB2").unwrap(), Some(two));
        // the new detection replaces the old one
        let again = facts("WEB1", 9, "", "");
        store(&host, "/h", &again).unwrap();
        assert_eq!(cached(&host, "/h", "WEB1").unwrap().unwrap().detected_at, 9);
        let files = host.files.borrow();
        let (text, mode) = files.get("/h/facts.json").unwrap();
        assert_eq!(*mode, 0o600);
        assert!(std::str::from_utf8(text).unwrap().contains("WEB2"));
    }

    #[test]
    fn the_lines_name_services_and_sites() {
        let found = facts(
            "WEB1",
            1,
            "   IIS Admin Service",
            "W3SVC/1/ServerComment=Intranet\nW3SVC/1/ServerBindings[0]=:80:a.example.lan\nW3SVC/1/ServerState=2\n",
        );
        let lines = facts_lines(&found);
        assert!(
            lines[0].starts_with("WEB1: IISADMIN running, W3SVC stopped"),
            "{lines:?}"
        );
        assert_eq!(lines[1], "web 1\tstarted\tIntranet\t:80:a.example.lan\t");
        let none = facts_lines(&facts("WEB2", 1, "", ""));
        assert!(none[1].starts_with("IIS not present"));
    }

    #[test]
    fn a_damaged_cache_is_an_error_not_an_empty_one() {
        let host = FakeHost::new();
        host.write("/h/facts.json", b"{", 0o600).unwrap();
        assert!(cached(&host, "/h", "WEB1").is_err());
        assert!(store(&host, "/h", &facts("WEB1", 1, "", "")).is_err());
    }

    #[test]
    fn detect_runs_the_listing_then_the_dump() {
        let mut seen = Vec::new();
        let found = detect("WEB1", 7, &mut |preset| {
            seen.push(preset.to_string());
            Ok(match preset {
                "services-status" => ran(0, "   World Wide Web Publishing Service\r\n"),
                _ => ran(
                    0,
                    "W3SVC/1/ServerComment=Intranet\r\nW3SVC/1/ServerState=2\r\n",
                ),
            })
        })
        .unwrap();
        assert_eq!(seen, ["services-status", "iis-dump"]);
        assert_eq!(found.sites.len(), 1);
        assert_eq!(found.sites[0].state, "started");
        assert!(found
            .services
            .iter()
            .any(|s| s.name == "W3SVC" && s.running));
    }

    #[test]
    fn detect_tolerates_an_empty_listing_but_not_a_failed_dump() {
        assert!(detect("WEB1", 1, &mut |preset| Ok(if preset == "iis-dump" {
            ran(0, "W3SVC=absent\n")
        } else {
            ran(1, "")
        }))
        .is_ok());
        assert!(detect("WEB1", 1, &mut |_| Ok(ran(1, ""))).is_err());
        let timed_out = || Ran {
            exit_code: None,
            timed_out: true,
            stdout: String::new(),
        };
        assert!(detect("WEB1", 1, &mut |_| Ok(timed_out())).is_err());
    }
}
