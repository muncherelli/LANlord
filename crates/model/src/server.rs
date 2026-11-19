//! what the signed-in server is: its name, system, uptime and network interfaces, from one
//! read-only script that needs no sudo. the My Server and Networks windows show it.

use anyhow::{bail, Result};

use crate::routes;

/// one network interface and what it is doing.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Iface {
    pub name: String,
    pub mtu: u32,
    /// the kernel's state: UP, DOWN or UNKNOWN (loopback and some virtual links).
    pub state: String,
    /// the ipv4 addresses with their prefix, like `192.0.2.10/24`.
    pub addresses: Vec<String>,
    /// the default route leaves through this interface.
    pub default_route: bool,
}

impl Iface {
    pub fn loopback(&self) -> bool {
        self.name == "lo"
    }

    /// the state as a word for a list: Connected, Disconnected or Unknown.
    pub fn link(&self) -> &'static str {
        match self.state.as_str() {
            "UP" => "Connected",
            "DOWN" | "LOWERLAYERDOWN" => "Disconnected",
            _ => "Unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServerFacts {
    pub hostname: String,
    pub os: String,
    pub kernel: String,
    pub uptime_secs: u64,
    pub user: String,
    pub is_root: bool,
    /// the processor model, like `Intel(R) Core(TM) i7-4790K CPU @ 4.00GHz`; empty when unknown.
    #[serde(default)]
    pub cpu_model: String,
    /// the processors the server can use: the host's, or fewer when a container limits them.
    pub cpus: u32,
    /// the physical processors (sockets) the host has; 0 when the server did not say.
    #[serde(default)]
    pub sockets: u16,
    /// the physical cores in total, as opposed to `cpus`, which counts threads; 0 when unknown.
    #[serde(default)]
    pub cores: u16,
    #[serde(default)]
    pub cpus_limited: bool,
    /// the memory the server can use: the host's, or less when a container limits it.
    pub mem_total_mb: u64,
    pub mem_available_mb: u64,
    #[serde(default)]
    pub mem_limited: bool,
    /// lanlord's directory and service account are both there.
    pub lanlord_installed: bool,
    pub lanlord_version: Option<String>,
    pub last_applied: Option<String>,
    pub interfaces: Vec<Iface>,
}

/// how many cpus a cpuset list like `0-1,4` names.
fn cpuset_count(list: &str) -> Option<u32> {
    let mut count = 0u32;
    for part in list.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let named = match part.split_once('-') {
            Some((a, b)) => {
                let (a, b) = (a.parse::<u32>().ok()?, b.parse::<u32>().ok()?);
                b.checked_sub(a)?.checked_add(1)?
            }
            None => part.parse::<u32>().map(|_| 1).ok()?,
        };
        count = count.checked_add(named)?;
    }
    (count > 0).then_some(count)
}

/// how many cpus a cgroup `cpu.max` (`quota period` or `max period`) allows, rounded up.
fn quota_cpus(text: &str) -> Option<u32> {
    let mut words = text.split_whitespace();
    let quota = words.next()?.parse::<u64>().ok()?;
    let period = words.next()?.parse::<u64>().ok().filter(|p| *p > 0)?;
    u32::try_from(quota.div_ceil(period).max(1)).ok()
}

/// `raw` (an rfc 3339 time, as ansible writes it) in `zone`, like `11/16/2026 4:05:09 PM`;
/// `raw` itself when it does not parse.
pub fn format_applied<Tz: chrono::TimeZone>(raw: &str, zone: &Tz) -> String
where
    Tz::Offset: core::fmt::Display,
{
    match chrono::DateTime::parse_from_rfc3339(raw.trim()) {
        Ok(time) => time
            .with_timezone(zone)
            .format("%-m/%-d/%Y %-I:%M:%S %p")
            .to_string(),
        Err(_) => raw.to_string(),
    }
}

impl ServerFacts {
    /// the rows that describe the processors, in order: `Processors` when there is more than
    /// one socket, `Cores` when there is more than one, and `Threads` when they outnumber the
    /// cores. a container's limit is only known in threads, so that is all it shows.
    pub fn cpu_rows(&self) -> Vec<(&'static str, String)> {
        let threads = self.cpus;
        if self.cpus_limited {
            return vec![("Threads", format!("{threads} (limited by container)"))];
        }
        // a server that did not say (an arm board, an old script) has as many cores as threads
        let cores = if self.cores == 0 {
            threads
        } else {
            u32::from(self.cores)
        };
        let mut rows = Vec::new();
        if self.sockets > 1 {
            rows.push(("Processors", self.sockets.to_string()));
        }
        if cores > 1 {
            rows.push(("Cores", cores.to_string()));
        }
        if threads > cores {
            rows.push(("Threads", threads.to_string()));
        }
        if rows.is_empty() {
            let one = if cores == 0 {
                "Unknown".to_string()
            } else {
                "1".to_string()
            };
            rows.push(("Cores", one));
        }
        rows
    }

