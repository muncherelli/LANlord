//! the host's own numbers, read straight from `/proc` (no shell, no ssh): the sampler the
//! gateway's web ui and a local desktop use. the parsers take the file's text so they are tested
//! on fixtures; `Sampler` reads the real files.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};

pub use lanlord_model::monitor::*;

/// the kernel counts disk transfers in sectors of this many bytes whatever the device's own size.
const SECTOR: u64 = 512;

/// one `cpu` line of `/proc/stat` after its name.
fn cpu_times(fields: &str) -> Option<CpuTimes> {
    let mut n = fields.split_whitespace().map(|f| f.parse::<u64>().ok());
    let mut next = || n.next().flatten();
    // older kernels give fewer fields; a missing one is zero
    let mut take = || next().unwrap_or(0);
    Some(CpuTimes {
        user: take(),
        nice: take(),
        system: take(),
        idle: take(),
        iowait: take(),
        irq: take(),
        softirq: take(),
        steal: take(),
    })
}

/// the total and each core's times from `/proc/stat`.
pub fn parse_stat(text: &str) -> Option<(CpuTimes, Vec<CpuTimes>)> {
    let mut total = None;
    let mut cores = Vec::new();
    for line in text.lines() {
        let Some((name, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if name == "cpu" {
            total = cpu_times(rest);
        } else if name.starts_with("cpu") && name[3..].bytes().all(|b| b.is_ascii_digit()) {
            cores.extend(cpu_times(rest));
        }
    }
    Some((total?, cores))
}

/// `/proc/loadavg`: the three averages.
pub fn parse_loadavg(text: &str) -> Option<Load> {
    let mut f = text.split_whitespace().map(|f| f.parse::<f32>().ok());
    Some(Load {
        one: f.next()??,
        five: f.next()??,
        fifteen: f.next()??,
    })
}

/// `/proc/uptime`: seconds since boot.
pub fn parse_uptime(text: &str) -> Option<f64> {
    text.split_whitespace().next()?.parse().ok()
}

/// `/proc/meminfo`: memory and swap, in bytes (the file says kB, which is 1024 bytes).
pub fn parse_meminfo(text: &str) -> (Memory, Swap) {
    let kb: HashMap<&str, u64> = text
        .lines()
        .filter_map(|line| {
            let (key, rest) = line.split_once(':')?;
            Some((key, rest.split_whitespace().next()?.parse::<u64>().ok()?))
        })
        .collect();
    let get = |key: &str| kb.get(key).copied().unwrap_or(0) * 1024;
    let free = get("MemFree");
    let mem = Memory {
        total: get("MemTotal"),
        // a kernel too old to say what is available: what is free plus the page cache
        available: if kb.contains_key("MemAvailable") {
            get("MemAvailable")
        } else {
            free + get("Buffers") + get("Cached")
        },
        buffers: get("Buffers"),
        cached: get("Cached"),
    };
    let swap = Swap {
        total: get("SwapTotal"),
        free: get("SwapFree"),
    };
    (mem, swap)
}

/// `/proc/net/dev`: each interface but loopback.
pub fn parse_net_dev(text: &str) -> Vec<IfaceCounters> {
    text.lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once(':')?;
            let name = name.trim();
            if name == "lo" {
                return None;
            }
            let f: Vec<u64> = rest
                .split_whitespace()
                .map(|f| f.parse().unwrap_or(0))
                .collect();
            if f.len() < 11 {
                return None;
            }
            Some(IfaceCounters {
                name: name.into(),
                rx_bytes: f[0],
                rx_packets: f[1],
                rx_errors: f[2],
                tx_bytes: f[8],
                tx_packets: f[9],
                tx_errors: f[10],
            })
        })
        .collect()
}

