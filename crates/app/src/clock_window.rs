//! the server's clock: the time shown beside the taskbar, and the Date/Time Properties window
//! you open by double-clicking it. both act on the server you are signed in to, not on this
//! computer.

use std::collections::BTreeMap;
use web_time::Instant;

use anyhow::Result;
use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};
use lanlord_model::clock::{self, Change, ClockState, NtpChange, Stored, Sync, ZoneNow};

use crate::gateway::Gateway;
use crate::remote::Remote;
use crate::services::Services;
use crate::tasks::Job;
use crate::view::{self, Button, Node, Size};

pub const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
pub const WEEKDAYS: [&str; 7] = ["S", "M", "T", "W", "T", "F", "S"];
/// what the server field says when lanlord has not configured any.
const DEFAULT_SERVERS: &str = "pool.ntp.org";
const NO_NTP_WITH_TIME: &str =
    "A time can only be set while automatic synchronization is off. Turn it off on the Internet Time tab.";

/// the server's local time `utc + offset`, as a date and time.
fn local_datetime(utc_secs: i64, offset_secs: i32) -> NaiveDateTime {
    chrono::DateTime::from_timestamp(utc_secs + i64::from(offset_secs), 0)
        .map(|t| t.naive_utc())
        .unwrap_or_default()
}

// the tray

/// the server's clock for the taskbar: read once over ssh, then kept by counting the seconds
/// here. it never marks the server busy, so it cannot hold up anything you are doing.
pub struct ClockFeed {
    /// the server `state` was read from.
    host: Option<String>,
    state: Option<ClockState>,
    read_at: Instant,
    job: Option<(String, Job<ClockState>)>,
    /// a server whose read failed, so it is not retried every frame.
    failed: Option<String>,
    /// the reading is old (the dialog was closed): read it again.
    stale: bool,
}

impl Default for ClockFeed {
    fn default() -> Self {
        Self {
            host: None,
            state: None,
            read_at: Instant::now(),
            job: None,
            failed: None,
            stale: false,
        }
    }
}

impl ClockFeed {
    /// follow `host`: read its clock when there is no reading, or an old one.
    pub fn watch(&mut self, services: &Services, host: Option<&str>) {
        self.poll(host);
        let Some(host) = host else {
            *self = Self::default();
            return;
        };
        let current = self.host.as_deref() == Some(host) && !self.stale;
        let given_up = self.failed.as_deref() == Some(host) && !self.stale;
        if current || given_up || self.job.is_some() {
            return;
        }
        if self.host.as_deref() != Some(host) {
            self.state = None;
        }
        let target = host.to_string();
        let job = services.quiet(move |gateway| gateway.clock_state(&target));
        self.job = Some((host.to_string(), job));
    }

    fn poll(&mut self, want: Option<&str>) {
        let Some((host, job)) = self.job.take() else {
            return;
        };
        match job.poll() {
            None => self.job = Some((host, job)),
            Some(Ok(state)) if want == Some(host.as_str()) => self.set(&host, state),
            Some(Ok(_)) => {}
            Some(Err(_)) => {
                self.failed = Some(host);
                self.stale = false;
            }
        }
    }

    /// take a reading that came from somewhere else (the dialog just read or changed it).
    pub fn set(&mut self, host: &str, state: ClockState) {
        self.host = Some(host.to_string());
        self.state = Some(state);
        self.read_at = Instant::now();
        self.stale = false;
        self.failed = None;
    }

    /// read the clock again soon.
    pub fn refresh(&mut self) {
        self.stale = true;
        self.failed = None;
    }

    /// the time on the clock, like `5:12 PM`: the server's when there is a reading, else this
    /// computer's.
    pub fn text(&self) -> String {
        match &self.state {
            Some(state) => {
                let secs = i64::try_from(self.read_at.elapsed().as_secs()).unwrap_or(0);
                local_datetime(state.utc_secs + secs, state.offset_secs)
                    .format("%-I:%M %p")
                    .to_string()
            }
            None => chrono::Local::now().format("%-I:%M %p").to_string(),
        }
    }

