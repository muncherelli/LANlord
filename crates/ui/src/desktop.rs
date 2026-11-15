//! the desktop: icons, windows and a taskbar with a start menu. the windows
//! show the real hosts, console and settings; gateway work runs in the
//! background through `Services`.

use std::path::PathBuf;

use eframe::egui::{
    self, Align, Align2, Frame, Id, Layout, Margin, Order, Pos2, Rect, RichText, Sense, Ui, Vec2,
};
use lanlord_core::log::{emit, Kind};
use lanlord_core::modules::{Module, Status, MODULES};

use crate::console::{self, Console};
use crate::dialogs::{Answer, TextDialog};
use crate::hostlist::HostList;
use crate::icons;
use crate::services::Services;
use crate::settings_editor::SettingsEditor;
use crate::tasks::PasswordRequest;
use crate::theme::{self, bevel, BLACK, GRAY, NAVY, TEAL, WHITE};
use crate::windows;

const CELL: Vec2 = Vec2::new(84.0, 84.0);
const TASKBAR: f32 = 30.0;

/// what an icon or a window shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Module(usize),
    Console,
    Hosts,
    Settings,
}

impl Target {
    fn id(self) -> &'static str {
        match self {
            Self::Module(i) => MODULES[i].id,
            Self::Console => "console",
            Self::Hosts => "hosts",
            Self::Settings => "settings",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Module(i) => MODULES[i].name,
            Self::Console => "Console",
            Self::Hosts => "Hosts",
            Self::Settings => "Settings",
        }
    }
}

struct Window {
    target: Target,
    minimized: bool,
}

pub struct Desktop {
    icons: Vec<Target>,
    selected: Option<usize>,
    windows: Vec<Window>,
    active: Option<Target>,
    start_open: bool,
    hosts: HostList,
    console: Console,
    services: Services,
    settings: SettingsEditor,
    /// a task is waiting for this password.
    password: Option<(PasswordRequest, TextDialog)>,
    add_host: Option<TextDialog>,
}

impl Desktop {
    /// a desktop that keeps its hosts in `hosts_path` and talks to gateways through `services`.
    pub fn new(services: Services, hosts_path: Option<PathBuf>) -> Self {
        let mut icons: Vec<Target> = (0..MODULES.len()).map(Target::Module).collect();
        icons.extend([Target::Settings, Target::Console, Target::Hosts]);
        Self {
            icons,
            selected: None,
            windows: Vec::new(),
            active: None,
            start_open: false,
            hosts: HostList::new(hosts_path),
            console: Console::new(&services.ctx),
            services,
            settings: SettingsEditor::default(),
            password: None,
            add_host: None,
        }
    }
}

impl Desktop {
    fn open(&mut self, target: Target) {
        match self.windows.iter_mut().find(|w| w.target == target) {
            Some(window) => window.minimized = false,
            None => self.windows.push(Window {
                target,
                minimized: false,
            }),
        }
        self.active = Some(target);
        self.start_open = false;
    }

    /// the host every action targets.
    fn host(&self) -> Option<String> {
        self.hosts.selected().map(String::from)
    }
}

impl eframe::App for Desktop {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

impl Desktop {
    /// draw one frame of the whole desktop.
    fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        theme::apply(&ctx);
        // keep the clock moving
        ctx.request_repaint_after(std::time::Duration::from_secs(1));

        egui::Panel::bottom("taskbar")
            .exact_size(TASKBAR)
            .frame(
                Frame::new()
                    .fill(GRAY)
                    .inner_margin(Margin::symmetric(2, 2)),
            )
            .show(ui, |ui| self.taskbar(ui));

        egui::CentralPanel::default()
            .frame(Frame::new().fill(TEAL))
            .show(ui, |ui| self.icons(ui));

        self.console.drain();
        self.settings.poll();
        self.show_windows(&ctx);
        if self.start_open {
            self.start_menu(&ctx);
        }
        self.dialogs(&ctx);
    }

