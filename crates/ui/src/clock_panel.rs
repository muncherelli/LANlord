//! the server's clock: the time shown beside the taskbar, and the Date/Time Properties window
//! you open by double-clicking it. both act on the server you are signed in to, not on this
//! computer.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};
use eframe::egui::{self, Align2, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};
use lanlord_core::clock::{self, Change, ClockState, NtpChange, Stored, Sync, ZoneNow};

use crate::controls;
use crate::gateway::Gateway;
use crate::remote::{Remote, RED};
use crate::services::Services;
use crate::tasks::Job;
use crate::theme::{self, BLACK, GRAY, NAVY, SHADOW, WHITE};

const MONTHS: [&str; 12] = [
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
const WEEKDAYS: [&str; 7] = ["S", "M", "T", "W", "T", "F", "S"];
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

// the window

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    DateTime,
    Zone,
    Internet,
}

/// what the window loads: the server's clock and the zones it knows, and whether the last
/// change was kept in the settings.
pub struct ClockData {
    pub state: ClockState,
    pub zones: Vec<String>,
    pub kept: Option<Stored>,
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

/// a number in a sunken box with an up and a down button; returns which button was pressed.
fn spin(ui: &mut Ui, id: &str, text: &str, width: f32, enabled: bool) -> i32 {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(width + 15.0, controls::FIELD_HEIGHT),
        Sense::hover(),
    );
    let field = Rect::from_min_size(rect.min, Vec2::new(width, rect.height()));
    ui.painter().rect_filled(field, 0.0, WHITE);
    theme::bevel(ui.painter(), field, false);
    let colour = if enabled { BLACK } else { SHADOW };
    ui.painter().text(
        field.center(),
        Align2::CENTER_CENTER,
        text,
        theme::font(),
        colour,
    );
    let half = (rect.height() - 4.0) / 2.0;
    let mut pressed = 0;
    for (dir, name) in [(1, "up"), (-1, "down")] {
        let top = if dir == 1 {
            rect.top() + 2.0
        } else {
            rect.top() + 2.0 + half
        };
        let button = Rect::from_min_size(Pos2::new(field.right(), top), Vec2::new(14.0, half));
        let sense = if enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let response = ui.interact(button, ui.scope_id().with(("clock-spin", id, name)), sense);
        let down = response.is_pointer_button_down_on();
        ui.painter().rect_filled(button, 0.0, GRAY);
        theme::bevel(ui.painter(), button, !down);
        arrow(ui, button, dir == 1, enabled);
        if response.clicked() {
            pressed = dir;
        }
        #[cfg(test)]
        theme::probe::record(ui, &format!("{id} {name}"), button, enabled);
    }
    pressed
}

fn arrow(ui: &Ui, rect: Rect, up: bool, enabled: bool) {
    let c = rect.center();
    let tip = if up { -2.0 } else { 2.0 };
    let colour = if enabled { BLACK } else { SHADOW };
    ui.painter().add(Shape::convex_polygon(
        vec![
            Pos2::new(c.x - 3.0, c.y - tip / 2.0),
            Pos2::new(c.x + 3.0, c.y - tip / 2.0),
            Pos2::new(c.x, c.y + tip / 2.0),
        ],
        colour,
        Stroke::NONE,
    ));
}

/// the clock face: ticks, an hour and a minute hand and a red second hand.
fn analog(ui: &mut Ui, size: f32, (h, m, s): (u32, u32, u32)) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let (centre, radius) = (rect.center(), size / 2.0 - 2.0);
    let painter = ui.painter();
    painter.circle_filled(centre, radius, WHITE);
    painter.circle_stroke(centre, radius, Stroke::new(1.0, SHADOW));
    let direction = |degrees: f32| {
        let a = degrees.to_radians();
        Vec2::new(a.sin(), -a.cos())
    };
    for i in 0..12 {
        let long = i % 3 == 0;
        let (from, to) = (radius - if long { 9.0 } else { 5.0 }, radius - 3.0);
        let d = direction(i as f32 * 30.0);
        let width = if long { 2.0 } else { 1.0 };
        painter.line_segment(
            [centre + d * from, centre + d * to],
            Stroke::new(width, BLACK),
        );
    }
    let hands = [
        (((h % 12) as f32 + m as f32 / 60.0) * 30.0, 0.5, 3.0, BLACK),
        ((m as f32 + s as f32 / 60.0) * 6.0, 0.78, 2.0, BLACK),
        (s as f32 * 6.0, 0.82, 1.0, RED),
    ];
    for (degrees, length, width, colour) in hands {
        painter.line_segment(
            [centre, centre + direction(degrees) * radius * length],
            Stroke::new(width, colour),
        );
    }
    painter.circle_filled(centre, 2.5, BLACK);
}