    /// which clock it is, for the tooltip.
    pub fn tooltip(&self) -> String {
        match (&self.state, &self.host) {
            (Some(state), Some(host)) => format!(
                "Time on {host} ({}). Double-click to change the date, time and time zone.",
                state.timezone
            ),
            _ => "This computer's time; there is no server reading yet. Double-click to open Date/Time Properties.".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    DateTime,
    Zone,
    Internet,
}

/// what a click or a keystroke in the window means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    Tab(usize),
    /// the month, 1 to 12.
    Month(usize),
    /// the year, one step up (1) or down (-1).
    Year(i32),
    Day(NaiveDate),
    Hour(i32),
    Minute(i32),
    Second(i32),
    AmPm(i32),
    /// the region at this index of the region list.
    Region(usize),
    /// the zone at this index of the chosen region's zones.
    City(usize),
    ToggleNtp,
    Servers(String),
    UpdateNow,
    Ok,
    Cancel,
    Apply,
}

/// which buttons work right now; the view greys out the rest and `update` ignores them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gates {
    pub ok: bool,
    pub apply: bool,
    /// the date and time can be set by hand.
    pub time: bool,
    /// internet time can be switched.
    pub ntp: bool,
    /// the server list can be edited.
    pub servers: bool,
    pub update_now: bool,
}

/// what the window loads: the server's clock and the zones it knows, and whether the last
/// change was kept in the settings.
pub struct ClockData {
    pub state: ClockState,
    pub zones: Vec<String>,
    pub kept: Option<Stored>,
}

/// what to say after a change, beyond "applied", about keeping it in the settings.
fn kept_text(kept: &Stored) -> Option<String> {
    match kept {
        Stored::Yes => Some("applied and kept in the settings".into()),
        Stored::NoKey => Some(
            "applied; not kept in the settings (no key yet), so an apply may undo it".into(),
        ),
        Stored::NotInstalled => Some(
            "applied; LANlord is not installed on this server, so the change is not stored in its settings"
                .into(),
        ),
        Stored::Failed(why) => Some(format!(
            "applied, but could not be remembered in LANlord's settings: {why}"
        )),
        Stored::Nothing => None,
    }
}

fn load(gateway: &dyn Gateway, host: &str, kept: Option<Stored>) -> Result<ClockData> {
    Ok(ClockData {
        state: gateway.clock_state(host)?,
        zones: gateway.clock_zones(host)?,
        kept,
    })
}

/// the zones grouped by region, for the two drop-downs.
struct Zones {
    regions: Vec<String>,
    by_region: BTreeMap<String, Vec<String>>,
}

impl Zones {
    fn new(list: &[String]) -> Self {
        let mut by_region: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for zone in list {
            by_region
                .entry(Self::region_of(zone))
                .or_default()
                .push(zone.clone());
        }
        let regions = by_region.keys().cloned().collect();
        Self { regions, by_region }
    }

    /// `Europe` for `Europe/Berlin`; zones with no slash (`UTC`) are in `Other`.
    fn region_of(zone: &str) -> String {
        zone.split_once('/')
            .map_or("Other", |(region, _)| region)
            .to_string()
    }

    /// `Berlin` for `Europe/Berlin`, `Argentina/Buenos_Aires` for the long ones.
    fn city(zone: &str) -> &str {
        zone.split_once('/').map_or(zone, |(_, city)| city)
    }
}

/// what you have edited so far, before OK or Apply.
struct Draft {
    date: NaiveDate,
    hour: u32,
    minute: u32,
    second: u32,
    /// the date or time was touched: the clock stops following the server's.
    edited: bool,
    zone: String,
    ntp: bool,
    servers: String,
}

fn step(value: u32, dir: i32, min: u32, max: u32) -> u32 {
    let span = i64::from(max - min + 1);
    let moved = i64::from(value - min) + i64::from(dir);
    u32::try_from(moved.rem_euclid(span)).unwrap_or(0) + min
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (y, m) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(y, m, 1)
        .and_then(|d| d.pred_opt())
        .map_or(28, |d| d.day())
}

impl Draft {
    fn from_state(state: &ClockState, elapsed: i64) -> Self {
        let now = local_datetime(state.utc_secs + elapsed, state.offset_secs);
        Self {
            date: now.date(),
            hour: now.hour(),
            minute: now.minute(),
            second: now.second(),
            edited: false,
            zone: state.timezone.clone(),
            ntp: state.ntp,
            servers: if state.servers.is_empty() {
                DEFAULT_SERVERS.to_string()
            } else {
                state.servers.join(" ")
            },
        }
    }

    /// keep up with the server's clock until the date or time is edited.
    fn follow(&mut self, state: &ClockState, elapsed: i64) {
        if !self.edited {
            let now = local_datetime(state.utc_secs + elapsed, state.offset_secs);
            (self.date, self.hour, self.minute, self.second) =
                (now.date(), now.hour(), now.minute(), now.second());
        }
    }

    fn hour12(&self) -> u32 {
        if self.hour.is_multiple_of(12) {
            12
        } else {
            self.hour % 12
        }
    }

    fn pm(&self) -> bool {
        self.hour >= 12
    }

    fn bump_hour(&mut self, dir: i32) {
        let h12 = step(self.hour12(), dir, 1, 12);
        self.hour = h12 % 12 + if self.pm() { 12 } else { 0 };
        self.edited = true;
    }

