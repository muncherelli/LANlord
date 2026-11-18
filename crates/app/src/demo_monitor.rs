//! made-up host numbers that move, for the demo desktop: smooth waves over time, so the
//! graphs, the tray icon and the process list have something to show without a host. the
//! counters are the integrals of the waves, so they only ever grow, like the kernel's.

use std::collections::HashSet;
use std::sync::Mutex;

use lanlord_model::monitor::{
    CpuTimes, DiskCounters, IfaceCounters, Load, Memory, ProcessRow, Sample, Swap,
};

/// a wave: amplitude, period in seconds and phase.
type Wave = (f64, f64, f64);

/// the integral from 0 to `t` of `mean + sum of amp * sin(2 pi x / period + phase)`.
fn integral(t: f64, mean: f64, waves: &[Wave]) -> f64 {
    let tau = std::f64::consts::TAU;
    mean * t
        + waves
            .iter()
            .map(|&(amp, period, phase)| {
                let k = tau / period;
                amp / k * (phase.cos() - (k * t + phase).cos())
            })
            .sum::<f64>()
}

fn value(t: f64, mean: f64, waves: &[Wave]) -> f64 {
    let tau = std::f64::consts::TAU;
    mean + waves
        .iter()
        .map(|&(amp, period, phase)| amp * (tau * t / period + phase).sin())
        .sum::<f64>()
}

const CORES: usize = 4;

fn core_waves(core: usize) -> [Wave; 2] {
    let c = core as f64;
    [(0.30, 23.0 + 5.0 * c, c), (0.10, 6.0 + c, 2.0 * c)]
}

const NET_WAVES: [Wave; 2] = [(0.7, 17.0, 0.0), (0.25, 4.0, 1.0)];

/// the made-up processes: pid, name, owner, share of one cpu, resident kilobytes.
const PROCESSES: [(u32, &str, &str, f64, u64); 10] = [
    (1, "systemd", "root", 0.01, 12_000),
    (412, "dnsmasq", "dnsmasq", 0.04, 8_400),
    (518, "sshd", "root", 0.02, 9_100),
    (733, "lanlord", "root", 0.12, 42_000),
    (801, "caddy", "caddy", 0.08, 61_000),
    (952, "smbd", "root", 0.06, 38_000),
    (1210, "dockerd", "root", 0.15, 98_000),
    (1377, "nginx", "www-data", 0.05, 17_000),
    (1520, "rsyslogd", "syslog", 0.01, 5_200),
    (1844, "backup-job", "alice", 0.45, 24_000),
];

#[derive(Default)]
pub struct Synthetic {
    ended: Mutex<HashSet<u32>>,
}