    /// the modal boxes: a task's password prompt and the add-host box.
    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.password.is_none() {
            self.password = self.services.tasks.next_request().map(|request| {
                let dialog = TextDialog::new("password", "Password", &request.prompt, true);
                (request, dialog)
            });
        }
        if let Some((request, mut dialog)) = self.password.take() {
            match dialog.show(ctx) {
                Some(answer) => {
                    let _ = request.reply.send(match answer {
                        Answer::Ok(password) => Some(password),
                        Answer::Cancel => None,
                    });
                }
                None => self.password = Some((request, dialog)),
            }
        }
        if let Some(mut dialog) = self.add_host.take() {
            match dialog.show(ctx) {
                Some(Answer::Ok(text)) => {
                    if let Err(error) = self.hosts.add(&text) {
                        dialog.error = Some(error);
                        self.add_host = Some(dialog);
                    }
                }
                Some(Answer::Cancel) => {}
                None => self.add_host = Some(dialog),
            }
        }
    }
}

// the desktop icons
impl Desktop {
    fn icons(&mut self, ui: &mut Ui) {
        let area = ui.max_rect();
        let per_column = (((area.height() - 16.0) / CELL.y) as usize).max(1);
        let mut opened = None;
        let mut clicked_blank = ui
            .interact(area, Id::unique("desktop"), Sense::click())
            .clicked();
        for (i, target) in self.icons.clone().into_iter().enumerate() {
            let pos = area.min
                + Vec2::new(
                    12.0 + (i / per_column) as f32 * CELL.x,
                    12.0 + (i % per_column) as f32 * CELL.y,
                );
            let rect = Rect::from_min_size(pos, CELL);
            let response = ui.interact(rect, Id::unique(("icon", i)), Sense::click());
            let selected = self.selected == Some(i);
            let painter = ui.painter();
            icons::draw(
                painter,
                Pos2::new(rect.center().x - 16.0, rect.top() + 4.0),
                icons::art(target.id()),
                2.0,
            );
            let galley = painter.layout(
                target.title().to_string(),
                theme::font(),
                WHITE,
                CELL.x - 6.0,
            );
            let at = Pos2::new(rect.center().x - galley.size().x / 2.0, rect.top() + 42.0);
            if selected {
                painter.rect_filled(
                    Rect::from_min_size(at, galley.size()).expand(1.0),
                    0.0,
                    NAVY,
                );
            }
            painter.galley(at, galley, WHITE);
            if response.clicked() {
                self.selected = Some(i);
                clicked_blank = false;
            }
            if response.double_clicked() {
                opened = Some(target);
            }
        }
        if clicked_blank {
            self.selected = None;
            self.start_open = false;
        }
        if let Some(target) = opened {
            self.open(target);
        }
    }
}