/// is this `/proc/diskstats` name a whole disk? partitions are inside their disk's totals, and
/// loop, ram and device-mapper devices only repeat what is under them.
pub fn is_whole_disk(name: &str) -> bool {
    if ["loop", "ram", "zram", "dm-", "sr", "fd"]
        .iter()
        .any(|p| name.starts_with(p))
    {
        return false;
    }
    let ends_digit = name.ends_with(|c: char| c.is_ascii_digit());
    if name.starts_with("nvme") || name.starts_with("mmcblk") {
        // nvme0n1 is a disk, nvme0n1p2 a partition
        return !name.rsplit_once('p').is_some_and(|(head, tail)| {
            head.ends_with(|c: char| c.is_ascii_digit())
                && !tail.is_empty()
                && tail.bytes().all(|b| b.is_ascii_digit())
        });
    }
    !ends_digit || name.starts_with("md")
}

/// `/proc/diskstats`: bytes read and written, summed over the whole disks.
pub fn parse_diskstats(text: &str) -> DiskCounters {
    let mut total = DiskCounters::default();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 10 || !is_whole_disk(f[2]) {
            continue;
        }
        let sectors = |i: usize| f[i].parse::<u64>().unwrap_or(0) * SECTOR;
        total.read_bytes += sectors(5);
        total.write_bytes += sectors(9);
    }
    total
}

/// `/proc/[pid]/stat`: the name, state and cpu ticks. the name is in parentheses and may hold
/// spaces and parentheses itself, so the fields are counted from the last `)`.
pub fn parse_pid_stat(text: &str) -> Option<(String, char, u64, u32)> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let name = text.get(open + 1..close)?.to_string();
    let rest: Vec<&str> = text.get(close + 1..)?.split_whitespace().collect();
    // rest[0] is the state (field 3); utime and stime are fields 14 and 15; threads is 20
    let state = rest.first()?.chars().next()?;
    let utime: u64 = rest.get(11)?.parse().ok()?;
    let stime: u64 = rest.get(12)?.parse().ok()?;
    let threads: u32 = rest.get(17)?.parse().ok()?;
    Some((name, state, utime + stime, threads))
}

/// `/proc/[pid]/status`: the owner's uid and the resident memory in bytes.
pub fn parse_pid_status(text: &str) -> (Option<u32>, u64) {
    let mut uid = None;
    let mut rss = 0;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            uid = rest.split_whitespace().next().and_then(|u| u.parse().ok());
        } else if let Some(rest) = line.strip_prefix("VmRSS:") {
            rss = rest
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<u64>().ok())
                .unwrap_or(0)
                * 1024;
        }
    }
    (uid, rss)
}

/// `/etc/passwd`: user names by uid.
pub fn parse_passwd(text: &str) -> HashMap<u32, String> {
    let mut names = HashMap::new();
    for line in text.lines() {
        let mut f = line.split(':');
        if let (Some(name), Some(_), Some(uid)) = (f.next(), f.next(), f.next()) {
            if let Ok(uid) = uid.parse() {
                names.entry(uid).or_insert_with(|| name.to_string());
            }
        }
    }
    names
}

/// reads the host's `/proc`.
pub struct Sampler {
    root: PathBuf,
    passwd: PathBuf,
}

impl Default for Sampler {
    fn default() -> Self {
        Self {
            root: "/proc".into(),
            passwd: "/etc/passwd".into(),
        }
    }
}