    fn bump_minute(&mut self, dir: i32) {
        self.minute = step(self.minute, dir, 0, 59);
        self.edited = true;
    }

    fn bump_second(&mut self, dir: i32) {
        self.second = step(self.second, dir, 0, 59);
        self.edited = true;
    }

    fn toggle_pm(&mut self) {
        self.hour = (self.hour + 12) % 24;
        self.edited = true;
    }

    /// the same day in another month or year, or the last day when it has fewer days.
    fn move_to(&mut self, year: i32, month: u32) {
        let day = self.date.day().min(days_in_month(year, month));
        if let Some(date) = NaiveDate::from_ymd_opt(year, month, day) {
            self.date = date;
            self.edited = true;
        }
    }

    fn pick(&mut self, date: NaiveDate) {
        self.date = date;
        self.edited = true;
    }

    fn datetime_text(&self) -> String {
        format!(
            "{} {:02}:{:02}:{:02}",
            self.date.format("%Y-%m-%d"),
            self.hour,
            self.minute,
            self.second
        )
    }
}

/// everything loaded from the server and what you are doing to it.
struct Loaded {
    state: ClockState,
    zones: Zones,
    read_at: Instant,
    draft: Draft,
    /// the region chosen in the time zone tab.
    region: String,
    /// what the chosen zone looks like on the server, and the job asking.
    preview: Option<(String, ZoneNow)>,
    asking: Option<(String, Job<ZoneNow>)>,
}

impl Loaded {
    fn new(data: &ClockData) -> Self {
        let draft = Draft::from_state(&data.state, 0);
        Self {
            region: Zones::region_of(&draft.zone),
            state: data.state.clone(),
            zones: Zones::new(&data.zones),
            read_at: Instant::now(),
            draft,
            preview: None,
            asking: None,
        }
    }

    fn elapsed(&self) -> i64 {
        i64::try_from(self.read_at.elapsed().as_secs()).unwrap_or(0)
    }

    /// the server's utc time now.
    fn utc_now(&self) -> i64 {
        self.state.utc_secs + self.elapsed()
    }

    /// what applying would do; `None` when nothing was changed.
    fn change(&self) -> Result<Option<Change>, String> {
        let (d, st) = (&self.draft, &self.state);
        let mut change = Change::default();
        if d.zone != st.timezone {
            change.timezone = Some(d.zone.clone());
        }
        let servers = clock::parse_servers(&d.servers).map_err(|e| e.to_string())?;
        let current: Vec<String> = if st.servers.is_empty() {
            vec![DEFAULT_SERVERS.to_string()]
        } else {
            st.servers.clone()
        };
        let servers_changed = st.sync == Sync::Timesyncd && servers != current;
        if !st.container && (d.ntp != st.ntp || servers_changed) {
            change.ntp = Some(NtpChange {
                enabled: d.ntp,
                servers,
            });
        }
        if d.edited && !st.container {
            if d.ntp {
                return Err(NO_NTP_WITH_TIME.into());
            }
            change.time = Some(d.datetime_text());
        }
        Ok((change != Change::default()).then_some(change))
    }

    fn dirty(&self) -> bool {
        !matches!(self.change(), Ok(None))
    }

    /// can the date and time be edited here?
    fn time_editable(&self) -> bool {
        !self.state.container && !self.draft.ntp
    }

    /// follow the server's clock, and the answer about the zone that was asked for.
    fn live(&mut self) {
        let elapsed = self.elapsed();
        self.draft.follow(&self.state, elapsed);
        if let Some((zone, job)) = self.asking.take() {
            match job.poll() {
                None => self.asking = Some((zone, job)),
                Some(Ok(now)) => self.preview = Some((zone, now)),
                Some(Err(_)) => {}
            }
        }
    }
}

/// the days of the month shown, as a grid of seven columns starting on Sunday.
pub fn month_days(year: i32, month: u32) -> Vec<NaiveDate> {
    let Some(first) = NaiveDate::from_ymd_opt(year, month, 1) else {
        return Vec::new();
    };
    let lead = i64::from(first.weekday().num_days_from_sunday());
    let start = first - chrono::TimeDelta::days(lead);
    (0..42)
        .map(|i| start + chrono::TimeDelta::days(i))
        .collect()
}

#[derive(Default)]
pub struct ClockPanel {
    remote: Remote<ClockData>,
    tab: Tab,
    loaded: Option<Loaded>,
    /// close the window when the job in flight finishes (OK).
    close_after: bool,
    /// the window should close now.
    close: bool,
    /// a reading to hand to the taskbar clock.
    fresh: Option<ClockState>,
}

