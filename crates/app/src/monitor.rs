//! live numbers for the Task Manager and the tray icon: where they come from, the history the
//! graphs draw and the process list's sorting. one `Monitor` per desktop asks its backend for a
//! sample about once a second, off the ui thread, and the windows read what has arrived.
//!
//! the backends: the made-up demo host, this machine's own `/proc` (lanlord on the gateway, a
//! native build) and the web ui's `/api/monitor` (the browser). a desktop that signed in to
//! another host over ssh has none yet; it shows why instead of numbers.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use lanlord_model::monitor::{History, ProcessRow, Rates, Sample};

use crate::demo_monitor::Synthetic;
use crate::wake::Wake;

/// where the numbers come from.
#[derive(Clone)]
pub enum Backend {
    /// nothing to read, and why.
    Off(String),
    Demo(Arc<Synthetic>),
    /// this machine's `/proc`.
    #[cfg(feature = "native")]
    Local,
    /// the page's own server.
    #[cfg(target_arch = "wasm32")]
    Web,
}

impl Backend {
    fn name(&self) -> &'static str {
        match self {
            Self::Off(_) => "off",
            Self::Demo(_) => "demo",
            #[cfg(feature = "native")]
            Self::Local => "local",
            #[cfg(target_arch = "wasm32")]
            Self::Web => "web",
        }
    }
}

enum Msg {
    Sample(Result<Box<Sample>, String>),
    /// the server turned a request down for want of a login.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    SignedOut,
    Ended(u32, Result<(), String>),
}

/// the seconds between samples while a window shows them, and while only the tray does.
pub const FAST: f64 = 1.0;
/// slower in the browser, where every sample is a request to the gateway.
pub const SLOW: f64 = if cfg!(target_arch = "wasm32") {
    5.0
} else {
    2.0
};

/// how often to sample, and whether to ask for the process list: only a visible Task Manager
/// needs the list and the quick pace, the tray alone takes the slow one.
pub fn cadence(task_manager_shown: bool) -> (f64, bool) {
    if task_manager_shown {
        (FAST, true)
    } else {
        (SLOW, false)
    }
}

pub struct Monitor {
    backend: Backend,
    pub history: History,
    /// why there is nothing to show, or the last read's failure.
    pub problem: Option<String>,
    /// the last End Process result: true when it failed.
    pub note: Option<(bool, String)>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    asked: bool,
    next_at: f64,
    signed_out: bool,
}

impl Default for Monitor {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            backend: Backend::Off("Nothing to monitor yet.".into()),
            history: History::default(),
            problem: None,
            note: None,
            tx,
            rx,
            asked: false,
            next_at: 0.0,
            signed_out: false,
        }
    }
}

