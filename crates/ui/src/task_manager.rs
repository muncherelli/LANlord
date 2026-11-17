//! the Task Manager: Processes, Performance and Networking tabs in the style of Windows NT 4's,
//! drawn from what `lanlord_app::monitor` collects, and the tray's network activity icon. any
//! signed-in user may look; ending a process is for administrators (the server checks too).

use eframe::egui::{self, Align2, Color32, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2};
use lanlord_app::monitor::{self, Column, Monitor, Order};
use lanlord_model::monitor::{
    human_bytes, human_rate, History, ProcessRow, Rates, Sample, HISTORY,
};

use crate::controls;
use crate::dialogs;
use crate::theme::{self, BLACK, NAVY, WHITE};

/// the tab the tray icon opens.
pub const NETWORKING: usize = 2;

const GREEN: Color32 = Color32::from_rgb(0, 255, 0);
const GRID: Color32 = Color32::from_rgb(0, 128, 0);
const AMBER: Color32 = Color32::from_rgb(255, 255, 0);
const RED: Color32 = Color32::from_rgb(255, 64, 64);
const CORE_COLOURS: [Color32; 4] = [GREEN, AMBER, Color32::from_rgb(0, 255, 255), RED];
const SCREEN_LIT: Color32 = Color32::from_rgb(0, 255, 255);
const SCREEN_DARK: Color32 = Color32::from_rgb(0, 0, 96);
const ROW: f32 = 17.0;
const WIDTHS: [f32; 5] = [130.0, 50.0, 90.0, 45.0, 85.0];

/// what the window asks the desktop to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Ask {
    End(u32),
}

#[derive(Default)]
pub struct TaskManager {
    pub tab: usize,
    order: Order,
    selected: Option<u32>,
    per_core: bool,
    /// the process an End Process box is asking about, and its name.
    confirm: Option<(u32, String)>,
}

impl TaskManager {
    pub fn show(&mut self, ui: &mut Ui, monitor: &Monitor, admin: bool) -> Option<Ask> {
        let mut ask = None;
        let mut tab = self.tab;
        controls::tabs(
            ui,
            &["Processes", "Performance", "Networking"],
            &mut tab,
            |ui, tab| {
                if !monitor.live() || monitor.history.last.is_none() {
                    let text = monitor
                        .problem
                        .clone()
                        .unwrap_or_else(|| "Reading the host...".into());
                    ui.label(text);
                    return;
                }
                match tab {
                    0 => ask = self.processes(ui, monitor, admin),
                    1 => self.performance(ui, monitor),
                    _ => networking(ui, monitor),
                }
            },
        );
        self.tab = tab;
        if let Some((pid, name)) = self.confirm.clone() {
            match self.confirmation(ui.ctx(), pid, &name) {
                Some(true) => {
                    self.confirm = None;
                    return Some(Ask::End(pid));
                }
                Some(false) => self.confirm = None,
                None => {}
            }
        }
        ask
    }

    fn confirmation(&self, ctx: &egui::Context, pid: u32, name: &str) -> Option<bool> {
        dialogs::modal(ctx, "end-process", "Task Manager Warning", |ui| {
            ui.label(format!(
                "Ending {name} (process {pid}) can make programs lose unsaved data or stop\nworking. Do you want to end it?"
            ));
            ui.add_space(8.0);
            let mut answer = None;
            controls::buttons(ui, |ui| {
                if theme::button(ui, "No", false).clicked() {
                    answer = Some(false);
                }
                if theme::button(ui, "Yes", false).clicked() {
                    answer = Some(true);
                }
            });
            answer
        })
    }

    fn processes(&mut self, ui: &mut Ui, monitor: &Monitor, admin: bool) -> Option<Ask> {
        let history = &monitor.history;
        let Some(procs) = &history.procs else {
            ui.label("Reading the process list...");
            return None;
        };
        let rows = monitor::sorted(&procs.procs, &history.process_cpu, self.order);
        self.process_list(ui, &rows);
        ui.add_space(4.0);
        self.end_button(ui, &procs.procs, admin);
        ui.add_space(4.0);
        status(ui, monitor, rows.len());
        None
    }