impl Sampler {
    /// a sampler over another tree, for tests.
    pub fn at(root: impl Into<PathBuf>, passwd: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            passwd: passwd.into(),
        }
    }

    fn read(&self, name: &str) -> Result<String> {
        let path = self.root.join(name);
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))
    }

    /// one reading; the process list too when `procs` is set.
    pub fn sample(&self, procs: bool) -> Result<Sample> {
        let (cpu_total, cpu_per_core) =
            parse_stat(&self.read("stat")?).context("no cpu line in /proc/stat")?;
        let (mem, swap) = parse_meminfo(&self.read("meminfo")?);
        Ok(Sample {
            at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64),
            cpu_total,
            cpu_per_core,
            load: parse_loadavg(&self.read("loadavg")?).unwrap_or_default(),
            mem,
            swap,
            net: parse_net_dev(&self.read("net/dev")?),
            // a kernel without disk statistics just shows no disk
            disk: parse_diskstats(&self.read("diskstats").unwrap_or_default()),
            uptime: parse_uptime(&self.read("uptime")?).unwrap_or(0.0),
            procs: if procs { self.processes() } else { Vec::new() },
        })
    }

    /// every process that can still be read; one that exits mid-read is skipped.
    pub fn processes(&self) -> Vec<ProcessRow> {
        let names = std::fs::read_to_string(&self.passwd)
            .map(|t| parse_passwd(&t))
            .unwrap_or_default();
        let Ok(dir) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut rows: Vec<ProcessRow> = dir
            .flatten()
            .filter_map(|entry| {
                let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
                let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
                let (name, state, cpu_ticks, threads) = parse_pid_stat(&stat)?;
                let status =
                    std::fs::read_to_string(entry.path().join("status")).unwrap_or_default();
                let (uid, rss_bytes) = parse_pid_status(&status);
                let user = uid.map_or_else(String::new, |u| {
                    names.get(&u).cloned().unwrap_or_else(|| u.to_string())
                });
                Some(ProcessRow {
                    pid,
                    name,
                    user,
                    state,
                    cpu_ticks,
                    rss_bytes,
                    threads,
                })
            })
            .collect();
        rows.sort_by_key(|r| r.pid);
        rows
    }
}