impl ClockPanel {
    /// open on the tab numbered `n` (0 date and time, 1 time zone, 2 internet time).
    pub fn open_tab(&mut self, n: &str) {
        self.tab = match n {
            "1" => Tab::Zone,
            "2" => Tab::Internet,
            _ => Tab::DateTime,
        };
    }

    /// forget what was loaded, so opening the window reads the server again.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// pick up a finished load; the reading, when there is a new one, is for the taskbar clock.
    pub fn poll(&mut self) -> Option<ClockState> {
        if self.remote.poll() {
            if let Some(data) = &self.remote.data {
                self.loaded = Some(Loaded::new(data));
                self.fresh = Some(data.state.clone());
                if let Some(text) = data.kept.as_ref().and_then(kept_text) {
                    self.remote.say(false, &text);
                }
            }
            self.close = std::mem::take(&mut self.close_after);
        } else if self.remote.note.as_ref().is_some_and(|(error, _)| *error) {
            self.close_after = false;
        }
        self.fresh.take()
    }

    fn reload(&mut self, services: &Services, host: &str) {
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, _| load(gateway, &target, None));
        self.remote.track(
            host,
            job,
            "reading the server's clock...",
            "read the clock",
            true,
        );
    }

    fn apply(&mut self, services: &Services, host: &str, close: bool) {
        let change = match self.loaded.as_ref().map(Loaded::change) {
            Some(Ok(Some(change))) => change,
            Some(Ok(None)) if close => {
                self.close = true;
                return;
            }
            Some(Ok(None)) => {
                self.remote.say(false, "nothing to change");
                return;
            }
            Some(Err(message)) => {
                self.remote.say(true, message);
                return;
            }
            None => return,
        };
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| {
            let (_, kept) = gateway.clock_apply(&target, &change, ask)?;
            load(gateway, &target, Some(kept))
        });
        self.close_after = close && job.is_some();
        self.remote
            .track(host, job, "applying...", "applied", false);
    }

    fn update_now(&mut self, services: &Services, host: &str) {
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| {
            gateway.clock_update_now(&target, ask)?;
            load(gateway, &target, None)
        });
        self.remote
            .track(host, job, "asking the time servers...", "updated", false);
    }

    /// should the window close now?
    pub fn closing(&self) -> bool {
        self.close
    }

    /// what may happen on `host` now.
    pub fn gates(&self, services: &Services, host: &str) -> Gates {
        let idle = !services.busy(host) && !self.remote.busy();
        let Some(loaded) = &self.loaded else {
            return Gates::default();
        };
        let usable = !loaded.state.container;
        Gates {
            ok: idle,
            apply: idle && loaded.dirty(),
            time: loaded.time_editable(),
            ntp: usable,
            servers: usable && loaded.state.sync != Sync::Chrony,
            update_now: idle && usable && loaded.draft.ntp,
        }
    }

    /// the window for `host`; reads the clock when it shows another server's.
    pub fn view(&mut self, services: &Services, host: Option<&str>) -> Vec<Node<Msg>> {
        let Some(host) = host else {
            return vec![Node::Text(
                "No server yet. Sign in to a server first.".into(),
            )];
        };
        let dirty = self.loaded.as_ref().is_some_and(Loaded::dirty);
        if self.remote.wants_load(host, dirty) {
            self.reload(services, host);
        }
        let gates = self.gates(services, host);
        let mut nodes = Vec::new();
        match self.loaded.as_mut() {
            Some(loaded) => {
                loaded.live();
                let page = match self.tab {
                    Tab::DateTime => loaded.date_time_tab(gates),
                    Tab::Zone => loaded.zone_tab(services, host),
                    Tab::Internet => loaded.internet_tab(gates),
                };
                nodes.push(Node::Tabs {
                    titles: view::titles(&["Date & Time", "Time Zone", "Internet Time"]),
                    current: self.tab as usize,
                    pick: Msg::Tab,
                    body: page,
                });
            }
            None => nodes.push(Node::Text("Reading the server's clock...".into())),
        }
        nodes.extend([
            Node::Gap(6.0),
            Node::Buttons(vec![
                Button::new("Apply", gates.apply, Msg::Apply),
                Button::new("Cancel", true, Msg::Cancel),
                Button::new("OK", gates.ok, Msg::Ok),
            ]),
            Node::Gap(4.0),
            view::status(&self.remote),
        ]);
        nodes
    }

    /// act on a click or keystroke for `host`; true when the window should close.
    pub fn update(&mut self, msg: Msg, services: &Services, host: &str) -> bool {
        // the same rules as the buttons: a front end may send any message
        let gates = self.gates(services, host);
        match msg {
            Msg::Tab(n) => {
                self.tab = match n {
                    1 => Tab::Zone,
                    2 => Tab::Internet,
                    _ => Tab::DateTime,
                };
            }
            Msg::Ok if gates.ok => self.apply(services, host, true),
            Msg::Apply if gates.apply => self.apply(services, host, false),
            Msg::Cancel => self.close = true,
            Msg::UpdateNow if gates.update_now => self.update_now(services, host),
            _ => {
                if let Some(loaded) = self.loaded.as_mut() {
                    loaded.edit(msg, gates);
                }
            }
        }
        self.close
    }
}