    /// the sortable list of processes.
    fn process_list(&mut self, ui: &mut Ui, rows: &[(&ProcessRow, f32)]) {
        let height = (controls::fill_height(ui) - 78.0).max(90.0);
        theme::sunken(ui, WHITE, |ui| {
            ui.set_width(ui.available_width());
            self.heading(ui);
            egui::ScrollArea::vertical()
                .id_salt("task-manager-processes")
                .max_height(height)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height(height);
                    for (row, cpu) in rows {
                        let cells = monitor::cells(row, *cpu);
                        let chosen = self.selected == Some(row.pid);
                        if data_row(ui, &cells, chosen).clicked() {
                            self.selected = Some(row.pid);
                        }
                    }
                });
        });
    }

    /// the End Process button; only an administrator may press it.
    fn end_button(&mut self, ui: &mut Ui, procs: &[ProcessRow], admin: bool) {
        controls::buttons(ui, |ui| {
            let target = self
                .selected
                .and_then(|pid| procs.iter().find(|p| p.pid == pid));
            let end = theme::action(ui, "End Process", admin && target.is_some());
            let end = if admin {
                end
            } else {
                end.on_hover_text("Only administrators can end a process.")
            };
            if let (true, Some(p)) = (end.clicked(), target) {
                self.confirm = Some((p.pid, p.name.clone()));
            }
        });
    }

    /// the heading row; a click sorts by that column, and again flips the order.
    fn heading(&mut self, ui: &mut Ui) {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
        let mut x = rect.left();
        for (i, (column, label)) in monitor::COLUMNS.iter().enumerate() {
            let width = WIDTHS.get(i).copied().unwrap_or(rect.right() - x);
            let width = if i + 1 == monitor::COLUMNS.len() {
                rect.right() - x
            } else {
                width
            };
            let cell = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(width, 20.0));
            let response = ui.interact(cell, ui.scope_id().with(("sort", i)), Sense::click());
            let down = response.is_pointer_button_down_on();
            ui.painter().rect_filled(cell, 0.0, theme::GRAY);
            theme::bevel(ui.painter(), cell, !down);
            let arrow = match (self.order.column == *column, self.order.descending) {
                (true, true) => " v",
                (true, false) => " ^",
                _ => "",
            };
            ui.painter().with_clip_rect(cell.shrink(2.0)).text(
                cell.left_center() + Vec2::new(5.0, 0.0),
                Align2::LEFT_CENTER,
                format!("{label}{arrow}"),
                theme::font(),
                BLACK,
            );
            if response.clicked() {
                self.order.click(*column);
            }
            x += width;
        }
    }

    fn performance(&mut self, ui: &mut Ui, monitor: &Monitor) {
        let history = &monitor.history;
        let Some(sample) = &history.last else { return };
        self.cpu_group(ui, history, sample.cpu_per_core.len());
        memory_group(ui, history, sample);
        totals_group(ui, history, sample);
    }

    fn cpu_group(&mut self, ui: &mut Ui, history: &History, cores: usize) {
        let cpu_now = history.now().map_or(0.0, |r| r.cpu_total);
        let cpu_line: Vec<f32> = history.rates.iter().map(|r| r.cpu_total).collect();
        controls::group(ui, "CPU Usage History", |ui| {
            ui.horizontal(|ui| {
                meter(ui, cpu_now);
                if self.per_core && cores > 1 {
                    let rates: Vec<&Rates> = history.rates.iter().collect();
                    per_core_graphs(ui, &rates, cores);
                } else {
                    let size = Vec2::new(ui.available_width(), 80.0);
                    graph(ui, size, &[(GREEN, cpu_line)]);
                }
            });
            if cores > 1 {
                controls::check(ui, &mut self.per_core, "Show each core separately");
            }
        });
    }
}