fn now_ms() -> u64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Monitor {
    /// switch to `backend` when it is another one; the history of the old one is dropped.
    pub fn use_backend(&mut self, backend: Backend) {
        if self.backend.name() == backend.name()
            && !matches!((&self.backend, &backend), (Backend::Off(a), Backend::Off(b)) if a != b)
        {
            return;
        }
        self.history = History::default();
        self.problem = match &backend {
            Backend::Off(why) => Some(why.clone()),
            _ => None,
        };
        self.backend = backend;
        self.next_at = 0.0;
        // a fresh channel: replies still on their way from the old backend find no receiver
        let (tx, rx) = channel();
        (self.tx, self.rx) = (tx, rx);
        self.asked = false;
    }

    /// did the server say the session is over? the desktop then returns to the logon box.
    pub fn signed_out(&self) -> bool {
        self.signed_out
    }

    /// the logon box is showing: forget the answer.
    pub fn clear_signed_out(&mut self) {
        self.signed_out = false;
        self.asked = false;
    }

    /// is there a host being read?
    pub fn live(&self) -> bool {
        !matches!(self.backend, Backend::Off(_))
    }

    /// the newest rates, once two samples have arrived.
    pub fn now(&self) -> Option<&Rates> {
        self.history.now()
    }

    /// take in what the background reads brought; true when something changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(msg) = self.rx.try_recv() {
            changed = true;
            match msg {
                Msg::Sample(Ok(sample)) => {
                    self.asked = false;
                    self.problem = None;
                    self.history.push(*sample);
                }
                Msg::Sample(Err(why)) => {
                    self.asked = false;
                    self.problem = Some(why);
                }
                Msg::Ended(pid, Ok(())) => {
                    self.note = Some((false, format!("Process {pid} was asked to end.")));
                    // show the list without it at once
                    if let Some(procs) = self.history.procs.as_mut() {
                        procs.procs.retain(|p| p.pid != pid);
                    }
                    self.next_at = 0.0;
                }
                Msg::Ended(_, Err(why)) => self.note = Some((true, why)),
                Msg::SignedOut => self.signed_out = true,
            }
        }
        changed
    }

    /// ask for the next sample when it is due and none is on its way. `procs` also asks for the
    /// process list, which the Task Manager's Processes tab needs and the tray does not.
    pub fn tick(&mut self, now: f64, every: f64, procs: bool, wake: &Wake) {
        if self.asked || now < self.next_at || !self.live() {
            return;
        }
        self.asked = true;
        self.next_at = now + every;
        let (tx, wake, backend) = (self.tx.clone(), wake.clone(), self.backend.clone());
        #[cfg(target_arch = "wasm32")]
        if matches!(backend, Backend::Web) {
            wasm_bindgen_futures::spawn_local(async move {
                let result = crate::gateway::http::monitor_sample(procs).await;
                let _ = tx.send(sample_msg(result));
                wake.wake();
            });
            return;
        }
        run(move || {
            let result = read(&backend, procs);
            let _ = tx.send(Msg::Sample(result.map(Box::new)));
            wake.wake();
        });
    }

    /// end process `pid` on the host. the caller has checked that the person may; the server
    /// checks again.
    pub fn end(&mut self, pid: u32, wake: &Wake) {
        let (tx, wake, backend) = (self.tx.clone(), wake.clone(), self.backend.clone());
        self.note = None;
        #[cfg(target_arch = "wasm32")]
        if matches!(backend, Backend::Web) {
            wasm_bindgen_futures::spawn_local(async move {
                let result = crate::gateway::http::monitor_end(pid).await;
                let _ = tx.send(ended_msg(pid, result));
                wake.wake();
            });
            return;
        }
        run(move || {
            let result = end_on(&backend, pid);
            let _ = tx.send(Msg::Ended(pid, result));
            wake.wake();
        });
    }
}

/// what a sample's answer from the server means: the person is signed out, or a sample or error.
#[cfg(target_arch = "wasm32")]
fn sample_msg(result: anyhow::Result<Sample>) -> Msg {
    match result {
        Err(e) if e.is::<crate::gateway::round_trip::SessionEnded>() => Msg::SignedOut,
        other => Msg::Sample(other.map(Box::new).map_err(|e| e.to_string())),
    }
}

#[cfg(target_arch = "wasm32")]
fn ended_msg(pid: u32, result: anyhow::Result<()>) -> Msg {
    match result {
        Err(e) if e.is::<crate::gateway::round_trip::SessionEnded>() => Msg::SignedOut,
        other => Msg::Ended(pid, other.map_err(|e| e.to_string())),
    }
}

/// read one sample from `backend`.
fn read(backend: &Backend, procs: bool) -> Result<Sample, String> {
    match backend {
        Backend::Off(why) => Err(why.clone()),
        Backend::Demo(demo) => Ok(demo.sample_at(now_ms(), procs)),
        #[cfg(feature = "native")]
        Backend::Local => lanlord_core::monitor::Sampler::default()
            .sample(procs)
            .map_err(|e| format!("{e:#}")),
        #[cfg(target_arch = "wasm32")]
        Backend::Web => Err("the browser fetches its samples".into()),
    }
}

fn end_on(backend: &Backend, pid: u32) -> Result<(), String> {
    match backend {
        Backend::Off(why) => Err(why.clone()),
        Backend::Demo(demo) => demo.end(pid),
        #[cfg(feature = "native")]
        Backend::Local => lanlord_core::monitor::end_process(pid).map_err(|e| format!("{e:#}")),
        #[cfg(target_arch = "wasm32")]
        Backend::Web => Err("the browser fetches its requests".into()),
    }
}