// the tabs
impl Loaded {
    fn container_note(&self) -> Vec<Node<Msg>> {
        if self.state.container {
            vec![Node::Text(clock::CONTAINER_NOTE.into()), Node::Gap(4.0)]
        } else {
            Vec::new()
        }
    }

    /// act on an edit, when the window allows it now.
    fn edit(&mut self, msg: Msg, gates: Gates) {
        let d = &mut self.draft;
        let (year, month) = (d.date.year(), d.date.month());
        match msg {
            Msg::Month(m) if gates.time => d.move_to(year, u32::try_from(m).unwrap_or(1)),
            Msg::Year(dir) if gates.time => d.move_to(year + dir, month),
            Msg::Day(date) if gates.time => d.pick(date),
            Msg::Hour(dir) if gates.time => d.bump_hour(dir),
            Msg::Minute(dir) if gates.time => d.bump_minute(dir),
            Msg::Second(dir) if gates.time => d.bump_second(dir),
            Msg::AmPm(_) if gates.time => d.toggle_pm(),
            Msg::Region(i) => {
                if let Some(region) = self.zones.regions.get(i).cloned() {
                    if region != self.region {
                        if let Some(first) =
                            self.zones.by_region.get(&region).and_then(|z| z.first())
                        {
                            self.draft.zone.clone_from(first);
                        }
                        self.region = region;
                    }
                }
            }
            Msg::City(i) => {
                if let Some(zone) = self
                    .zones
                    .by_region
                    .get(&self.region)
                    .and_then(|z| z.get(i))
                {
                    self.draft.zone.clone_from(zone);
                }
            }
            Msg::ToggleNtp if gates.ntp => d.ntp = !d.ntp,
            Msg::Servers(text) if gates.servers => d.servers = text,
            _ => {}
        }
    }