// the taskbar and the start menu
impl Desktop {
    fn taskbar(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
            let start = self.start_button(ui);
            if start.clicked() {
                self.start_open = !self.start_open;
            }
            ui.add_space(4.0);
            let (rect, _) = ui.allocate_exact_size(Vec2::new(2.0, 22.0), Sense::hover());
            bevel(ui.painter(), rect, false);
            ui.add_space(2.0);

            let mut to_activate = None;
            let mut to_minimize = None;
            for window in &self.windows {
                let active = self.active == Some(window.target) && !window.minimized;
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::new(150.0, 22.0), Sense::click());
                let down = active || response.is_pointer_button_down_on();
                ui.painter().rect_filled(rect, 0.0, GRAY);
                bevel(ui.painter(), rect, !down);
                icons::draw(
                    ui.painter(),
                    rect.left_top() + Vec2::new(4.0, 3.0),
                    icons::art(window.target.id()),
                    1.0,
                );
                ui.painter().text(
                    rect.left_center() + Vec2::new(24.0, 0.0),
                    Align2::LEFT_CENTER,
                    window.target.title(),
                    theme::font(),
                    BLACK,
                );
                if response.clicked() {
                    if window.minimized || !active {
                        to_activate = Some(window.target);
                    } else {
                        to_minimize = Some(window.target);
                    }
                }
            }
            if let Some(target) = to_activate {
                self.open(target);
            }
            if let Some(target) = to_minimize {
                if let Some(window) = self.windows.iter_mut().find(|w| w.target == target) {
                    window.minimized = true;
                }
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let clock = chrono::Local::now().format("%-I:%M %p").to_string();
                let (rect, _) = ui.allocate_exact_size(Vec2::new(78.0, 22.0), Sense::hover());
                bevel(ui.painter(), rect, false);
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    clock,
                    theme::font(),
                    BLACK,
                );
            });
        });
    }

    fn start_button(&self, ui: &mut Ui) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(70.0, 22.0), Sense::click());
        let down = self.start_open || response.is_pointer_button_down_on();
        ui.painter().rect_filled(rect, 0.0, GRAY);
        bevel(ui.painter(), rect, !down);
        let nudge = if down {
            Vec2::new(1.0, 1.0)
        } else {
            Vec2::ZERO
        };
        let logo = Rect::from_min_size(
            rect.left_top() + Vec2::new(6.0, 4.0) + nudge,
            Vec2::splat(14.0),
        );
        ui.painter().rect_filled(logo, 0.0, NAVY);
        ui.painter().text(
            logo.center(),
            Align2::CENTER_CENTER,
            "L",
            theme::font(),
            WHITE,
        );
        ui.painter().text(
            rect.left_center() + Vec2::new(26.0, 0.0) + nudge,
            Align2::LEFT_CENTER,
            "Start",
            egui::FontId::proportional(14.0),
            BLACK,
        );
        response
    }

    fn start_menu(&mut self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        let width = 190.0;
        let rows = MODULES.len() + 3;
        let height = rows as f32 * 26.0 + 14.0;
        let pos = Pos2::new(screen.left() + 2.0, screen.bottom() - TASKBAR - height);
        let mut chosen: Option<Option<Target>> = None;
        let response = egui::Area::new(Id::unique("start-menu"))
            .order(Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
                ui.painter().rect_filled(rect, 0.0, GRAY);
                bevel(ui.painter(), rect, true);
                let strip =
                    Rect::from_min_size(rect.min + Vec2::splat(3.0), Vec2::new(22.0, height - 6.0));
                ui.painter().rect_filled(strip, 0.0, NAVY);
                let mut y = rect.top() + 8.0;
                let item = |ui: &mut Ui, label: &str, art: Option<&str>, y: &mut f32| -> bool {
                    let row = Rect::from_min_size(
                        Pos2::new(rect.left() + 28.0, *y),
                        Vec2::new(width - 32.0, 24.0),
                    );
                    let response =
                        ui.interact(row, Id::unique(("start-item", label)), Sense::click());
                    let hot = response.hovered();
                    if hot {
                        ui.painter().rect_filled(row, 0.0, NAVY);
                    }
                    if let Some(art) = art {
                        icons::draw(
                            ui.painter(),
                            row.left_top() + Vec2::new(4.0, 4.0),
                            icons::art(art),
                            1.0,
                        );
                    }
                    ui.painter().text(
                        row.left_center() + Vec2::new(28.0, 0.0),
                        Align2::LEFT_CENTER,
                        label,
                        theme::font(),
                        if hot { WHITE } else { BLACK },
                    );
                    *y += 26.0;
                    response.clicked()
                };
                for (i, module) in MODULES.iter().enumerate() {
                    if item(ui, module.name, Some(module.id), &mut y) {
                        chosen = Some(Some(Target::Module(i)));
                    }
                }
                if item(ui, "Settings", Some("settings"), &mut y) {
                    chosen = Some(Some(Target::Settings));
                }
                if item(ui, "Console", Some("console"), &mut y) {
                    chosen = Some(Some(Target::Console));
                }
                if item(ui, "Shut Down...", None, &mut y) {
                    chosen = Some(None);
                }
                rect
            });
        match chosen {
            Some(Some(target)) => self.open(target),
            Some(None) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            None => {
                // a click anywhere else closes the menu
                let outside = ctx.input(|i| i.pointer.any_pressed())
                    && !response.response.contains_pointer()
                    && ctx
                        .input(|i| i.pointer.interact_pos())
                        .is_some_and(|p| p.y < screen.bottom() - TASKBAR);
                if outside {
                    self.start_open = false;
                }
            }
        }
    }
}