impl Synthetic {
    /// the made-up host at `at` milliseconds.
    pub fn sample_at(&self, at: u64, procs: bool) -> Sample {
        let t = at as f64 / 1000.0;
        let jiffies = |core: usize| {
            let busy = integral(t, 0.5, &core_waves(core)) * 100.0;
            let all = t * 100.0;
            CpuTimes {
                user: (busy * 0.7) as u64,
                system: (busy * 0.3) as u64,
                idle: (all - busy).max(0.0) as u64,
                ..CpuTimes::default()
            }
        };
        let cpu_per_core: Vec<CpuTimes> = (0..CORES).map(jiffies).collect();
        let cpu_total = cpu_per_core
            .iter()
            .fold(CpuTimes::default(), |a, c| CpuTimes {
                user: a.user + c.user,
                system: a.system + c.system,
                idle: a.idle + c.idle,
                ..a
            });
        let bytes = |mean: f64, phase: f64| {
            let waves = [
                (NET_WAVES[0].0, NET_WAVES[0].1, phase),
                (NET_WAVES[1].0, NET_WAVES[1].1, phase + 1.0),
            ];
            (integral(t, 1.0, &waves) * mean) as u64
        };
        let (rx, tx) = (bytes(900_000.0, 0.0), bytes(250_000.0, 2.0));
        let net = vec![
            IfaceCounters {
                name: "eth0".into(),
                rx_bytes: rx,
                rx_packets: rx / 900,
                tx_bytes: tx,
                tx_packets: tx / 700,
                ..IfaceCounters::default()
            },
            IfaceCounters {
                name: "br-lan".into(),
                rx_bytes: bytes(300_000.0, 3.0),
                rx_packets: bytes(300_000.0, 3.0) / 800,
                tx_bytes: bytes(500_000.0, 4.5),
                tx_packets: bytes(500_000.0, 4.5) / 800,
                ..IfaceCounters::default()
            },
        ];
        let total_mem = 8u64 << 30;
        let used = value(t, 0.42, &[(0.05, 31.0, 0.5)]);
        let mem = Memory {
            total: total_mem,
            available: (total_mem as f64 * (1.0 - used)) as u64,
            buffers: 200 << 20,
            cached: 1500 << 20,
        };
        let swap_total = 2u64 << 30;
        let procs = if procs {
            let ended = self.ended.lock().map(|e| e.clone()).unwrap_or_default();
            PROCESSES
                .iter()
                .filter(|p| !ended.contains(&p.0))
                .map(|&(pid, name, user, share, rss)| ProcessRow {
                    pid,
                    name: name.into(),
                    user: user.into(),
                    state: 'S',
                    cpu_ticks: (integral(
                        t,
                        share,
                        &[(share * 0.8, 9.0 + f64::from(pid % 7), f64::from(pid))],
                    ) * 100.0) as u64,
                    rss_bytes: rss * 1024,
                    threads: 1 + pid % 9,
                })
                .collect()
        } else {
            Vec::new()
        };
        Sample {
            at,
            cpu_total,
            cpu_per_core,
            load: Load {
                one: value(t, 1.2, &[(0.6, 40.0, 0.0)]) as f32,
                five: 1.1,
                fifteen: 0.9,
            },
            mem,
            swap: Swap {
                total: swap_total,
                free: swap_total - (64 << 20),
            },
            net,
            disk: DiskCounters {
                read_bytes: (integral(t, 400_000.0, &[(300_000.0, 13.0, 1.0)])) as u64,
                write_bytes: (integral(t, 700_000.0, &[(500_000.0, 9.0, 0.0)])) as u64,
            },
            uptime: 86_400.0 * 3.0 + t,
            procs,
        }
    }

    /// pretend to end a process: it leaves the list.
    pub fn end(&self, pid: u32) -> Result<(), String> {
        if pid <= 1 {
            return Err(format!("process {pid} cannot be ended from here"));
        }
        if let Ok(mut ended) = self.ended.lock() {
            ended.insert(pid);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanlord_model::monitor::{rates, History};

    #[test]
    fn the_made_up_host_moves_and_its_counters_only_grow() {
        let demo = Synthetic::default();
        let mut history = History::default();
        let mut last = demo.sample_at(1_000_000, false);
        for i in 1..40u64 {
            let next = demo.sample_at(1_000_000 + i * 1000, false);
            assert!(next.net[0].rx_bytes >= last.net[0].rx_bytes);
            assert!(next.cpu_total.total() >= last.cpu_total.total());
            history.push(last);
            last = next;
        }
        let r = rates(&history.last.clone().unwrap(), &last).unwrap();
        assert!(r.rx_total() > 0.0 && r.tx_total() > 0.0);
        assert!(r.cpu_total > 0.0 && r.cpu_total < 1.0);
        let spread = |f: fn(&lanlord_model::monitor::Rates) -> f64| {
            let all: Vec<f64> = history.rates.iter().map(f).collect();
            all.iter().cloned().fold(f64::MIN, f64::max)
                - all.iter().cloned().fold(f64::MAX, f64::min)
        };
        assert!(spread(|r| r.rx_total()) > 1000.0, "traffic should vary");
        assert!(spread(|r| f64::from(r.cpu_total)) > 0.01, "cpu should vary");
    }

    #[test]
    fn an_ended_process_leaves_the_list_and_init_stays() {
        let demo = Synthetic::default();
        let all = demo.sample_at(5000, true).procs.len();
        assert!(demo.end(1).is_err());
        demo.end(1844).unwrap();
        let rest = demo.sample_at(6000, true).procs;
        assert_eq!(rest.len(), all - 1);
        assert!(rest.iter().all(|p| p.pid != 1844));
        assert!(demo.sample_at(7000, false).procs.is_empty());
    }
}
