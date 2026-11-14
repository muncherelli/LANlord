//! the desktop: icons, windows and a taskbar with a start menu.
//! the contents are dummy data for now; this is about the look.

use eframe::egui::{
    self, Align, Align2, Color32, Frame, Id, Layout, Margin, Order, Pos2, Rect, RichText, Sense,
    Ui, Vec2,
};
use lanlord_core::modules::{Module, Status, MODULES};

use crate::icons;
use crate::theme::{self, bevel, BLACK, GRAY, NAVY, SHADOW, TEAL, WHITE};
use crate::windows;

const CELL: Vec2 = Vec2::new(84.0, 84.0);
const TASKBAR: f32 = 30.0;

/// what an icon or a window shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Module(usize),
    Console,
    Hosts,
}

impl Target {
    fn id(self) -> &'static str {
        match self {
            Self::Module(i) => MODULES[i].id,
            Self::Console => "console",
            Self::Hosts => "hosts",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Module(i) => MODULES[i].name,
            Self::Console => "Console",
            Self::Hosts => "Hosts",
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
    hosts: Vec<String>,
    host: usize,
    users: Vec<String>,
    user: usize,
    console: Vec<(Color32, String)>,
}

impl Default for Desktop {
    fn default() -> Self {
        let mut icons: Vec<Target> = (0..MODULES.len()).map(Target::Module).collect();
        icons.push(Target::Console);
        icons.push(Target::Hosts);
        Self {
            icons,
            selected: None,
            windows: Vec::new(),
            active: None,
            start_open: false,
            hosts: vec!["linuxuser@lanlord.lan".into(), "admin@backup.lan".into()],
            host: 0,
            users: vec!["james".into(), "olive".into()],
            user: 0,
            console: vec![
                (
                    Color32::from_rgb(85, 255, 255),
                    "linuxuser@lanlord.lan$ uname -sr".into(),
                ),
                (Color32::LIGHT_GRAY, "Linux 7.0.14-28-pve".into()),
                (Color32::from_rgb(85, 255, 255), "exit 0".into()),
                (
                    Color32::from_rgb(85, 255, 255),
                    "applying nt4-domain to linuxuser@lanlord.lan".into(),
                ),
                (Color32::LIGHT_GRAY, "ok: [localhost]".into()),
                (Color32::from_rgb(85, 255, 255), "apply finished".into()),
            ],
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

    fn log(&mut self, text: String) {
        self.console.push((Color32::from_rgb(85, 255, 255), text));
    }

    fn host(&self) -> &str {
        self.hosts.get(self.host).map_or("-", String::as_str)
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

        self.show_windows(&ctx);
        if self.start_open {
            self.start_menu(&ctx);
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
        let rows = MODULES.len() + 2;
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
                ui.set_min_height(64.0);
                for (i, user) in self.users.clone().iter().enumerate() {
                    if theme::list_row(ui, user, self.user == i).clicked() {
                        self.user = i;
                    }
                }
            });
            ui.horizontal(|ui| {
                if theme::button(ui, "Add...", false).clicked() {
                    self.log("add a login (preview only)".into());
                }
                if theme::button(ui, "Password...", false).clicked() {
                    self.log("change a password (preview only)".into());
                }
                if theme::button(ui, "Delete", false).clicked() {
                    self.log("delete a login (preview only)".into());
                }
            });
        }
        ui.add_space(8.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let available = module.status == Status::Available;
            if theme::button(ui, "Apply", false).clicked() && available {
                let line = format!("applying {} to {}", module.id, self.host());
                self.log(line);
            }
            ui.label(format!("Host: {}", self.host()));
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
                    for (colour, line) in &self.console {
                        ui.label(RichText::new(line).monospace().color(*colour));
                    }
                });
        });
    }

    fn hosts_window(&mut self, ui: &mut Ui) {
        ui.label("Gateways lanlord manages:");
        theme::sunken(ui, WHITE, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(90.0);
            for (i, host) in self.hosts.clone().iter().enumerate() {
                if theme::list_row(ui, host, self.host == i).clicked() {
                    self.host = i;
                }
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if theme::button(ui, "Add...", false).clicked() {
                self.log("add a host (preview only)".into());
            }
            if theme::button(ui, "Remove", false).clicked() {
                self.log("remove a host (preview only)".into());
            }
            if theme::button(ui, "Bootstrap", false).clicked() {
                let line = format!("bootstrapping {} (preview only)", self.host());
                self.log(line);
            }
        });
        let _ = SHADOW;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Vec2 = Vec2::new(960.0, 640.0);

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

    fn click(at: Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![egui::Event::PointerMoved(at), button(true), button(false)]
    }

    #[test]
    fn the_empty_desktop_draws() {
        let (ctx, mut desktop) = (egui::Context::default(), Desktop::default());
        for _ in 0..3 {
            frame(&mut desktop, &ctx, Vec::new());
        }
        assert_eq!(desktop.icons.len(), MODULES.len() + 2);
        assert!(desktop.windows.is_empty());
    }

    #[test]
    fn every_window_draws() {
        let (ctx, mut desktop) = (egui::Context::default(), Desktop::default());
        for target in desktop.icons.clone() {
            desktop.open(target);
        }
        for _ in 0..3 {
            frame(&mut desktop, &ctx, Vec::new());
        }
        assert_eq!(desktop.windows.len(), desktop.icons.len());
        assert!(desktop.windows.iter().all(|w| !w.minimized));
    }

    #[test]
    fn opening_a_window_twice_reuses_it_and_unminimizes_it() {
        let mut desktop = Desktop::default();
        desktop.open(Target::Console);
        desktop.windows[0].minimized = true;
        desktop.open(Target::Console);
        assert_eq!(desktop.windows.len(), 1);
        assert!(!desktop.windows[0].minimized);
        assert_eq!(desktop.active, Some(Target::Console));
    }

    #[test]
    fn the_start_menu_draws_and_lists_the_modules() {
        let (ctx, mut desktop) = (egui::Context::default(), Desktop::default());
        desktop.start_open = true;
        for _ in 0..3 {
            frame(&mut desktop, &ctx, Vec::new());
        }
        assert!(desktop.start_open);
    }

    #[test]
    fn clicking_an_icon_selects_it_and_clicking_the_desktop_clears_it() {
        let (ctx, mut desktop) = (egui::Context::default(), Desktop::default());
        frame(&mut desktop, &ctx, Vec::new());
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
        let (ctx, mut desktop) = (egui::Context::default(), Desktop::default());
        desktop.open(Target::Console);
        frame(&mut desktop, &ctx, Vec::new());
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
    fn the_preview_buttons_write_to_the_console() {
        let mut desktop = Desktop::default();
        let before = desktop.console.len();
        desktop.log("hello".into());
        assert_eq!(desktop.console.len(), before + 1);
        assert_eq!(desktop.host(), "linuxuser@lanlord.lan");
    }
}