fn networking(ui: &mut Ui, monitor: &Monitor) {
    let history = &monitor.history;
    let names = monitor::interfaces(history);
    if names.is_empty() {
        ui.label("No network interfaces.");
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("task-manager-networking")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for name in names {
                let rx: Vec<f64> = series(&history.rates, &name, |n| n.rx_bytes_per_s);
                let tx: Vec<f64> = series(&history.rates, &name, |n| n.tx_bytes_per_s);
                // the scale grows with the traffic and never drops below 100 KB/s
                let peak = rx.iter().chain(&tx).cloned().fold(100.0 * 1024.0, f64::max);
                let norm = |v: &[f64]| v.iter().map(|x| (*x / peak) as f32).collect::<Vec<_>>();
                controls::group(ui, &name, |ui| {
                    graph(
                        ui,
                        Vec2::new(ui.available_width(), 70.0),
                        &[(GREEN, norm(&rx)), (RED, norm(&tx))],
                    );
                    ui.label(format!(
                        "Receiving {}    Sending {}    Scale {}",
                        human_rate(rx.last().copied().unwrap_or(0.0)),
                        human_rate(tx.last().copied().unwrap_or(0.0)),
                        human_rate(peak)
                    ));
                });
            }
        });
}

fn series(
    rates: &std::collections::VecDeque<Rates>,
    name: &str,
    pick: fn(&lanlord_model::monitor::IfaceRate) -> f64,
) -> Vec<f64> {
    rates
        .iter()
        .map(|r| r.net.iter().find(|n| n.name == name).map_or(0.0, pick))
        .collect()
}

fn data_row(ui: &mut Ui, cells: &[String], selected: bool) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
    if selected {
        ui.painter().rect_filled(rect, 0.0, NAVY);
    }
    let colour = if selected { WHITE } else { BLACK };
    let mut x = 0.0;
    for (i, cell) in cells.iter().enumerate() {
        let width = WIDTHS.get(i).copied().unwrap_or(0.0);
        let right = if i + 1 == cells.len() {
            rect.right()
        } else {
            rect.left() + x + width
        };
        let column = Rect::from_min_max(
            Pos2::new(rect.left() + x, rect.top()),
            Pos2::new(right, rect.bottom()),
        );
        // numbers line up on their right edge
        let numeric = matches!(
            monitor::COLUMNS[i].0,
            Column::Pid | Column::Cpu | Column::Memory | Column::Threads
        );
        let (at, align) = if numeric {
            (
                Pos2::new(column.right() - 8.0, rect.center().y),
                Align2::RIGHT_CENTER,
            )
        } else {
            (
                Pos2::new(column.left() + 5.0, rect.center().y),
                Align2::LEFT_CENTER,
            )
        };
        ui.painter()
            .with_clip_rect(column)
            .text(at, align, cell, theme::font(), colour);
        x += width;
    }
    response
}

/// a black box with green traces, like the Task Manager's history graphs: each series is
/// values from 0 to 1, the newest at the right edge.
fn graph(ui: &mut Ui, size: Vec2, lines: &[(Color32, Vec<f32>)]) {
    let (rect, _) = ui.allocate_exact_size(size.max(Vec2::new(40.0, 30.0)), Sense::hover());
    paint_graph(ui, rect, lines);
}

fn paint_graph(ui: &Ui, rect: Rect, lines: &[(Color32, Vec<f32>)]) {
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 0.0, BLACK);
    let grid = Stroke::new(1.0, GRID);
    for i in 1..5 {
        let y = rect.top() + rect.height() * i as f32 / 5.0;
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            grid,
        );
    }
    let mut x = rect.right();
    while x > rect.left() {
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            grid,
        );
        x -= 20.0;
    }
    let step = rect.width() / (HISTORY - 1) as f32;
    for (colour, values) in lines {
        let n = values.len();
        let points: Vec<Pos2> = values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                Pos2::new(
                    rect.right() - (n - 1 - i) as f32 * step,
                    rect.bottom() - 1.0 - v.clamp(0.0, 1.0) * (rect.height() - 2.0),
                )
            })
            .collect();
        if points.len() > 1 {
            painter.add(Shape::line(points, Stroke::new(1.0, *colour)));
        }
    }
    controls::thin_bevel(ui, rect, false);
}