    /// when lanlord was last applied, in this computer's time zone (the browser's, on the
    /// web); the text as the server wrote it when that is not a time.
    pub fn last_applied_local(&self) -> Option<String> {
        let raw = self.last_applied.as_deref()?;
        Some(format_applied(raw, &chrono::Local))
    }

    /// the memory as a line: what the server can use and how much of it is free.
    pub fn memory(&self) -> String {
        let limit = if self.mem_limited {
            " (limited by container)"
        } else {
            ""
        };
        format!(
            "{} MB, {} MB free{limit}",
            self.mem_total_mb, self.mem_available_mb
        )
    }

    /// the uptime as `3 days, 4 hours, 12 minutes`.
    pub fn uptime(&self) -> String {
        let (d, h, m) = (
            self.uptime_secs / 86_400,
            self.uptime_secs % 86_400 / 3_600,
            self.uptime_secs % 3_600 / 60,
        );
        let part = |n: u64, one: &str| format!("{n} {one}{}", if n == 1 { "" } else { "s" });
        match (d, h) {
            (0, 0) => part(m, "minute"),
            (0, _) => format!("{}, {}", part(h, "hour"), part(m, "minute")),
            _ => format!(
                "{}, {}, {}",
                part(d, "day"),
                part(h, "hour"),
                part(m, "minute")
            ),
        }
    }
}

fn field<'a>(lines: &[&'a str], key: &str) -> &'a str {
    lines
        .iter()
        .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
        .unwrap_or("")
        .trim()
}

fn number(lines: &[&str], key: &str) -> u64 {
    field(lines, key).parse().unwrap_or(0)
}

/// the words after `key` in `line`, like the number after `mtu`.
fn after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let mut words = line.split_whitespace();
    words.find(|w| *w == key)?;
    words.next()
}

#[doc(hidden)]
pub fn parse_links(text: &str) -> Vec<Iface> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            words.next()?; // the index, like `2:`
            let name = words.next()?.trim_end_matches(':');
            let name = name.split('@').next().unwrap_or(name);
            (!name.is_empty()).then(|| Iface {
                name: name.to_string(),
                mtu: after(line, "mtu").and_then(|m| m.parse().ok()).unwrap_or(0),
                state: after(line, "state").unwrap_or("UNKNOWN").to_string(),
                addresses: Vec::new(),
                default_route: false,
            })
        })
        .collect()
}