/// off the ui thread: a thread on the desktop; in the browser the work runs after the frame
/// (the demo's numbers are computed there; the real server's are fetched, see `Monitor::tick`).
#[cfg(not(target_arch = "wasm32"))]
fn run(work: impl FnOnce() + Send + 'static) {
    std::thread::spawn(work);
}

#[cfg(target_arch = "wasm32")]
fn run(work: impl FnOnce() + Send + 'static) {
    wasm_bindgen_futures::spawn_local(async move { work() });
}

/// how the process list is ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Name,
    Pid,
    User,
    Cpu,
    Memory,
    Threads,
}

pub const COLUMNS: [(Column, &str); 6] = [
    (Column::Name, "Image Name"),
    (Column::Pid, "PID"),
    (Column::User, "User Name"),
    (Column::Cpu, "CPU"),
    (Column::Memory, "Mem Usage"),
    (Column::Threads, "Threads"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Order {
    pub column: Column,
    pub descending: bool,
}

impl Default for Order {
    fn default() -> Self {
        Self {
            column: Column::Cpu,
            descending: true,
        }
    }
}

impl Order {
    /// a click on a heading: the same column flips, another starts the way its numbers read best
    /// (biggest first), and names start A to Z.
    pub fn click(&mut self, column: Column) {
        if self.column == column {
            self.descending = !self.descending;
        } else {
            self.column = column;
            self.descending = !matches!(column, Column::Name | Column::User);
        }
    }
}

/// the rows ordered, with each one's cpu percentage.
pub fn sorted<'a>(
    rows: &'a [ProcessRow],
    cpu: &HashMap<u32, f32>,
    order: Order,
) -> Vec<(&'a ProcessRow, f32)> {
    let mut list: Vec<(&ProcessRow, f32)> = rows
        .iter()
        .map(|r| (r, cpu.get(&r.pid).copied().unwrap_or(0.0)))
        .collect();
    list.sort_by(|(a, ac), (b, bc)| {
        let by = match order.column {
            Column::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            Column::Pid => a.pid.cmp(&b.pid),
            Column::User => a.user.cmp(&b.user),
            Column::Cpu => ac.total_cmp(bc),
            Column::Memory => a.rss_bytes.cmp(&b.rss_bytes),
            Column::Threads => a.threads.cmp(&b.threads),
        }
        .then(a.pid.cmp(&b.pid));
        if order.descending {
            by.reverse()
        } else {
            by
        }
    });
    list
}

/// the cells of one process row, in `COLUMNS` order.
pub fn cells(row: &ProcessRow, cpu: f32) -> Vec<String> {
    vec![
        row.name.clone(),
        row.pid.to_string(),
        row.user.clone(),
        format!("{cpu:.0}"),
        format!("{} K", row.rss_bytes / 1024),
        row.threads.to_string(),
    ]
}

/// the interface names, in the order the graphs list them.
pub fn interfaces(history: &History) -> Vec<String> {
    history
        .last
        .as_ref()
        .map(|s| s.net.iter().map(|n| n.name.clone()).collect())
        .unwrap_or_default()
}

/// the tray icon's tooltip: the rates summed over every interface.
pub fn tooltip(rates: Option<&Rates>) -> String {
    use lanlord_model::monitor::human_rate;
    match rates {
        Some(r) => format!(
            "Network activity\nReceiving: {}\nSending: {}",
            human_rate(r.rx_total()),
            human_rate(r.tx_total())
        ),
        None => "Network activity".into(),
    }
}

/// what the top-like screen can be asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopMsg {
    Sort(Column),
}