/// one small graph per cpu core, two to a row.
fn per_core_graphs(ui: &mut Ui, rates: &[&Rates], cores: usize) {
    let width = ui.available_width();
    let cell = Vec2::new((width - 6.0) / 2.0, 38.0);
    ui.vertical(|ui| {
        for pair in 0..cores.div_ceil(2) {
            ui.horizontal(|ui| {
                for core in [pair * 2, pair * 2 + 1] {
                    if core < cores {
                        let line: Vec<f32> = rates
                            .iter()
                            .map(|r| r.cpu_per_core.get(core).copied().unwrap_or(0.0))
                            .collect();
                        graph(ui, cell, &[(CORE_COLOURS[core % CORE_COLOURS.len()], line)]);
                    }
                }
            });
        }
    });
}

/// the big readout beside a graph: the share as a percentage over a bar.
fn meter(ui: &mut Ui, share: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(70.0, 80.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, BLACK);
    let fill = Rect::from_min_max(
        Pos2::new(
            rect.left() + 6.0,
            rect.bottom() - 4.0 - (rect.height() - 24.0) * share.clamp(0.0, 1.0),
        ),
        Pos2::new(rect.right() - 6.0, rect.bottom() - 4.0),
    );
    ui.painter().rect_filled(fill, 0.0, GREEN);
    ui.painter().text(
        Pos2::new(rect.center().x, rect.top() + 10.0),
        Align2::CENTER_CENTER,
        format!("{:.0} %", share * 100.0),
        theme::font(),
        GREEN,
    );
    controls::thin_bevel(ui, rect, false);
}

/// the tray's network activity icon: two small monitors that light up while data is received
/// (the one behind) and sent (the one in front). returns the response for the tooltip and the
/// double click.
pub fn tray_icon(ui: &mut Ui, at: Rect, receiving: bool, sending: bool) -> Response {
    let response = ui.interact(at, ui.scope_id().with("tray-network"), Sense::click());
    let origin = at.center() - Vec2::splat(8.0);
    let monitor = |offset: Vec2, lit: bool| {
        let case = Rect::from_min_size(origin + offset, Vec2::new(10.0, 8.0));
        let painter = ui.painter();
        painter.rect_filled(case, 0.0, theme::GRAY);
        painter.rect_stroke(case, 0.0, Stroke::new(1.0, BLACK), egui::StrokeKind::Inside);
        let screen = case.shrink(2.0);
        painter.rect_filled(screen, 0.0, if lit { SCREEN_LIT } else { SCREEN_DARK });
        // the stand
        let foot = Rect::from_min_size(
            Pos2::new(case.center().x - 2.0, case.bottom()),
            Vec2::new(4.0, 2.0),
        );
        painter.rect_filled(foot, 0.0, theme::SHADOW);
    };
    monitor(Vec2::new(0.0, 0.0), receiving);
    monitor(Vec2::new(6.0, 6.0), sending);
    #[cfg(test)]
    theme::probe::record(ui, "network-activity", at, true);
    response
}

/// the memory graph and its meter.
fn memory_group(ui: &mut Ui, history: &History, sample: &Sample) {
    let used = sample.mem.used() as f32 / sample.mem.total.max(1) as f32;
    controls::group(ui, "Memory Usage History", |ui| {
        ui.horizontal(|ui| {
            meter(ui, used);
            let line: Vec<f32> = history.rates.iter().map(|r| r.mem_used).collect();
            graph(ui, Vec2::new(ui.available_width(), 60.0), &[(GREEN, line)]);
        });
    });
}

