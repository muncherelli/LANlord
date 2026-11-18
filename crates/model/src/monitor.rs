//! what a host is doing right now: cpu, memory, network and disk counters and a process list,
//! as one `Sample`, and the rates two samples make. the counters are the kernel's own running
//! totals, so a rate is always the difference of two samples over the time between them.

use std::collections::{HashMap, VecDeque};

/// jiffies the cpu spent in each state since boot (the fields of a `cpu` line in `/proc/stat`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CpuTimes {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl CpuTimes {
    /// every jiffy counted.
    pub fn total(&self) -> u64 {
        self.user
            + self.nice
            + self.system
            + self.idle
            + self.iowait
            + self.irq
            + self.softirq
            + self.steal
    }

    /// the jiffies the cpu was waiting rather than working.
    pub fn idle_all(&self) -> u64 {
        self.idle + self.iowait
    }

    /// the share of time spent working between `earlier` and `self`, from 0 to 1.
    pub fn busy_since(&self, earlier: &CpuTimes) -> f32 {
        let total = self.total().saturating_sub(earlier.total());
        if total == 0 {
            return 0.0;
        }
        let idle = self.idle_all().saturating_sub(earlier.idle_all());
        (total.saturating_sub(idle) as f32 / total as f32).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Load {
    pub one: f32,
    pub five: f32,
    pub fifteen: f32,
}

/// bytes of memory.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Memory {
    pub total: u64,
    pub available: u64,
    pub buffers: u64,
    pub cached: u64,
}

impl Memory {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }
}

/// one network interface's running totals.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IfaceCounters {
    pub name: String,
    pub rx_bytes: u64,
    pub rx_packets: u64,
    pub rx_errors: u64,
    pub tx_bytes: u64,
    pub tx_packets: u64,
    pub tx_errors: u64,
}

/// the disk's running totals, summed over its whole devices, in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DiskCounters {
    pub read_bytes: u64,
    pub write_bytes: u64,
}

/// one process in the list.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProcessRow {
    pub pid: u32,
    pub name: String,
    pub user: String,
    /// the state letter from the kernel: R running, S sleeping, Z zombie...
    pub state: char,
    /// user plus system cpu time in clock ticks since the process started.
    pub cpu_ticks: u64,
    pub rss_bytes: u64,
    pub threads: u32,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Sample {
    /// milliseconds since 1970 when it was taken.
    pub at: u64,
    pub cpu_total: CpuTimes,
    pub cpu_per_core: Vec<CpuTimes>,
    pub load: Load,
    pub mem: Memory,
    pub swap: Swap,
    pub net: Vec<IfaceCounters>,
    pub disk: DiskCounters,
    /// seconds since the host booted.
    pub uptime: f64,
    /// the process list, empty unless it was asked for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub procs: Vec<ProcessRow>,
}

/// bytes of swap.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Swap {
    pub total: u64,
    pub free: u64,
}

impl Swap {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }
}

/// the clock ticks per second the kernel counts process cpu time in.
pub const TICKS_PER_SECOND: u64 = 100;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IfaceRate {
    pub name: String,
    pub rx_bytes_per_s: f64,
    pub tx_bytes_per_s: f64,
    pub rx_packets_per_s: f64,
    pub tx_packets_per_s: f64,
    pub errors: u64,
}

/// what changed between two samples.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rates {
    pub seconds: f64,
    /// busy share of all cpus, 0 to 1.
    pub cpu_total: f32,
    pub cpu_per_core: Vec<f32>,
    /// share of memory in use at the later sample, 0 to 1.
    pub mem_used: f32,
    pub net: Vec<IfaceRate>,
    pub disk_read_per_s: f64,
    pub disk_write_per_s: f64,
}

impl Rates {
    pub fn rx_total(&self) -> f64 {
        self.net.iter().map(|n| n.rx_bytes_per_s).sum()
    }

    pub fn tx_total(&self) -> f64 {
        self.net.iter().map(|n| n.tx_bytes_per_s).sum()
    }
}