/// the days of the month shown, as a grid of seven columns starting on Sunday.
fn month_days(year: i32, month: u32) -> Vec<NaiveDate> {
    let Some(first) = NaiveDate::from_ymd_opt(year, month, 1) else {
        return Vec::new();
    };
    let lead = i64::from(first.weekday().num_days_from_sunday());
    let start = first - chrono::TimeDelta::days(lead);
    (0..42)
        .map(|i| start + chrono::TimeDelta::days(i))
        .collect()
}

/// what a click on the buttons at the bottom asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Button {
    Ok,
    Cancel,
    Apply,
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
                match data.kept {
                    Some(Stored::Yes) => self.remote.say(false, "applied and kept in the settings"),
                    Some(Stored::NoKey) => self.remote.say(
                        false,
                        "applied; not kept in the settings (no key yet), so an apply may undo it",
                    ),
                    _ => {}
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

    /// draw the window's contents; true when it should be closed.
    pub fn show(&mut self, ui: &mut Ui, services: &Services, host: Option<&str>) -> bool {
        let Some(host) = host else {
            ui.label("No server yet. Sign in to a server first.");
            return false;
        };
        let dirty = self.loaded.as_ref().is_some_and(Loaded::dirty);
        if self.remote.wants_load(host, dirty) {
            self.reload(services, host);
        }
        let idle = !services.busy(host) && !self.remote.busy();
        let mut update = false;
        let mut chosen = self.tab as usize;
        if let Some(loaded) = self.loaded.as_mut() {
            loaded.live();
            ui.ctx().request_repaint_after(Duration::from_millis(250));
            controls::tabs(
                ui,
                &["Date & Time", "Time Zone", "Internet Time"],
                &mut chosen,
                |ui, open| match open {
                    0 => loaded.date_time_tab(ui),
                    1 => loaded.zone_tab(ui, services, host),
                    _ => update = loaded.internet_tab(ui, idle),
                },
            );
        } else {
            ui.label("Reading the server's clock...");
        }
        self.tab = match chosen {
            1 => Tab::Zone,
            2 => Tab::Internet,
            _ => Tab::DateTime,
        };
        ui.add_space(6.0);
        let pressed = self.buttons(ui, idle);
        ui.add_space(4.0);
        self.remote.show_note(ui);
        match pressed {
            Some(Button::Ok) => self.apply(services, host, true),
            Some(Button::Apply) => self.apply(services, host, false),
            Some(Button::Cancel) => self.close = true,
            None => {}
        }
        if update && idle {
            self.update_now(services, host);
        }
        self.close
    }

    fn buttons(&self, ui: &mut Ui, idle: bool) -> Option<Button> {
        let ready = idle && self.loaded.is_some();
        let dirty = self.loaded.as_ref().is_some_and(Loaded::dirty);
        let mut pressed = None;
        controls::buttons(ui, |ui| {
            if theme::action(ui, "Apply", ready && dirty).clicked() {
                pressed = Some(Button::Apply);
            }
            if theme::action(ui, "Cancel", true).clicked() {
                pressed = Some(Button::Cancel);
            }
            if theme::action(ui, "OK", ready).clicked() {
                pressed = Some(Button::Ok);
            }
        });
        pressed
    }
}

// the tabs
impl Loaded {
    fn container_note(&self, ui: &mut Ui) {
        if self.state.container {
            ui.label(clock::CONTAINER_NOTE);
            ui.add_space(4.0);
        }
    }