/// the counts and the memory and swap totals.
fn totals_group(ui: &mut Ui, history: &History, sample: &Sample) {
    let procs = history.procs.as_ref().map_or(0, |p| p.procs.len());
    let threads: u32 = history
        .procs
        .as_ref()
        .map_or(0, |p| p.procs.iter().map(|p| p.threads).sum());
    let lines = [
        ("Processes", procs.to_string()),
        ("Threads", threads.to_string()),
        ("Load average", load_text(sample)),
        ("Uptime", uptime_text(sample.uptime)),
        ("Physical memory", memory_text(sample)),
        ("Swap", swap_text(sample)),
    ];
    controls::group(ui, "Totals", |ui| {
        for (label, value) in lines {
            controls::row(ui, label, |ui| ui.label(value));
        }
    });
}

fn load_text(sample: &Sample) -> String {
    let load = &sample.load;
    format!("{:.2}  {:.2}  {:.2}", load.one, load.five, load.fifteen)
}

fn uptime_text(uptime: f64) -> String {
    let secs = uptime as u64;
    format!(
        "{}d {:02}:{:02}",
        secs / 86_400,
        secs / 3600 % 24,
        secs / 60 % 60
    )
}

fn memory_text(sample: &Sample) -> String {
    let mem = &sample.mem;
    format!(
        "{} total, {} available, {} cached",
        human_bytes(mem.total as f64),
        human_bytes(mem.available as f64),
        human_bytes(mem.cached as f64)
    )
}

fn swap_text(sample: &Sample) -> String {
    format!(
        "{} of {} used",
        human_bytes(sample.swap.used() as f64),
        human_bytes(sample.swap.total as f64)
    )
}