// the windows
impl Desktop {
    fn show_windows(&mut self, ctx: &egui::Context) {
        let mut windows = std::mem::take(&mut self.windows);
        let mut close = Vec::new();
        for (n, window) in windows.iter_mut().enumerate() {
            if window.minimized {
                continue;
            }
            let target = window.target;
            let size = match target {
                Target::Module(_) => Vec2::new(420.0, 400.0),
                Target::Console => Vec2::new(520.0, 260.0),
                Target::Hosts => Vec2::new(340.0, 250.0),
                Target::Settings => Vec2::new(480.0, 420.0),
            };
            let shown = windows::show(
                ctx,
                target.id(),
                target.title(),
                Pos2::new(130.0 + n as f32 * 30.0, 30.0 + n as f32 * 26.0),
                size,
                self.active == Some(target),
                |ui| self.contents(ui, target),
            );
            if let Some((actions, clicked)) = shown {
                if actions.close {
                    close.push(target);
                } else if actions.minimize {
                    window.minimized = true;
                } else if clicked {
                    self.active = Some(target);
                }
            }
        }
        windows.retain(|w| !close.contains(&w.target));
        self.windows = windows;
    }

    fn contents(&mut self, ui: &mut Ui, target: Target) {
        match target {
            Target::Module(i) => self.module_window(ui, &MODULES[i]),
            Target::Console => self.console_window(ui),
            Target::Hosts => self.hosts_window(ui),
            Target::Settings => {
                let host = self.host();
                self.settings.show(ui, &self.services, host.as_deref());
            }
        }
    }