/// the rates between `earlier` and `later`; `None` when the clock did not move forward.
/// a counter that went down (an interface reset) counts as no traffic, not as a huge number.
pub fn rates(earlier: &Sample, later: &Sample) -> Option<Rates> {
    let millis = later.at.checked_sub(earlier.at).filter(|m| *m > 0)?;
    let seconds = millis as f64 / 1000.0;
    let per = |a: u64, b: u64| b.saturating_sub(a) as f64 / seconds;
    let cpu_per_core = later
        .cpu_per_core
        .iter()
        .zip(&earlier.cpu_per_core)
        .map(|(l, e)| l.busy_since(e))
        .collect();
    let net = later
        .net
        .iter()
        .filter_map(|l| {
            let e = earlier.net.iter().find(|e| e.name == l.name)?;
            Some(IfaceRate {
                name: l.name.clone(),
                rx_bytes_per_s: per(e.rx_bytes, l.rx_bytes),
                tx_bytes_per_s: per(e.tx_bytes, l.tx_bytes),
                rx_packets_per_s: per(e.rx_packets, l.rx_packets),
                tx_packets_per_s: per(e.tx_packets, l.tx_packets),
                errors: (l.rx_errors + l.tx_errors).saturating_sub(e.rx_errors + e.tx_errors),
            })
        })
        .collect();
    Some(Rates {
        seconds,
        cpu_total: later.cpu_total.busy_since(&earlier.cpu_total),
        cpu_per_core,
        mem_used: later.mem.used() as f32 / later.mem.total.max(1) as f32,
        net,
        disk_read_per_s: per(earlier.disk.read_bytes, later.disk.read_bytes),
        disk_write_per_s: per(earlier.disk.write_bytes, later.disk.write_bytes),
    })
}

/// each process's cpu use between two samples as a percentage of one cpu, by pid. a process
/// that is new has no earlier reading, so it counts as 0.
pub fn process_cpu(earlier: &Sample, later: &Sample) -> HashMap<u32, f32> {
    let seconds = later.at.saturating_sub(earlier.at) as f64 / 1000.0;
    let before: HashMap<u32, u64> = earlier.procs.iter().map(|p| (p.pid, p.cpu_ticks)).collect();
    later
        .procs
        .iter()
        .map(|p| {
            let used = before
                .get(&p.pid)
                .map_or(0, |b| p.cpu_ticks.saturating_sub(*b));
            let percent = if seconds > 0.0 {
                used as f64 / TICKS_PER_SECOND as f64 / seconds * 100.0
            } else {
                0.0
            };
            (p.pid, percent as f32)
        })
        .collect()
}

/// how many readings the graphs keep.
pub const HISTORY: usize = 120;

/// the samples taken so far, as the rates the graphs draw.
#[derive(Clone, Debug, Default)]
pub struct History {
    pub last: Option<Sample>,
    /// the last sample that came with a process list.
    pub procs: Option<Sample>,
    pub process_cpu: HashMap<u32, f32>,
    pub rates: VecDeque<Rates>,
}

impl History {
    /// take in the next sample. one that is not newer than the last is ignored.
    pub fn push(&mut self, sample: Sample) {
        if let Some(last) = &self.last {
            let Some(rate) = rates(last, &sample) else {
                return;
            };
            if self.rates.len() == HISTORY {
                self.rates.pop_front();
            }
            self.rates.push_back(rate);
        }
        if !sample.procs.is_empty() {
            if let Some(before) = &self.procs {
                self.process_cpu = process_cpu(before, &sample);
            }
            self.procs = Some(sample.clone());
        }
        self.last = Some(sample);
    }

    pub fn now(&self) -> Option<&Rates> {
        self.rates.back()
    }
}

/// `1.5 KB/s`, `12 MB/s`: a rate in bytes per second, in the largest unit that keeps it short.
pub fn human_rate(bytes_per_s: f64) -> String {
    format!("{}/s", human_bytes(bytes_per_s))
}