/// end a process with SIGTERM through the `kill` program. the init process and this one are
/// refused. the caller checks that whoever asked may.
pub fn end_process(pid: u32) -> Result<()> {
    if pid <= 1 || pid == std::process::id() {
        bail!("process {pid} cannot be ended from here");
    }
    let status = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("run kill")?;
    if !status.success() {
        bail!("process {pid} could not be ended");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "cpu  100 5 50 800 20 1 2 3 0 0\ncpu0 60 2 20 400 10 1 1 1 0 0\ncpu1 40 3 30 400 10 0 1 2 0 0\nintr 1\nctxt 5\n";

    #[test]
    fn stat_gives_the_total_and_each_core() {
        let (total, cores) = parse_stat(STAT).unwrap();
        assert_eq!(total.user, 100);
        assert_eq!(total.steal, 3);
        assert_eq!(total.total(), 981);
        assert_eq!(cores.len(), 2);
        assert_eq!(cores[1].system, 30);
        assert!(parse_stat("intr 1\n").is_none());
    }

    #[test]
    fn meminfo_is_in_bytes_and_falls_back_without_available() {
        let (mem, swap) = parse_meminfo(
            "MemTotal: 1000 kB\nMemFree: 100 kB\nMemAvailable: 400 kB\nBuffers: 10 kB\nCached: 200 kB\nSwapTotal: 50 kB\nSwapFree: 20 kB\n",
        );
        assert_eq!(mem.total, 1000 * 1024);
        assert_eq!(mem.used(), 600 * 1024);
        assert_eq!(swap.used(), 30 * 1024);
        let (old, _) =
            parse_meminfo("MemTotal: 1000 kB\nMemFree: 100 kB\nBuffers: 10 kB\nCached: 200 kB\n");
        assert_eq!(old.available, 310 * 1024);
    }

    #[test]
    fn net_dev_skips_loopback_and_reads_the_right_columns() {
        let text = "Inter-|   Receive   |  Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n    lo: 10 1 0 0 0 0 0 0 10 1 0 0 0 0 0 0\n  eth0: 1000 10 2 0 0 0 0 0 500 5 1 0 0 0 0 0\n";
        let nets = parse_net_dev(text);
        assert_eq!(nets.len(), 1);
        assert_eq!(nets[0].name, "eth0");
        assert_eq!(
            (nets[0].rx_bytes, nets[0].rx_packets, nets[0].rx_errors),
            (1000, 10, 2)
        );
        assert_eq!(
            (nets[0].tx_bytes, nets[0].tx_packets, nets[0].tx_errors),
            (500, 5, 1)
        );
    }

    #[test]
    fn diskstats_sums_whole_disks_only() {
        let text = "   8 0 sda 1 0 100 0 1 0 200 0 0 0 0\n   8 1 sda1 1 0 90 0 1 0 190 0 0 0 0\n   7 0 loop0 1 0 5 0 1 0 5 0 0 0 0\n 259 0 nvme0n1 1 0 10 0 1 0 20 0 0 0 0\n 259 1 nvme0n1p1 1 0 9 0 1 0 19 0 0 0 0\n";
        let d = parse_diskstats(text);
        assert_eq!(d.read_bytes, 110 * 512);
        assert_eq!(d.write_bytes, 220 * 512);
        assert!(is_whole_disk("vda") && !is_whole_disk("vda2") && !is_whole_disk("dm-0"));
        assert!(is_whole_disk("mmcblk0") && !is_whole_disk("mmcblk0p1"));
    }

    #[test]
    fn pid_stat_survives_odd_names() {
        let text = "42 (my (odd) proc) S 1 42 42 0 -1 4194304 100 0 0 0 7 3 0 0 20 0 4 0 100 1000 50 18446744073709551615";
        let (name, state, ticks, threads) = parse_pid_stat(text).unwrap();
        assert_eq!(name, "my (odd) proc");
        assert_eq!(state, 'S');
        assert_eq!(ticks, 10);
        assert_eq!(threads, 4);
        assert!(parse_pid_stat("garbage").is_none());
    }

    #[test]
    fn status_and_passwd_give_the_owner_and_resident_memory() {
        let (uid, rss) =
            parse_pid_status("Name:\tx\nUid:\t1000\t1000\t1000\t1000\nVmRSS:\t   2048 kB\n");
        assert_eq!((uid, rss), (Some(1000), 2048 * 1024));
        assert_eq!(parse_pid_status("Name:\tkthread\n"), (None, 0));
        let names =
            parse_passwd("root:x:0:0::/root:/bin/sh\nalice:x:1000:1000::/home/alice:/bin/sh\n");
        assert_eq!(names[&1000], "alice");
    }

    #[test]
    fn loadavg_and_uptime() {
        let load = parse_loadavg("0.50 1.25 2.00 1/100 999\n").unwrap();
        assert_eq!((load.one, load.five, load.fifteen), (0.5, 1.25, 2.0));
        assert_eq!(parse_uptime("123.45 678.9\n"), Some(123.45));
        assert!(parse_loadavg("x").is_none());
    }

    #[test]
    fn a_sampler_reads_a_tree_of_files() {
        let dir = std::env::temp_dir().join(format!("lanlord-proc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("net")).unwrap();
        std::fs::create_dir_all(dir.join("7")).unwrap();
        std::fs::write(dir.join("stat"), STAT).unwrap();
        std::fs::write(dir.join("meminfo"), "MemTotal: 10 kB\nMemAvailable: 4 kB\n").unwrap();
        std::fs::write(dir.join("loadavg"), "0.1 0.2 0.3 1/1 1\n").unwrap();
        std::fs::write(dir.join("uptime"), "5.0 1.0\n").unwrap();
        std::fs::write(
            dir.join("net/dev"),
            "a:\n  eth0: 1 1 0 0 0 0 0 0 2 2 0 0 0 0 0 0\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("7/stat"),
            "7 (init) S 0 7 7 0 -1 0 0 0 0 0 1 1 0 0 20 0 1 0 1 1 1 1",
        )
        .unwrap();
        std::fs::write(dir.join("7/status"), "Uid:\t0\t0\t0\t0\nVmRSS:\t4 kB\n").unwrap();
        std::fs::write(dir.join("passwd"), "root:x:0:0::/root:/bin/sh\n").unwrap();
        let sampler = Sampler::at(&dir, dir.join("passwd"));
        let s = sampler.sample(true).unwrap();
        assert_eq!(s.cpu_per_core.len(), 2);
        assert_eq!(s.net[0].tx_bytes, 2);
        assert_eq!(s.procs.len(), 1);
        assert_eq!(
            (s.procs[0].user.as_str(), s.procs[0].cpu_ticks),
            ("root", 2)
        );
        assert!(sampler.sample(false).unwrap().procs.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_and_this_process_cannot_be_ended() {
        assert!(end_process(0).is_err());
        assert!(end_process(1).is_err());
        assert!(end_process(std::process::id()).is_err());
    }
}