/// the status bar: the last note, or the totals.
fn status(ui: &mut Ui, monitor: &Monitor, count: usize) {
    if let Some((error, text)) = &monitor.note {
        controls::status_bar(ui, text, *error);
        return;
    }
    let history = &monitor.history;
    let cpu = history.now().map_or(0.0, |r| r.cpu_total * 100.0);
    let mem = history.last.as_ref().map_or(0, |s| s.mem.used());
    let total = history.last.as_ref().map_or(0, |s| s.mem.total);
    let text = format!(
        "Processes: {count}    CPU Usage: {cpu:.0}%    Mem Usage: {} / {}",
        human_bytes(mem as f64),
        human_bytes(total as f64)
    );
    controls::status_bar(ui, &text, false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanlord_app::demo_monitor::Synthetic;
    use lanlord_app::monitor::Backend;
    use std::sync::Arc;

    fn filled() -> Monitor {
        let mut m = Monitor::default();
        m.use_backend(Backend::Demo(Arc::default()));
        let demo = Synthetic::default();
        for i in 0..30u64 {
            m.history.push(demo.sample_at(1_000_000 + i * 1000, true));
        }
        m
    }

    fn run_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        draw: impl FnMut(&mut Ui),
    ) -> Vec<Shape> {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, crate::testkit::SCREEN)),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, draw);
        out.textures_delta.clear();
        out.shapes.into_iter().map(|s| s.shape).collect()
    }

    fn step(
        ctx: &egui::Context,
        manager: &mut TaskManager,
        monitor: &Monitor,
        admin: bool,
        events: Vec<egui::Event>,
    ) -> Option<Ask> {
        let mut ask = None;
        run_frame(ctx, events, |ui| {
            ask = manager.show(ui, monitor, admin).or(ask.take());
        });
        ask
    }

    fn frame(manager: &mut TaskManager, monitor: &Monitor, admin: bool) -> Option<Ask> {
        let ctx = egui::Context::default();
        let mut ask = None;
        for _ in 0..2 {
            ask = step(&ctx, manager, monitor, admin, Vec::new()).or(ask.take());
        }
        ask
    }

    #[test]
    fn every_tab_draws_with_data_and_without() {
        let monitor = filled();
        let mut manager = TaskManager::default();
        for tab in 0..3 {
            manager.tab = tab;
            assert_eq!(frame(&mut manager, &monitor, true), None);
            manager.per_core = true;
            assert_eq!(frame(&mut manager, &monitor, false), None);
        }
        let mut empty = TaskManager::default();
        assert_eq!(frame(&mut empty, &Monitor::default(), false), None);
    }

    /// click the button the last frame drew under `text`; whether it was enabled.
    fn press(
        ctx: &egui::Context,
        manager: &mut TaskManager,
        monitor: &Monitor,
        admin: bool,
        text: &str,
    ) -> (bool, Option<Ask>) {
        let (rect, enabled) = theme::probe::find(ctx, text).unwrap_or_else(|| panic!("{text}"));
        let ask = step(
            ctx,
            manager,
            monitor,
            admin,
            crate::testkit::click(rect.center()),
        );
        (enabled, ask)
    }

    #[test]
    fn only_an_administrator_can_end_a_process_and_only_after_the_warning() {
        let monitor = filled();
        let pid = monitor.history.procs.as_ref().unwrap().procs[1].pid;
        for admin in [true, false] {
            let ctx = egui::Context::default();
            let mut manager = TaskManager::default();
            manager.selected = Some(pid);
            for _ in 0..2 {
                step(&ctx, &mut manager, &monitor, admin, Vec::new());
            }
            let (enabled, ask) = press(&ctx, &mut manager, &monitor, admin, "End Process");
            assert_eq!((enabled, ask), (admin, None));
            // the warning is up for an administrator and never for anyone else
            assert_eq!(manager.confirm.is_some(), admin);
            if !admin {
                continue;
            }
            for _ in 0..2 {
                step(&ctx, &mut manager, &monitor, admin, Vec::new());
            }
            let (_, ask) = press(&ctx, &mut manager, &monitor, admin, "No");
            assert_eq!(ask, None);
            assert!(manager.confirm.is_none(), "No closes the warning");
        }
        // confirmed: End(pid) comes back
        let ctx = egui::Context::default();
        let mut manager = TaskManager::default();
        manager.selected = Some(pid);
        for _ in 0..2 {
            step(&ctx, &mut manager, &monitor, true, Vec::new());
        }
        press(&ctx, &mut manager, &monitor, true, "End Process");
        for _ in 0..2 {
            step(&ctx, &mut manager, &monitor, true, Vec::new());
        }
        let (_, ask) = press(&ctx, &mut manager, &monitor, true, "Yes");
        assert_eq!(ask, Some(Ask::End(pid)));
    }

    /// how many of the tray icon's two screens are lit, from what it painted.
    fn lit_screens(receiving: bool, sending: bool) -> (usize, usize) {
        let ctx = egui::Context::default();
        let at = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::splat(16.0));
        let shapes = run_frame(&ctx, Vec::new(), |ui| {
            tray_icon(ui, at, receiving, sending);
        });
        let fills = |colour| {
            shapes
                .iter()
                .filter(|s| matches!(s, Shape::Rect(r) if r.fill == colour))
                .count()
        };
        (fills(SCREEN_LIT), fills(SCREEN_DARK))
    }

    #[test]
    fn the_tray_icon_lights_each_monitor_on_its_own() {
        assert_eq!(lit_screens(false, false), (0, 2));
        assert_eq!(lit_screens(true, false), (1, 1));
        assert_eq!(lit_screens(false, true), (1, 1));
        assert_eq!(lit_screens(true, true), (2, 0));
    }

    #[test]
    fn the_window_opens_tall_enough_for_the_performance_tab_without_a_scrollbar() {
        let monitor = filled();
        let mut manager = TaskManager::default();
        manager.tab = 1;
        let ctx = egui::Context::default();
        let mut used = 0.0;
        for _ in 0..3 {
            run_frame(&ctx, Vec::new(), |ui| {
                manager.show(ui, &monitor, true);
                used = ui.min_rect().height();
            });
        }
        // the title bar, the gaps and the frame around the contents
        const CHROME: f32 = 56.0;
        let (_, height) = lanlord_app::desktop::Target::TaskManager.size();
        assert!(used + CHROME <= height, "{used} + {CHROME} > {height}");
    }
}