/// the top-like screen for the terminal: the totals, then the processes in text columns. it
/// only reads; ending a process is for the Task Manager window.
pub fn top_view(monitor: &Monitor, order: Order) -> Vec<crate::view::Node<TopMsg>> {
    use crate::view::{Button, Node};
    use lanlord_model::monitor::{human_bytes, human_rate};
    let mut body = Vec::new();
    if let (Some(sample), rates) = (&monitor.history.last, monitor.now()) {
        let cpu = rates.map_or(0.0, |r| r.cpu_total * 100.0);
        body.push(Node::Field {
            label: "CPU".into(),
            value: format!(
                "{cpu:.0}%   load {:.2} {:.2} {:.2}",
                sample.load.one, sample.load.five, sample.load.fifteen
            ),
        });
        body.push(Node::Field {
            label: "Memory".into(),
            value: format!(
                "{} used of {}   swap {} of {}",
                human_bytes(sample.mem.used() as f64),
                human_bytes(sample.mem.total as f64),
                human_bytes(sample.swap.used() as f64),
                human_bytes(sample.swap.total as f64)
            ),
        });
        body.push(Node::Field {
            label: "Network".into(),
            value: rates.map_or_else(String::new, |r| {
                format!(
                    "in {}   out {}",
                    human_rate(r.rx_total()),
                    human_rate(r.tx_total())
                )
            }),
        });
        body.push(Node::Field {
            label: "Uptime".into(),
            value: format!(
                "{}d {:02}h {:02}m",
                sample.uptime as u64 / 86_400,
                sample.uptime as u64 / 3600 % 24,
                sample.uptime as u64 / 60 % 60
            ),
        });
    } else {
        let text = monitor
            .problem
            .clone()
            .unwrap_or_else(|| "Reading the host...".into());
        body.push(Node::Text(text));
    }
    body.push(Node::Gap(4.0));
    body.push(Node::Buttons(
        COLUMNS
            .iter()
            .rev()
            .filter(|(c, _)| *c != Column::User && *c != Column::Threads)
            .map(|(c, label)| Button::new(label, order.column != *c, TopMsg::Sort(*c)))
            .collect(),
    ));
    let mut lines = vec![format!(
        "{:>7} {:<12} {:>5} {:>10} {:>4}  {}",
        "PID", "USER", "CPU%", "MEM", "THR", "NAME"
    )];
    if let Some(procs) = &monitor.history.procs {
        for (row, cpu) in sorted(&procs.procs, &monitor.history.process_cpu, order) {
            lines.push(format!(
                "{:>7} {:<12.12} {:>5.1} {:>10} {:>4}  {}",
                row.pid,
                row.user,
                cpu,
                human_bytes(row.rss_bytes as f64),
                row.threads,
                row.name
            ));
        }
    }
    body.push(Node::Lines {
        id: "top",
        lines,
        height: 300.0,
    });
    body
}