pub fn human_bytes(bytes: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes.max(0.0);
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 || value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu(busy: u64, idle: u64) -> CpuTimes {
        CpuTimes {
            user: busy,
            idle,
            ..Default::default()
        }
    }

    fn iface(name: &str, rx: u64, tx: u64) -> IfaceCounters {
        IfaceCounters {
            name: name.into(),
            rx_bytes: rx,
            tx_bytes: tx,
            rx_packets: rx / 100,
            ..Default::default()
        }
    }

    fn sample(at: u64, busy: u64, idle: u64, rx: u64) -> Sample {
        Sample {
            at,
            cpu_total: cpu(busy, idle),
            cpu_per_core: vec![cpu(busy, idle), cpu(0, idle)],
            net: vec![iface("eth0", rx, rx / 2)],
            disk: DiskCounters {
                read_bytes: rx,
                write_bytes: 0,
            },
            ..Default::default()
        }
    }

    #[test]
    fn rates_are_differences_over_the_time_between() {
        let r = rates(&sample(1000, 0, 100, 0), &sample(3000, 150, 150, 4000)).unwrap();
        assert_eq!(r.seconds, 2.0);
        assert!((r.cpu_total - 0.75).abs() < 1e-6);
        assert!((r.cpu_per_core[1] - 0.0).abs() < 1e-6);
        assert_eq!(r.net[0].rx_bytes_per_s, 2000.0);
        assert_eq!(r.net[0].tx_bytes_per_s, 1000.0);
        assert_eq!(r.disk_read_per_s, 2000.0);
        assert_eq!(r.rx_total(), 2000.0);
    }

    #[test]
    fn a_clock_that_stood_still_or_went_back_has_no_rates() {
        assert!(rates(&sample(5, 0, 0, 0), &sample(5, 1, 1, 1)).is_none());
        assert!(rates(&sample(9, 0, 0, 0), &sample(5, 1, 1, 1)).is_none());
    }

    #[test]
    fn a_counter_that_reset_counts_as_no_traffic() {
        let r = rates(&sample(0, 0, 0, 9000), &sample(1000, 0, 0, 100)).unwrap();
        assert_eq!(r.net[0].rx_bytes_per_s, 0.0);
        assert_eq!(r.cpu_total, 0.0);
    }

    #[test]
    fn an_interface_that_appeared_has_no_rate_until_the_next_sample() {
        let mut later = sample(1000, 0, 0, 0);
        later.net.push(iface("eth1", 50, 50));
        let r = rates(&sample(0, 0, 0, 0), &later).unwrap();
        assert_eq!(r.net.len(), 1);
    }

    #[test]
    fn process_cpu_is_a_percentage_of_one_cpu() {
        let row = |pid, ticks| ProcessRow {
            pid,
            cpu_ticks: ticks,
            ..Default::default()
        };
        let mut a = sample(0, 0, 0, 0);
        a.procs = vec![row(1, 100)];
        let mut b = sample(2000, 0, 0, 0);
        b.procs = vec![row(1, 200), row(2, 500)];
        let c = process_cpu(&a, &b);
        assert!((c[&1] - 50.0).abs() < 1e-4);
        assert_eq!(c[&2], 0.0);
    }

    #[test]
    fn history_keeps_a_window_and_skips_stale_samples() {
        let mut h = History::default();
        for i in 0..(HISTORY as u64 + 10) {
            h.push(sample(i * 1000, i, i, i * 10));
        }
        assert_eq!(h.rates.len(), HISTORY);
        let len = h.rates.len();
        h.push(sample(0, 0, 0, 0));
        assert_eq!(h.rates.len(), len);
        assert!(h.now().is_some());
    }

    #[test]
    fn rates_read_as_short_text() {
        assert_eq!(human_rate(0.0), "0 B/s");
        assert_eq!(human_rate(1536.0), "1.5 KB/s");
        assert_eq!(human_bytes(150.0 * 1024.0 * 1024.0), "150 MB");
    }

    #[test]
    fn a_sample_round_trips_as_json_and_omits_an_empty_process_list() {
        let s = sample(1, 2, 3, 4);
        let text = serde_json::to_string(&s).unwrap();
        assert!(!text.contains("procs"));
        assert_eq!(serde_json::from_str::<Sample>(&text).unwrap(), s);
    }
}