    fn module_window(&mut self, ui: &mut Ui, module: &'static Module) {
        ui.horizontal(|ui| {
            icons::draw(ui.painter(), ui.cursor().min, icons::art(module.id), 2.0);
            ui.add_space(38.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(module.name).size(16.0).strong());
                let status = match module.status {
                    Status::Available => "available",
                    Status::Planned => "planned - not built yet",
                };
                ui.label(format!("Status: {status}"));
            });
        });
        ui.add_space(4.0);
        ui.label(module.summary);
        ui.add_space(6.0);
        ui.label("Settings (in your settings file):");
        theme::sunken(ui, WHITE, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(module.example).monospace());
        });
        if module.id == "nt4-domain" {
            ui.add_space(6.0);
            ui.label("Logins:");
            theme::sunken(ui, WHITE, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(48.0);
                ui.label("Loading the logins from the gateway is coming.");
            });
        }
        ui.add_space(8.0);
        let host = self.host();
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let busy = host.as_deref().is_some_and(|h| self.services.busy(h));
            let ready = module.status == Status::Available && !busy;
            let apply = theme::action(ui, "Apply", ready && host.is_some());
            if let (true, Some(host)) = (apply.clicked(), &host) {
                self.services.apply(host, vec![module.id.to_string()]);
            }
            ui.label(match (&host, busy) {
                (None, _) => "No host selected".to_string(),
                (Some(host), true) => format!("Host: {host} - working..."),
                (Some(host), false) => format!("Host: {host}"),
            });
        });
    }

    fn console_window(&mut self, ui: &mut Ui) {
        theme::sunken(ui, BLACK, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .max_height(ui.available_height() - 4.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for line in self.console.lines() {
                        let text = RichText::new(&line.text)
                            .monospace()
                            .color(console::colour(line.kind));
                        ui.label(text);
                    }
                });
        });
    }

    fn hosts_window(&mut self, ui: &mut Ui) {
        ui.label("Gateways lanlord manages:");
        let mut picked = None;
        theme::sunken(ui, WHITE, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(90.0);
            if self.hosts.hosts().is_empty() {
                ui.label("No gateways yet. Click Add... and type user@gateway.");
            }
            for (i, host) in self.hosts.hosts().iter().enumerate() {
                if theme::list_row(ui, host, self.hosts.selected_index() == i).clicked() {
                    picked = Some(i);
                }
            }
        });
        if let Some(i) = picked {
            self.hosts.select(i);
        }
        ui.add_space(6.0);
        let host = self.host();
        let busy = host.as_deref().is_some_and(|h| self.services.busy(h));
        let ready = host.is_some() && !busy;
        ui.horizontal(|ui| {
            if theme::action(ui, "Add...", true).clicked() && self.add_host.is_none() {
                self.add_host = Some(TextDialog::new(
                    "add-host",
                    "Add a host",
                    "Gateway as user@host:",
                    false,
                ));
            }
            if theme::action(ui, "Remove", ready).clicked() {
                if let Err(error) = self.hosts.remove_selected() {
                    emit(Kind::Err, error);
                }
            }
            if theme::action(ui, "Bootstrap", ready).clicked() {
                if let Some(host) = &host {
                    self.services.bootstrap(host);
                }
            }
        });
        if busy {
            ui.label(RichText::new("working...").italics());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::gateway::Fake;
    use lanlord_core::log::Kind;

    const SCREEN: Vec2 = Vec2::new(960.0, 640.0);

    fn desktop() -> (Desktop, egui::Context, Arc<Fake>) {
        let ctx = egui::Context::default();
        let fake = Arc::new(Fake::new());
        let desktop = Desktop::new(Services::new(fake.clone(), &ctx), None);
        (desktop, ctx, fake)
    }

    fn frame(desktop: &mut Desktop, ctx: &egui::Context, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            events,
            ..Default::default()
        };
        // nothing uploads the textures here, so drop the update on purpose
        ctx.run_ui(input, |ui| desktop.show(ui))
            .textures_delta
            .clear();
    }

    fn frames(desktop: &mut Desktop, ctx: &egui::Context, n: usize) {
        for _ in 0..n {
            frame(desktop, ctx, Vec::new());
        }
    }

    fn click(at: Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![egui::Event::PointerMoved(at), button(true), button(false)]
    }

    fn click_at(desktop: &mut Desktop, ctx: &egui::Context, at: Pos2) {
        for event in click(at) {
            frame(desktop, ctx, vec![event]);
        }
        frames(desktop, ctx, 2);
    }

    /// click the button labelled `text`; false when it is missing or disabled.
    fn press(desktop: &mut Desktop, ctx: &egui::Context, text: &str) -> bool {
        match theme::probe::find(ctx, text) {
            Some((rect, true)) => {
                click_at(desktop, ctx, rect.center());
                true
            }
            _ => false,
        }
    }

    fn enabled(ctx: &egui::Context, text: &str) -> Option<bool> {
        theme::probe::find(ctx, text).map(|(_, on)| on)
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn type_text(desktop: &mut Desktop, ctx: &egui::Context, text: &str) {
        frame(desktop, ctx, vec![egui::Event::Text(text.into())]);
        frame(desktop, ctx, vec![key(egui::Key::Enter)]);
        frames(desktop, ctx, 2);
    }

    /// draw frames until `done` holds.
    fn until(desktop: &mut Desktop, ctx: &egui::Context, done: impl Fn(&Desktop) -> bool) {
        let start = Instant::now();
        while !done(desktop) {
            assert!(start.elapsed() < Duration::from_secs(5), "timed out");
            frames(desktop, ctx, 1);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn idle(host: &'static str) -> impl Fn(&Desktop) -> bool {
        move |d| !d.services.busy(host)
    }

    fn with_host(desktop: &mut Desktop, ctx: &egui::Context) {
        desktop.hosts.add("me@gw").unwrap();
        frames(desktop, ctx, 2);
    }

    #[test]
    fn the_empty_desktop_draws() {
        let (mut desktop, ctx, _) = desktop();
        frames(&mut desktop, &ctx, 3);
        assert_eq!(desktop.icons.len(), MODULES.len() + 3);
        assert!(desktop.windows.is_empty());
    }

    #[test]
    fn every_window_draws_with_and_without_a_host() {
        let (mut desktop, ctx, fake) = desktop();
        for target in desktop.icons.clone() {
            desktop.open(target);
        }
        frames(&mut desktop, &ctx, 3);
        assert!(
            fake.calls().is_empty(),
            "nothing is contacted without a host"
        );
        with_host(&mut desktop, &ctx);
        until(&mut desktop, &ctx, idle("me@gw"));
        frames(&mut desktop, &ctx, 3);
        assert_eq!(desktop.windows.len(), desktop.icons.len());
        assert!(desktop.windows.iter().all(|w| !w.minimized));
    }

    #[test]
    fn opening_a_window_twice_reuses_it_and_unminimizes_it() {
        let (mut desktop, _, _) = desktop();
        desktop.open(Target::Console);
        desktop.windows[0].minimized = true;
        desktop.open(Target::Console);
        assert_eq!(desktop.windows.len(), 1);
        assert!(!desktop.windows[0].minimized);
        assert_eq!(desktop.active, Some(Target::Console));
    }

    #[test]
    fn the_start_menu_opens_settings() {
        let (mut desktop, ctx, _) = desktop();
        desktop.start_open = true;
        frames(&mut desktop, &ctx, 3);
        assert!(desktop.start_open);
        // the settings row is third from the bottom, above Console and Shut Down
        let rows = MODULES.len() + 3;
        let top = SCREEN.y - TASKBAR - (rows as f32 * 26.0 + 14.0);
        let row = top + 8.0 + (MODULES.len() as f32) * 26.0 + 12.0;
        click_at(&mut desktop, &ctx, Pos2::new(100.0, row));
        assert_eq!(desktop.active, Some(Target::Settings));
        assert!(!desktop.start_open);
    }

    #[test]
    fn clicking_an_icon_selects_it_and_clicking_the_desktop_clears_it() {
        let (mut desktop, ctx, _) = desktop();
        frames(&mut desktop, &ctx, 1);
        // the first icon's cell starts at (12, 12)
        for events in click(Pos2::new(54.0, 54.0)).into_iter().map(|e| vec![e]) {
            frame(&mut desktop, &ctx, events);
        }
        assert_eq!(desktop.selected, Some(0));
        for events in click(Pos2::new(700.0, 400.0)).into_iter().map(|e| vec![e]) {
            frame(&mut desktop, &ctx, events);
        }
        assert_eq!(desktop.selected, None);
    }

    #[test]
    fn taskbar_buttons_raise_and_minimize() {
        let (mut desktop, ctx, _) = desktop();
        desktop.open(Target::Console);
        frames(&mut desktop, &ctx, 1);
        // the first window button sits just right of the start button
        let button = Pos2::new(
            70.0 + 4.0 + 4.0 + 2.0 + 20.0 + 75.0,
            SCREEN.y - TASKBAR / 2.0,
        );
        for events in click(button).into_iter().map(|e| vec![e]) {
            frame(&mut desktop, &ctx, events);
        }
        assert!(
            desktop.windows[0].minimized,
            "clicking the active window's button minimizes it"
        );
        for events in click(button).into_iter().map(|e| vec![e]) {
            frame(&mut desktop, &ctx, events);
        }
        assert!(
            !desktop.windows[0].minimized,
            "clicking again brings it back"
        );
    }

    #[test]
    fn the_hosts_window_starts_empty_and_adds_through_the_dialog() {
        let (mut desktop, ctx, _) = desktop();
        desktop.open(Target::Hosts);
        frames(&mut desktop, &ctx, 2);
        assert_eq!(enabled(&ctx, "Remove"), Some(false));
        assert_eq!(enabled(&ctx, "Bootstrap"), Some(false));
        assert!(press(&mut desktop, &ctx, "Add..."));
        assert!(desktop.add_host.is_some());
        type_text(&mut desktop, &ctx, "two words");
        assert!(
            desktop.add_host.is_some(),
            "a bad host keeps the dialog open"
        );
        assert!(desktop.hosts.hosts().is_empty());
        let dialog = desktop.add_host.as_mut().unwrap();
        dialog.error = None;
        // clear the text by cancelling and starting over
        frame(&mut desktop, &ctx, vec![key(egui::Key::Escape)]);
        assert!(desktop.add_host.is_none());
        assert!(press(&mut desktop, &ctx, "Add..."));
        type_text(&mut desktop, &ctx, "me@gw");
        assert!(desktop.add_host.is_none());
        assert_eq!(desktop.hosts.hosts(), ["me@gw"]);
        assert_eq!(enabled(&ctx, "Remove"), Some(true));
    }

    #[test]
    fn hosts_are_selected_by_clicking_and_removed_with_the_button() {
        let (mut desktop, ctx, _) = desktop();
        desktop.hosts.add("me@one").unwrap();
        desktop.hosts.add("me@two").unwrap();
        desktop.open(Target::Hosts);
        frames(&mut desktop, &ctx, 2);
        assert_eq!(desktop.host().as_deref(), Some("me@two"));
        // the list starts under the title bar, the label and the window margin
        let (add, _) = theme::probe::find(&ctx, "Add...").unwrap();
        click_at(
            &mut desktop,
            &ctx,
            Pos2::new(add.left() + 10.0, add.top() - 100.0),
        );
        assert_eq!(desktop.host().as_deref(), Some("me@one"));
        assert!(press(&mut desktop, &ctx, "Remove"));
        assert_eq!(desktop.hosts.hosts(), ["me@two"]);
        assert!(press(&mut desktop, &ctx, "Remove"));
        frames(&mut desktop, &ctx, 2);
        assert!(desktop.host().is_none());
        assert_eq!(enabled(&ctx, "Remove"), Some(false));
    }

    #[test]
    fn bootstrap_runs_through_the_gateway_and_asks_for_the_password() {
        let (mut desktop, ctx, fake) = desktop();
        *fake.password_prompt.lock().unwrap() = Some("sudo password for me".into());
        with_host(&mut desktop, &ctx);
        desktop.open(Target::Hosts);
        frames(&mut desktop, &ctx, 2);
        assert!(press(&mut desktop, &ctx, "Bootstrap"));
        until(&mut desktop, &ctx, |d| d.password.is_some());
        assert_eq!(
            desktop.password.as_ref().unwrap().1.prompt(),
            "sudo password for me"
        );
        assert_eq!(
            enabled(&ctx, "Bootstrap"),
            Some(false),
            "working: buttons are off"
        );
        assert_eq!(enabled(&ctx, "Remove"), Some(false));
        type_text(&mut desktop, &ctx, "hunter2");
        until(&mut desktop, &ctx, idle("me@gw"));
        assert_eq!(fake.calls(), ["bootstrap me@gw"]);
        assert_eq!(fake.answer.lock().unwrap().as_deref(), Some("hunter2"));
        assert!(desktop.password.is_none());
        frames(&mut desktop, &ctx, 2);
        assert_eq!(enabled(&ctx, "Bootstrap"), Some(true));
    }

    #[test]
    fn cancelling_the_password_dialog_cancels_the_task() {
        let (mut desktop, ctx, fake) = desktop();
        *fake.password_prompt.lock().unwrap() = Some("pw".into());
        with_host(&mut desktop, &ctx);
        desktop.open(Target::Hosts);
        frames(&mut desktop, &ctx, 2);
        let seen = lanlord_core::log::subscribe();
        assert!(press(&mut desktop, &ctx, "Bootstrap"));
        until(&mut desktop, &ctx, |d| d.password.is_some());
        frame(&mut desktop, &ctx, vec![key(egui::Key::Escape)]);
        until(&mut desktop, &ctx, idle("me@gw"));
        assert!(fake.answer.lock().unwrap().is_none());
        let errors: Vec<String> = seen
            .try_iter()
            .filter(|l| l.kind == Kind::Err)
            .map(|l| l.text)
            .collect();
        assert!(errors.iter().any(|e| e == "cancelled"), "{errors:?}");
    }

    #[test]
    fn a_module_window_applies_just_that_module() {
        let (mut desktop, ctx, fake) = desktop();
        with_host(&mut desktop, &ctx);
        desktop.open(Target::Module(0));
        frames(&mut desktop, &ctx, 2);
        assert!(press(&mut desktop, &ctx, "Apply"));
        until(&mut desktop, &ctx, idle("me@gw"));
        assert_eq!(fake.calls(), [format!("apply me@gw [{}]", MODULES[0].id)]);
    }

    #[test]
    fn modules_that_are_planned_or_have_no_host_cannot_apply() {
        let (mut desktop, ctx, fake) = desktop();
        let planned = MODULES
            .iter()
            .position(|m| m.status == Status::Planned)
            .expect("a planned module");
        desktop.open(Target::Module(planned));
        frames(&mut desktop, &ctx, 2);
        assert_eq!(enabled(&ctx, "Apply"), Some(false), "no host");
        with_host(&mut desktop, &ctx);
        assert_eq!(enabled(&ctx, "Apply"), Some(false), "planned");
        assert!(!press(&mut desktop, &ctx, "Apply"));
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn the_nt4_window_no_longer_shows_dummy_logins() {
        let (mut desktop, ctx, _) = desktop();
        let nt4 = MODULES.iter().position(|m| m.id == "nt4-domain").unwrap();
        desktop.open(Target::Module(nt4));
        frames(&mut desktop, &ctx, 2);
        for label in ["Add...", "Password...", "Delete"] {
            assert!(theme::probe::find(&ctx, label).is_none(), "{label}");
        }
    }

    #[test]
    fn the_settings_window_loads_edits_saves_reloads_and_applies() {
        let (mut desktop, ctx, fake) = desktop();
        *fake.stored.lock().unwrap() = "# mine\ngateway:\n  motd: hi\n".into();
        desktop.open(Target::Settings);
        frames(&mut desktop, &ctx, 2);
        assert!(fake.calls().is_empty(), "no host, no contact");
        with_host(&mut desktop, &ctx);
        until(&mut desktop, &ctx, |d| d.settings.belongs_to("me@gw"));
        assert_eq!(desktop.settings.text, "# mine\ngateway:\n  motd: hi\n");

        desktop.settings.text.push_str("# more\n");
        frames(&mut desktop, &ctx, 2);
        assert!(press(&mut desktop, &ctx, "Save"));
        until(&mut desktop, &ctx, |d| !d.settings.dirty());
        assert_eq!(
            *fake.stored.lock().unwrap(),
            "# mine\ngateway:\n  motd: hi\n# more\n"
        );

        desktop.settings.text = "changed".into();
        assert!(press(&mut desktop, &ctx, "Reload"));
        until(&mut desktop, &ctx, |d| !d.settings.dirty());
        assert!(desktop.settings.text.ends_with("# more\n"));

        assert!(press(&mut desktop, &ctx, "Apply"));
        until(&mut desktop, &ctx, idle("me@gw"));
        assert!(press(&mut desktop, &ctx, "Apply module"));
        until(&mut desktop, &ctx, idle("me@gw"));
        assert_eq!(
            fake.calls(),
            [
                "load me@gw",
                "save me@gw",
                "load me@gw",
                "apply me@gw []",
                &format!("apply me@gw [{}]", MODULES[0].id)
            ]
        );
    }

    #[test]
    fn broken_yaml_disables_saving() {
        let (mut desktop, ctx, fake) = desktop();
        with_host(&mut desktop, &ctx);
        desktop.open(Target::Settings);
        until(&mut desktop, &ctx, |d| d.settings.belongs_to("me@gw"));
        desktop.settings.text = "a: [oops".into();
        frames(&mut desktop, &ctx, 2);
        assert_eq!(enabled(&ctx, "Save"), Some(false));
        assert!(!press(&mut desktop, &ctx, "Save"));
        assert_eq!(fake.calls(), ["load me@gw"]);
    }

    #[test]
    fn a_missing_key_shows_in_the_settings_window() {
        let (mut desktop, ctx, fake) = desktop();
        *fake.fail.lock().unwrap() = Some("no key yet; run `lanlord keygen`".into());
        with_host(&mut desktop, &ctx);
        desktop.open(Target::Settings);
        until(&mut desktop, &ctx, |d| {
            d.settings.message.is_some() && !d.settings.working()
        });
        assert!(desktop
            .settings
            .message
            .as_ref()
            .unwrap()
            .1
            .contains("keygen"));
        frames(&mut desktop, &ctx, 3);
        assert_eq!(
            fake.calls().len(),
            1,
            "a failed load is not retried by itself"
        );
    }

    #[test]
    fn switching_hosts_loads_the_new_hosts_settings() {
        let (mut desktop, ctx, fake) = desktop();
        desktop.hosts.add("me@one").unwrap();
        desktop.open(Target::Settings);
        until(&mut desktop, &ctx, |d| d.settings.belongs_to("me@one"));
        *fake.stored.lock().unwrap() = "other: 1\n".into();
        desktop.hosts.add("me@two").unwrap();
        until(&mut desktop, &ctx, |d| d.settings.belongs_to("me@two"));
        assert_eq!(desktop.settings.text, "other: 1\n");
    }

    #[test]
    fn the_console_shows_what_lanlord_logs() {
        let (mut desktop, ctx, _) = desktop();
        desktop.open(Target::Console);
        for (kind, text) in [
            (Kind::Out, "desk-test-out"),
            (Kind::Err, "desk-test-err"),
            (Kind::Info, "desk-test-info"),
            (Kind::Debug, "desk-test-debug"),
        ] {
            lanlord_core::log::emit(kind, text);
        }
        until(&mut desktop, &ctx, |d| {
            d.console
                .lines()
                .iter()
                .filter(|l| l.text.starts_with("desk-test-"))
                .count()
                == 4
        });
        frames(&mut desktop, &ctx, 2);
    }
}