/// a rate counts as activity (a monitor lights up) above this many bytes a second.
pub const ACTIVE: f64 = 1.0;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo_monitor::Synthetic;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// poll until something arrives, or fail after a few seconds.
    fn wait(m: &mut Monitor) {
        let start = Instant::now();
        while !m.poll() {
            assert!(start.elapsed() < Duration::from_secs(5), "no sample came");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn demo() -> Monitor {
        let mut m = Monitor::default();
        m.use_backend(Backend::Demo(Arc::new(Synthetic::default())));
        m
    }

    #[test]
    fn the_pace_and_the_process_list_follow_the_task_manager() {
        assert_eq!(cadence(true), (FAST, true));
        assert_eq!(cadence(false), (SLOW, false));
    }

    #[test]
    fn a_sample_is_asked_for_only_when_due_and_none_is_on_its_way() {
        let mut m = demo();
        m.tick(10.0, 1.0, true, &Wake::none());
        assert!(m.asked && m.next_at == 11.0);
        // one is on its way: a later tick asks for nothing more
        m.tick(12.0, 1.0, true, &Wake::none());
        assert!(m.next_at == 11.0);
        wait(&mut m);
        assert!(m.history.procs.is_some(), "the list was asked for");
        // not due yet
        m.tick(10.5, 1.0, true, &Wake::none());
        assert!(!m.asked);
        std::thread::sleep(Duration::from_millis(5));
        // due, without the list: the sample carries none
        m.tick(11.0, 2.0, false, &Wake::none());
        assert!(m.asked && m.next_at == 13.0);
        wait(&mut m);
        assert!(m.history.last.as_ref().is_some_and(|s| s.procs.is_empty()));
    }

    #[test]
    fn a_reply_from_the_old_backend_is_dropped_after_a_switch() {
        let mut m = demo();
        m.tick(0.0, 1.0, false, &Wake::none());
        assert!(m.asked);
        m.use_backend(Backend::Off("elsewhere".into()));
        assert!(!m.asked && m.history.last.is_none());
        std::thread::sleep(Duration::from_millis(100));
        assert!(!m.poll(), "the old reply must not land");
        assert!(m.history.last.is_none());
        // and the new backend can be asked at once
        m.use_backend(Backend::Demo(Arc::new(Synthetic::default())));
        m.tick(0.0, 1.0, false, &Wake::none());
        wait(&mut m);
        assert!(m.history.last.is_some());
    }

    #[test]
    fn a_signed_out_answer_is_kept_until_the_logon_box_clears_it() {
        let mut m = demo();
        m.tx.send(Msg::SignedOut).unwrap();
        assert!(m.poll() && m.signed_out());
        m.clear_signed_out();
        assert!(!m.signed_out());
    }

    fn row(pid: u32, name: &str, rss: u64) -> ProcessRow {
        ProcessRow {
            pid,
            name: name.into(),
            rss_bytes: rss,
            ..Default::default()
        }
    }

    #[test]
    fn rows_sort_by_the_clicked_column_and_flip_on_a_second_click() {
        let rows = vec![
            row(3, "Bravo", 10),
            row(1, "alpha", 30),
            row(2, "charlie", 20),
        ];
        let cpu = HashMap::from([(1, 5.0), (2, 50.0), (3, 0.0)]);
        let pids = |o| {
            sorted(&rows, &cpu, o)
                .iter()
                .map(|r| r.0.pid)
                .collect::<Vec<_>>()
        };
        let mut order = Order::default();
        assert_eq!(pids(order), [2, 1, 3]);
        order.click(Column::Name);
        assert_eq!(pids(order), [1, 3, 2]);
        order.click(Column::Name);
        assert_eq!(pids(order), [2, 3, 1]);
        order.click(Column::Memory);
        assert_eq!(pids(order), [1, 2, 3]);
    }

    #[test]
    fn the_demo_backend_fills_the_history_and_ends_a_process() {
        let mut monitor = Monitor::default();
        assert!(!monitor.live());
        monitor.use_backend(Backend::Demo(Arc::default()));
        assert!(monitor.live() && monitor.problem.is_none());
        let wake = Wake::none();
        let mut t = 0.0;
        // each pass starts a read on a thread; wait for it
        for _ in 0..3 {
            monitor.tick(t, FAST, true, &wake);
            while !monitor.poll() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            t += 2.0;
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(monitor.now().is_some());
        assert!(monitor.history.procs.is_some());
        monitor.end(1844, &wake);
        while !monitor.poll() {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(monitor.note.as_ref().map(|n| n.0), Some(false));
        assert!(monitor
            .history
            .procs
            .unwrap()
            .procs
            .iter()
            .all(|p| p.pid != 1844));
    }

    #[test]
    fn a_backend_with_nothing_to_read_says_why_and_never_asks() {
        let mut monitor = Monitor::default();
        monitor.use_backend(Backend::Off("remote".into()));
        assert_eq!(monitor.problem.as_deref(), Some("remote"));
        monitor.tick(0.0, FAST, false, &Wake::none());
        assert!(!monitor.poll());
    }

    #[test]
    fn the_top_screen_lists_processes_under_the_totals() {
        use crate::view::Node;
        let mut monitor = Monitor::default();
        monitor.use_backend(Backend::Demo(Arc::default()));
        let demo = Synthetic::default();
        monitor.history.push(demo.sample_at(1000, true));
        monitor.history.push(demo.sample_at(3000, true));
        let nodes = top_view(&monitor, Order::default());
        assert!(nodes
            .iter()
            .any(|n| matches!(n, Node::Field { label, .. } if label == "Network")));
        let lines = nodes
            .iter()
            .find_map(|n| match n {
                Node::Lines { lines, .. } => Some(lines),
                _ => None,
            })
            .unwrap();
        assert_eq!(lines.len(), 11, "a heading and the ten made-up processes");
        assert!(lines[0].contains("NAME") && lines.iter().any(|l| l.contains("dockerd")));
    }

    #[test]
    fn the_tooltip_names_both_rates() {
        let r = Rates {
            net: vec![lanlord_model::monitor::IfaceRate {
                name: "eth0".into(),
                rx_bytes_per_s: 2048.0,
                tx_bytes_per_s: 0.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let tip = tooltip(Some(&r));
        assert!(tip.contains("Receiving: 2.0 KB/s") && tip.contains("Sending: 0 B/s"));
    }
}
