//! what the signed-in server is: its name, system, uptime and network interfaces, from one
//! read-only script that needs no sudo. the My Server and Networks windows show it.

use anyhow::{bail, Result};

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
    pub cpus: u32,
    pub mem_total_mb: u64,
    pub mem_available_mb: u64,
    /// lanlord's directory and service account are both there.
    pub lanlord_installed: bool,
    pub lanlord_version: Option<String>,
    pub last_applied: Option<String>,
    pub interfaces: Vec<Iface>,
}

impl ServerFacts {
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

/// the interface a `default via ... dev NAME` line leaves through.
fn default_interface(route: &str) -> Option<&str> {
    route.lines().find_map(|l| after(l, "dev"))
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
    if let Some(name) = default_interface(route) {
        if let Some(iface) = interfaces.iter_mut().find(|i| i.name == name) {
            iface.default_route = true;
        }
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
        cpus: number(&head, "cpus") as u32,
        mem_total_mb: number(&head, "mem_total_kb") / 1024,
        mem_available_mb: number(&head, "mem_available_kb") / 1024,
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

const DEMO: &str = "hostname=demo\nos=Arch Linux\nkernel=Linux 6.12.7-arch1-1\nuptime=273900\nuser=root\nuid=0\ncpus=4\nmem_total_kb=4194304\nmem_available_kb=3407872\ninstalled=1\nversion=0.1.0\napplied=2026-11-15T20:41:07-06:00\n---link\n1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536 qdisc noqueue state UNKNOWN mode DEFAULT group default qlen 1000\\    link/loopback 00:00:00:00:00:00 brd 00:00:00:00:00:00\n2: eth0@if48: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc noqueue state UP mode DEFAULT group default qlen 1000\\    link/ether 02:00:00:00:aa:01 brd ff:ff:ff:ff:ff:ff link-netnsid 0\n3: eth1@if49: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1450 qdisc noqueue state UP mode DEFAULT group default qlen 1000\\    link/ether 02:00:00:00:aa:02 brd ff:ff:ff:ff:ff:ff link-netnsid 0\n---addr\n1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever preferred_lft forever\n2: eth0    inet 192.0.2.10/24 metric 1024 brd 192.0.2.255 scope global dynamic eth0\\       valid_lft 3600sec preferred_lft 3600sec\n3: eth1    inet 172.16.120.1/22 brd 172.16.123.255 scope global eth1\\       valid_lft forever preferred_lft forever\n---route\ndefault via 192.0.2.1 dev eth0 proto dhcp src 192.0.2.10 metric 1024\n";