    fn date_time_tab(&self, gates: Gates) -> Vec<Node<Msg>> {
        let mut nodes = self.container_note();
        let d = &self.draft;
        let (year, month) = (d.date.year(), d.date.month());
        let today = local_datetime(self.utc_now(), self.state.offset_secs).date();
        let editable = gates.time;
        let month_choice = Node::Choice {
            id: "clock-month",
            current: MONTHS[(month - 1) as usize].into(),
            width: 92.0,
            options: MONTHS
                .iter()
                .enumerate()
                .map(|(i, name)| (i + 1, (*name).to_string()))
                .collect(),
            pick: Msg::Month,
        };
        let spin = |id: &'static str, text: String, width: f32, step: fn(i32) -> Msg| Node::Spin {
            id,
            text,
            width,
            enabled: editable,
            step,
        };
        let date = vec![
            Node::Row(vec![
                if editable {
                    month_choice
                } else {
                    Node::Disabled(vec![month_choice])
                },
                spin("year", year.to_string(), 38.0, Msg::Year),
            ]),
            Node::Gap(4.0),
            Node::Calendar {
                id: "clock-days",
                year,
                month,
                selected: d.date,
                today,
                enabled: editable,
                pick: Msg::Day,
            },
        ];
        let time = vec![
            Node::ClockFace {
                hour: d.hour,
                minute: d.minute,
                second: d.second,
                size: 118.0,
            },
            Node::Gap(6.0),
            Node::Row(vec![
                spin("hour", d.hour12().to_string(), 22.0, Msg::Hour),
                spin("minute", format!("{:02}", d.minute), 22.0, Msg::Minute),
                spin("second", format!("{:02}", d.second), 22.0, Msg::Second),
                spin(
                    "ampm",
                    if d.pm() { "PM" } else { "AM" }.into(),
                    24.0,
                    Msg::AmPm,
                ),
            ]),
        ];
        nodes.push(Node::Columns(vec![
            (
                Some(205.0),
                vec![Node::Group {
                    title: "Date".into(),
                    body: date,
                }],
            ),
            (
                None,
                vec![Node::Group {
                    title: "Time".into(),
                    body: time,
                }],
            ),
        ]));
        if !editable && !self.state.container {
            nodes.push(Node::Gap(4.0));
            nodes.push(Node::Text("Automatic synchronization is on; turn it off on the Internet Time tab to set the time by hand.".into()));
        }
        nodes.push(Node::Gap(4.0));
        nodes.push(Node::Text(format!(
            "Time zone: {}  (UTC{})",
            self.draft.zone,
            clock::offset_text(self.state.offset_secs)
        )));
        nodes
    }

    fn zone_tab(&mut self, services: &Services, host: &str) -> Vec<Node<Msg>> {
        let cities = self
            .zones
            .by_region
            .get(&self.region)
            .cloned()
            .unwrap_or_default();
        let mut nodes = vec![
            Node::Group {
                title: "Time zone".into(),
                body: vec![
                    Node::Labelled {
                        label: "Region".into(),
                        body: vec![Node::Choice {
                            id: "clock-region",
                            current: self.region.clone(),
                            width: 200.0,
                            options: self.zones.regions.iter().cloned().enumerate().collect(),
                            pick: Msg::Region,
                        }],
                    },
                    Node::Labelled {
                        label: "City".into(),
                        body: vec![Node::Choice {
                            id: "clock-city",
                            current: Zones::city(&self.draft.zone).to_string(),
                            width: 200.0,
                            options: cities
                                .iter()
                                .enumerate()
                                .map(|(i, z)| (i, Zones::city(z).to_string()))
                                .collect(),
                            pick: Msg::City,
                        }],
                    },
                ],
            },
            Node::Gap(6.0),
        ];
        nodes.extend(self.preview(services, host));
        nodes
    }

    /// what the chosen zone looks like on the server now: its offset and the time there.
    fn preview(&mut self, services: &Services, host: &str) -> Vec<Node<Msg>> {
        let zone = self.draft.zone.clone();
        let known = if zone == self.state.timezone {
            Some(ZoneNow {
                offset_secs: self.state.offset_secs,
                abbrev: String::new(),
            })
        } else {
            self.preview
                .as_ref()
                .filter(|(z, _)| *z == zone)
                .map(|(_, now)| now.clone())
        };
        let mut nodes = Vec::new();
        match known {
            Some(now) => {
                let abbrev = if now.abbrev.is_empty() {
                    String::new()
                } else {
                    format!(", {}", now.abbrev)
                };
                nodes.push(Node::Text(format!(
                    "{zone}: UTC{}{abbrev}. It is {} there.",
                    clock::offset_text(now.offset_secs),
                    clock::local_text(self.utc_now(), now.offset_secs)
                )));
            }
            None => {
                nodes.push(Node::Text(format!("{zone}: asking the server...")));
                self.ask_zone(services, host, &zone);
            }
        }
        if zone != self.state.timezone {
            nodes.push(Node::Gap(2.0));
            nodes.push(Node::Text(format!(
                "The server's time zone is {} now; Apply changes it.",
                self.state.timezone
            )));
        }
        nodes
    }

    fn ask_zone(&mut self, services: &Services, host: &str, zone: &str) {
        if self.asking.is_some() {
            return;
        }
        let (target, name) = (host.to_string(), zone.to_string());
        let job = services.quiet(move |gateway| gateway.clock_zone_now(&target, &name));
        self.asking = Some((zone.to_string(), job));
    }

    fn internet_tab(&self, gates: Gates) -> Vec<Node<Msg>> {
        let mut nodes = self.container_note();
        let check = Node::Check {
            label: "Automatically synchronize with an Internet time server".into(),
            checked: self.draft.ntp,
            enabled: gates.ntp,
            toggle: Msg::ToggleNtp,
        };
        let server = Node::Labelled {
            label: "Server".into(),
            body: vec![Node::Input {
                value: self.draft.servers.clone(),
                size: Size::Points(220.0),
                focus: false,
                hint: String::new(),
                edit: Box::new(Msg::Servers),
                enter: None,
            }],
        };
        let mut body = vec![
            check,
            Node::Gap(4.0),
            if gates.servers {
                server
            } else {
                Node::Disabled(vec![server])
            },
            Node::Text("Separate several servers with spaces.".into()),
            Node::Gap(6.0),
            Node::Button(Button::new("Update now", gates.update_now, Msg::UpdateNow)),
        ];
        if !gates.ntp {
            body = vec![Node::Disabled(body)];
        }
        nodes.extend(body);
        nodes.push(Node::Gap(6.0));
        nodes.push(Node::Group {
            title: "Status".into(),
            body: self.status(),
        });
        nodes
    }

    fn status(&self) -> Vec<Node<Msg>> {
        if self.state.container {
            return vec![Node::Text(
                "The clock follows the host this container runs on.".into(),
            )];
        }
        let service = match self.state.sync {
            Sync::Timesyncd => "systemd-timesyncd",
            Sync::Chrony => "chrony",
            Sync::None => "no time service is running",
        };
        let sync = if self.state.synchronized {
            "synchronized"
        } else {
            "not synchronized"
        };
        let mut nodes = vec![
            Node::Text(format!("Service: {service}")),
            Node::Text(format!(
                "Internet time is {}; the clock is {sync}.",
                if self.state.ntp { "on" } else { "off" }
            )),
        ];
        nodes.extend(self.state.status.iter().cloned().map(Node::Text));
        nodes
    }
}