    fn date_time_tab(&mut self, ui: &mut Ui) {
        self.container_note(ui);
        let editable = self.time_editable();
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(205.0);
                controls::group(ui, "Date", |ui| self.calendar(ui, editable));
            });
            ui.vertical(|ui| {
                controls::group(ui, "Time", |ui| self.time_box(ui, editable));
            });
        });
        if !editable && !self.state.container {
            ui.add_space(4.0);
            ui.label("Automatic synchronization is on; turn it off on the Internet Time tab to set the time by hand.");
        }
        ui.add_space(4.0);
        ui.label(format!(
            "Time zone: {}  (UTC{})",
            self.draft.zone,
            clock::offset_text(self.state.offset_secs)
        ));
    }

    fn calendar(&mut self, ui: &mut Ui, editable: bool) {
        let (year, month) = (self.draft.date.year(), self.draft.date.month());
        ui.horizontal(|ui| {
            let mut picked = month;
            ui.add_enabled_ui(editable, |ui| {
                controls::dropdown(
                    ui,
                    "clock-month",
                    MONTHS[(month - 1) as usize],
                    92.0,
                    |ui| {
                        for (i, name) in MONTHS.iter().enumerate() {
                            let number = i as u32 + 1;
                            ui.selectable_value(&mut picked, number, *name);
                        }
                    },
                );
            });
            if picked != month {
                self.draft.move_to(year, picked);
            }
            let dir = spin(ui, "year", &year.to_string(), 38.0, editable);
            if dir != 0 {
                self.draft.move_to(year + dir, month);
            }
        });
        ui.add_space(4.0);
        self.day_grid(ui, editable);
    }

    fn day_grid(&mut self, ui: &mut Ui, editable: bool) {
        let (year, month) = (self.draft.date.year(), self.draft.date.month());
        let today = local_datetime(self.utc_now(), self.state.offset_secs).date();
        let cell = Vec2::new(26.0, 19.0);
        let (grid, _) =
            ui.allocate_exact_size(Vec2::new(cell.x * 7.0, cell.y * 7.0), Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(grid, 0.0, WHITE);
        theme::bevel(painter, grid, false);
        for (i, name) in WEEKDAYS.iter().enumerate() {
            let at = Rect::from_min_size(grid.min + Vec2::new(cell.x * i as f32, 0.0), cell);
            painter.text(
                at.center(),
                Align2::CENTER_CENTER,
                *name,
                theme::font(),
                SHADOW,
            );
        }
        let mut picked = None;
        for (i, date) in month_days(year, month).into_iter().enumerate() {
            let at = Rect::from_min_size(
                grid.min + Vec2::new(cell.x * (i % 7) as f32, cell.y * (1 + i / 7) as f32),
                cell,
            );
            let (selected, in_month) = (date == self.draft.date, date.month() == month);
            let sense = if editable {
                Sense::click()
            } else {
                Sense::hover()
            };
            let response = ui.interact(at, ui.scope_id().with(("clock-day", date)), sense);
            let painter = ui.painter();
            if selected {
                painter.rect_filled(at.shrink(1.0), 0.0, NAVY);
            }
            let colour = match (selected, in_month) {
                (true, _) => WHITE,
                (false, true) => BLACK,
                (false, false) => SHADOW,
            };
            if date == today && !selected {
                painter.rect_stroke(
                    at.shrink(2.0),
                    0.0,
                    Stroke::new(1.0, BLACK),
                    egui::StrokeKind::Inside,
                );
            }
            painter.text(
                at.center(),
                Align2::CENTER_CENTER,
                date.day().to_string(),
                theme::font(),
                colour,
            );
            if response.clicked() {
                picked = Some(date);
            }
            #[cfg(test)]
            theme::probe::record(
                ui,
                &format!("day {}", date.format("%Y-%m-%d")),
                at,
                editable,
            );
        }
        if let Some(date) = picked {
            self.draft.pick(date);
        }
    }

    fn time_box(&mut self, ui: &mut Ui, editable: bool) {
        let d = &self.draft;
        analog(ui, 118.0, (d.hour, d.minute, d.second));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let hour = spin(ui, "hour", &self.draft.hour12().to_string(), 22.0, editable);
            let minute = spin(
                ui,
                "minute",
                &format!("{:02}", self.draft.minute),
                22.0,
                editable,
            );
            let second = spin(
                ui,
                "second",
                &format!("{:02}", self.draft.second),
                22.0,
                editable,
            );
            let ampm = if self.draft.pm() { "PM" } else { "AM" };
            let toggle = spin(ui, "ampm", ampm, 24.0, editable);
            if hour != 0 {
                self.draft.bump_hour(hour);
            }
            if minute != 0 {
                self.draft.bump_minute(minute);
            }
            if second != 0 {
                self.draft.bump_second(second);
            }
            if toggle != 0 {
                self.draft.toggle_pm();
            }
        });
    }

    fn zone_tab(&mut self, ui: &mut Ui, services: &Services, host: &str) {
        controls::group(ui, "Time zone", |ui| {
            controls::row(ui, "Region", |ui| self.region_box(ui));
            controls::row(ui, "City", |ui| self.city_box(ui));
        });
        ui.add_space(6.0);
        self.preview(ui, services, host);
    }

    fn region_box(&mut self, ui: &mut Ui) {
        let mut region = self.region.clone();
        controls::dropdown(ui, "clock-region", &region.clone(), 200.0, |ui| {
            for name in &self.zones.regions {
                ui.selectable_value(&mut region, name.clone(), name);
            }
        });
        if region != self.region {
            if let Some(first) = self.zones.by_region.get(&region).and_then(|z| z.first()) {
                self.draft.zone.clone_from(first);
            }
            self.region = region;
        }
    }

    fn city_box(&mut self, ui: &mut Ui) {
        let mut zone = self.draft.zone.clone();
        let list = self
            .zones
            .by_region
            .get(&self.region)
            .cloned()
            .unwrap_or_default();
        controls::dropdown(
            ui,
            "clock-city",
            Zones::city(&zone).to_string().as_str(),
            200.0,
            |ui| {
                for name in &list {
                    ui.selectable_value(&mut zone, name.clone(), Zones::city(name));
                }
            },
        );
        self.draft.zone = zone;
    }

    /// what the chosen zone looks like on the server now: its offset and the time there.
    fn preview(&mut self, ui: &mut Ui, services: &Services, host: &str) {
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
        match known {
            Some(now) => {
                let abbrev = if now.abbrev.is_empty() {
                    String::new()
                } else {
                    format!(", {}", now.abbrev)
                };
                ui.label(format!(
                    "{zone}: UTC{}{abbrev}. It is {} there.",
                    clock::offset_text(now.offset_secs),
                    clock::local_text(self.utc_now(), now.offset_secs)
                ));
            }
            None => {
                ui.label(format!("{zone}: asking the server..."));
                self.ask_zone(services, host, &zone);
            }
        }
        if zone != self.state.timezone {
            ui.add_space(2.0);
            ui.label(format!(
                "The server's time zone is {} now; Apply changes it.",
                self.state.timezone
            ));
        }
    }

    fn ask_zone(&mut self, services: &Services, host: &str, zone: &str) {
        if self.asking.is_some() {
            return;
        }
        let (target, name) = (host.to_string(), zone.to_string());
        let job = services.quiet(move |gateway| gateway.clock_zone_now(&target, &name));
        self.asking = Some((zone.to_string(), job));
    }

    /// the internet time tab; true when Update now was pressed.
    fn internet_tab(&mut self, ui: &mut Ui, idle: bool) -> bool {
        self.container_note(ui);
        let usable = !self.state.container;
        let mut update = false;
        ui.add_enabled_ui(usable, |ui| {
            controls::check(
                ui,
                &mut self.draft.ntp,
                "Automatically synchronize with an Internet time server",
            );
            ui.add_space(4.0);
            let own_servers = usable && self.state.sync != Sync::Chrony;
            ui.add_enabled_ui(own_servers, |ui| {
                controls::row(ui, "Server", |ui| {
                    controls::edit(ui, &mut self.draft.servers, 220.0);
                });
            });
            ui.label("Separate several servers with spaces.");
            ui.add_space(6.0);
            update = theme::action(ui, "Update now", idle && usable && self.draft.ntp).clicked();
        });
        ui.add_space(6.0);
        controls::group(ui, "Status", |ui| self.status(ui));
        update
    }

    fn status(&self, ui: &mut Ui) {
        if self.state.container {
            ui.label("The clock follows the host this container runs on.");
            return;
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
        ui.label(format!("Service: {service}"));
        ui.label(format!(
            "Internet time is {}; the clock is {sync}.",
            if self.state.ntp { "on" } else { "off" }
        ));
        for line in &self.state.status {
            ui.label(line);
        }
    }
}

/// what the desktop tests need to look at and to do without a mouse on a drop-down.
#[cfg(test)]
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

#[cfg(test)]
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
}