/// the facts in the output of the read script.
pub fn parse(output: &str) -> Result<ServerFacts> {
    let mut parts = output.split("---link");
    let head: Vec<&str> = parts.next().unwrap_or("").lines().collect();
    let Some(rest) = parts.next() else {
        bail!("the server did not answer with its facts");
    };
    let (links, rest) = rest.split_once("---addr").unwrap_or((rest, ""));
    let (addrs, route) = rest.split_once("---route").unwrap_or((rest, ""));
    let mut interfaces = parse_links(links);
    for line in addrs.lines() {
        let mut words = line.split_whitespace();
        words.next();
        let (Some(name), Some("inet"), Some(address)) = (words.next(), words.next(), words.next())
        else {
            continue;
        };
        if let Some(iface) = interfaces.iter_mut().find(|i| i.name == name) {
            iface.addresses.push(address.to_string());
        }
    }
    // `---route` holds the ipv4 defaults and, from newer scripts, `---route6` the ipv6 ones
    let (route4, route6) = route.split_once("---route6").unwrap_or((route, ""));
    let mut defaults = routes::from_text(route4, false);
    defaults.extend(routes::from_text(route6, true));
    if let Some(name) = routes::external(&defaults) {
        if let Some(iface) = interfaces.iter_mut().find(|i| i.name == name) {
            iface.default_route = true;
        }
    }
    // a container's limits, when they are lower than the host's
    let host_cpus = number(&head, "cpus") as u32;
    let cpu_limit = [
        quota_cpus(field(&head, "cgroup_cpu_max")),
        quota_cpus(field(&head, "cgroup_cpu_v1")),
        cpuset_count(field(&head, "cgroup_cpuset")),
    ]
    .into_iter()
    .flatten()
    .min();
    let cpus_limited = cpu_limit.is_some_and(|n| host_cpus > 0 && n < host_cpus);
    let host_mem = number(&head, "mem_total_kb") / 1024;
    let mut mem_total_mb = host_mem;
    let mut mem_available_mb = number(&head, "mem_available_kb") / 1024;
    // `max` (v2) is no limit and does not parse; a huge v1 number is not below the host's memory
    let mem_limit = field(&head, "cgroup_mem_max").parse::<u64>().ok();
    let mem_limited = mem_limit.is_some_and(|b| host_mem > 0 && b / 1_048_576 < host_mem);
    if let (true, Some(bytes)) = (mem_limited, mem_limit) {
        mem_total_mb = bytes / 1_048_576;
        let used = number(&head, "cgroup_mem_current") / 1_048_576;
        mem_available_mb = mem_available_mb.min(mem_total_mb.saturating_sub(used));
    }
    let version = field(&head, "version");
    let applied = field(&head, "applied");
    Ok(ServerFacts {
        hostname: field(&head, "hostname").to_string(),
        os: field(&head, "os").to_string(),
        kernel: field(&head, "kernel").to_string(),
        uptime_secs: number(&head, "uptime"),
        user: field(&head, "user").to_string(),
        is_root: field(&head, "uid") == "0",
        cpu_model: field(&head, "cpu_model").to_string(),
        cpus: match cpu_limit {
            Some(n) if cpus_limited => n,
            _ => host_cpus,
        },
        sockets: u16::try_from(number(&head, "sockets")).unwrap_or(0),
        cores: u16::try_from(number(&head, "cores")).unwrap_or(0),
        cpus_limited,
        mem_total_mb,
        mem_available_mb,
        mem_limited,
        lanlord_installed: field(&head, "installed") == "1",
        lanlord_version: (!version.is_empty()).then(|| version.to_string()),
        last_applied: (!applied.is_empty()).then(|| applied.to_string()),
        interfaces,
    })
}

/// made-up facts for trying the windows without a server (`LANLORD_FAKE_SERVER=1`).
pub fn demo() -> ServerFacts {
    parse(DEMO).expect("the demo facts parse")
}

const DEMO: &str = "hostname=demo\nos=Arch Linux\nkernel=Linux 6.12.7-arch1-1\nuptime=273900\nuser=root\nuid=0\ncpu_model=Intel(R) Core(TM) i7-4790K CPU @ 4.00GHz\ncpus=4\nsockets=1\ncores=2\nmem_total_kb=4194304\nmem_available_kb=3407872\ninstalled=1\nversion=0.1.0\napplied=2026-11-15T20:41:07-06:00\n---link\n1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536 qdisc noqueue state UNKNOWN mode DEFAULT group default qlen 1000\\    link/loopback 00:00:00:00:00:00 brd 00:00:00:00:00:00\n2: eth0@if48: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc noqueue state UP mode DEFAULT group default qlen 1000\\    link/ether 02:00:00:00:aa:01 brd ff:ff:ff:ff:ff:ff link-netnsid 0\n3: eth1@if49: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1450 qdisc noqueue state UP mode DEFAULT group default qlen 1000\\    link/ether 02:00:00:00:aa:02 brd ff:ff:ff:ff:ff:ff link-netnsid 0\n---addr\n1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever preferred_lft forever\n2: eth0    inet 192.0.2.10/24 metric 1024 brd 192.0.2.255 scope global dynamic eth0\\       valid_lft 3600sec preferred_lft 3600sec\n3: eth1    inet 172.16.120.1/22 brd 172.16.123.255 scope global eth1\\       valid_lft forever preferred_lft forever\n---route\ndefault via 192.0.2.1 dev eth0 proto dhcp src 192.0.2.10 metric 1024\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_out_of_range_cpu_topology_reads_as_unknown() {
        let wrapped = DEMO
            .replace("sockets=1", "sockets=65537")
            .replace("cores=2", "cores=70000");
        let facts = parse(&wrapped).unwrap();
        assert_eq!((facts.sockets, facts.cores), (0, 0));
        let facts = parse(DEMO).unwrap();
        assert_eq!((facts.sockets, facts.cores), (1, 2));
    }
}