/// what the desktop tests need to look at and to do without a mouse on a drop-down.
impl ClockPanel {
    pub fn is_loaded(&self) -> bool {
        self.loaded.is_some()
    }

    pub fn pretend_zone(&mut self, zone: &str) {
        if let Some(loaded) = self.loaded.as_mut() {
            loaded.draft.zone = zone.to_string();
            loaded.region = Zones::region_of(zone);
        }
    }

    pub fn pretend_ntp(&mut self, on: bool, servers: &str) {
        if let Some(loaded) = self.loaded.as_mut() {
            loaded.draft.ntp = on;
            loaded.draft.servers = servers.to_string();
        }
    }

    pub fn note(&self) -> Option<String> {
        self.remote.note.as_ref().map(|(_, text)| text.clone())
    }
}

impl ClockFeed {
    pub fn has_reading(&self) -> bool {
        self.state.is_some()
    }

    pub fn job_is_idle(&self) -> bool {
        self.job.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> ClockState {
        ClockState {
            timezone: "Europe/Berlin".into(),
            utc_secs: 1_794_759_123,
            offset_secs: 3600,
            ntp: true,
            synchronized: true,
            container: false,
            sync: Sync::Timesyncd,
            servers: Vec::new(),
            status: Vec::new(),
        }
    }

    fn loaded(state: ClockState) -> Loaded {
        Loaded::new(&ClockData {
            state,
            zones: [
                "Europe/Berlin",
                "Europe/Paris",
                "America/Argentina/Buenos_Aires",
                "UTC",
            ]
            .map(String::from)
            .to_vec(),
            kept: None,
        })
    }

    #[test]
    fn stepping_wraps() {
        assert_eq!(step(12, 1, 1, 12), 1);
        assert_eq!(step(1, -1, 1, 12), 12);
        assert_eq!(step(59, 1, 0, 59), 0);
        assert_eq!(step(0, -1, 0, 59), 59);
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2026, 12), 31);
        assert_eq!(days_in_month(2026, 11), 30);
    }

    #[test]
    fn the_draft_starts_on_the_servers_clock_and_follows_it_until_edited() {
        let mut draft = Draft::from_state(&state(), 0);
        assert_eq!(draft.datetime_text(), "2026-11-15 17:12:03");
        assert_eq!((draft.hour12(), draft.pm()), (5, true));
        draft.follow(&state(), 60);
        assert_eq!(draft.datetime_text(), "2026-11-15 17:13:03");
        draft.bump_minute(1);
        assert!(draft.edited);
        draft.follow(&state(), 600);
        assert_eq!(
            draft.datetime_text(),
            "2026-11-15 17:14:03",
            "edited: it stops"
        );
    }

    #[test]
    fn the_spinners_keep_am_and_pm_and_the_days_stay_valid() {
        let mut draft = Draft::from_state(&state(), 0);
        draft.bump_hour(1);
        assert_eq!((draft.hour12(), draft.pm()), (6, true));
        draft.toggle_pm();
        assert_eq!((draft.hour, draft.pm()), (6, false));
        draft.hour = 12;
        draft.bump_hour(1);
        assert_eq!((draft.hour12(), draft.pm()), (1, true));
        draft.hour = 0;
        assert_eq!(draft.hour12(), 12);
        draft.bump_second(-1);
        assert_eq!(draft.second, 2);
        draft.date = NaiveDate::from_ymd_opt(2026, 1, 31).unwrap();
        draft.move_to(2026, 2);
        assert_eq!(draft.date, NaiveDate::from_ymd_opt(2026, 2, 28).unwrap());
        draft.move_to(2027, 13);
        assert_eq!(
            draft.date.year(),
            2026,
            "an impossible month changes nothing"
        );
    }

    #[test]
    fn the_month_grid_starts_on_a_sunday_and_has_six_weeks() {
        let days = month_days(2026, 11);
        assert_eq!(days.len(), 42);
        assert_eq!(days[0].weekday().num_days_from_sunday(), 0);
        assert!(days.contains(&NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()));
        assert!(month_days(2026, 13).is_empty());
    }

    #[test]
    fn zones_are_grouped_by_region() {
        let zones = Zones::new(
            &[
                "Europe/Berlin",
                "UTC",
                "America/Argentina/Buenos_Aires",
                "Europe/Paris",
            ]
            .map(String::from),
        );
        assert_eq!(zones.regions, ["America", "Europe", "Other"]);
        assert_eq!(zones.by_region["Europe"], ["Europe/Berlin", "Europe/Paris"]);
        assert_eq!(
            Zones::city("America/Argentina/Buenos_Aires"),
            "Argentina/Buenos_Aires"
        );
        assert_eq!(Zones::city("UTC"), "UTC");
        assert_eq!(Zones::region_of("UTC"), "Other");
    }

    #[test]
    fn nothing_edited_means_nothing_to_apply() {
        let loaded = loaded(state());
        assert_eq!(loaded.change(), Ok(None));
        assert!(!loaded.dirty());
    }

    #[test]
    fn a_zone_change_is_only_the_zone() {
        let mut loaded = loaded(state());
        loaded.draft.zone = "Europe/Paris".into();
        let change = loaded.change().unwrap().unwrap();
        assert_eq!(change.timezone.as_deref(), Some("Europe/Paris"));
        assert!(change.time.is_none() && change.ntp.is_none());
    }

    #[test]
    fn internet_time_changes_carry_the_servers() {
        let mut loaded = loaded(state());
        loaded.draft.ntp = false;
        let ntp = loaded.change().unwrap().unwrap().ntp.unwrap();
        assert!(!ntp.enabled);
        assert_eq!(ntp.servers, ["pool.ntp.org"]);
        loaded.draft.ntp = true;
        loaded.draft.servers = "a.org, b.org".into();
        let ntp = loaded.change().unwrap().unwrap().ntp.unwrap();
        assert_eq!(ntp.servers, ["a.org", "b.org"]);
        loaded.draft.servers = "bad;name".into();
        assert!(loaded.change().is_err());
        assert!(loaded.dirty(), "a bad entry still counts as an edit");
    }

    #[test]
    fn setting_the_time_needs_internet_time_off() {
        let mut loaded = loaded(state());
        loaded.draft.bump_minute(1);
        assert!(!loaded.time_editable());
        assert_eq!(loaded.change(), Err(NO_NTP_WITH_TIME.to_string()));
        loaded.draft.ntp = false;
        let change = loaded.change().unwrap().unwrap();
        assert_eq!(change.time.as_deref(), Some("2026-11-15 17:13:03"));
        assert!(!change.ntp.unwrap().enabled, "turning it off goes first");
    }

    #[test]
    fn a_container_only_lets_the_zone_change() {
        let mut container = state();
        container.container = true;
        container.ntp = false;
        let mut loaded = loaded(container);
        loaded.draft.ntp = true;
        loaded.draft.bump_minute(1);
        loaded.draft.zone = "UTC".into();
        let change = loaded.change().unwrap().unwrap();
        assert_eq!(change.timezone.as_deref(), Some("UTC"));
        assert!(change.ntp.is_none() && change.time.is_none());
        assert!(!loaded.time_editable());
    }

    #[test]
    fn chrony_servers_are_not_edited_here() {
        let mut chrony = state();
        chrony.sync = Sync::Chrony;
        let mut loaded = loaded(chrony);
        loaded.draft.servers = "other.org".into();
        assert_eq!(loaded.change(), Ok(None));
    }

    #[test]
    fn the_feed_shows_the_servers_time_once_read_and_the_local_time_before() {
        let mut feed = ClockFeed::default();
        assert!(feed.tooltip().contains("This computer"));
        feed.set("me@gw", state());
        assert_eq!(feed.text(), "5:12 PM");
        assert!(feed.tooltip().contains("me@gw") && feed.tooltip().contains("Europe/Berlin"));
        feed.refresh();
        assert!(feed.stale && feed.failed.is_none());
    }

    #[test]
    fn the_rules_hold_for_messages_too() {
        use std::sync::Arc;

        use crate::gateway::Fake;
        use crate::wake::Wake;

        let services = Services::new(Arc::new(Fake::new()), Wake::none());
        let mut panel = ClockPanel {
            loaded: Some(loaded(state())),
            ..ClockPanel::default()
        };
        // internet time is on: the time cannot be set by hand
        let gates = panel.gates(&services, "a@b");
        assert!(!gates.time && !gates.apply && gates.update_now);
        let before = panel.loaded.as_ref().unwrap().draft.hour;
        panel.update(Msg::Hour(1), &services, "a@b");
        let loaded = panel.loaded.as_ref().unwrap();
        assert_eq!(loaded.draft.hour, before);
        assert!(!loaded.draft.edited);
        // nothing changed: Apply does nothing, Cancel closes
        assert!(!panel.update(Msg::Apply, &services, "a@b"));
        assert!(panel.update(Msg::Cancel, &services, "a@b"));
    }
}
