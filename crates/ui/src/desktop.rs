//! the desktop: icons, windows and a taskbar with a start menu. the windows
//! show the real hosts, console and settings; gateway work runs in the
//! background through `Services`.

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{
    self, Align, Align2, Frame, Id, Layout, Margin, Pos2, Rect, RichText, Sense, Ui, Vec2,
};
use lanlord_core::log::{emit, Kind};
use lanlord_core::modules::{Module, Status, MODULES};
use lanlord_core::update;

mod menu;

use menu::Confirm;

use crate::admin_setup::{self, AdminSetup};
use crate::command_panel::CommandPanel;
use crate::console::{self, Console};
use crate::controls;
use crate::dialogs::{Answer, TextDialog};
use crate::dns_panel::DnsPanel;
use crate::form::Form;
use crate::hostlist::HostList;
use crate::icons;
use crate::login::{Auth, LoginBox, Outcome, SignedIn};
use crate::nt4_window::Nt4Window;
use crate::services::Services;
use crate::settings_editor::SettingsEditor;
use crate::tasks::PasswordRequest;
use crate::theme::{self, bevel, BLACK, GRAY, NAVY, TEAL, WHITE};
use crate::update_panel::{Choice, Updates};
use crate::windows;

const CELL: Vec2 = Vec2::new(84.0, 84.0);
const TASKBAR: f32 = 30.0;
/// the width of the LANlord button.
const MENU_BUTTON: f32 = 92.0;
/// the clock's width in the tray, and the width of one tray icon.
const CLOCK: f32 = 72.0;
const TRAY_ICON: f32 = 22.0;

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
    nt4: Nt4Window,
    dns: DnsPanel,
    commands: CommandPanel,
    /// the box asking before a reboot or power off, or showing the version.
    confirm: Option<(Confirm, Form)>,
    /// a task is waiting for this password.
    password: Option<(PasswordRequest, TextDialog)>,
    add_host: Option<TextDialog>,
    /// how to sign in; `None` when lanlord runs on the gateway itself (or in a test).
    auth: Option<Arc<dyn Auth>>,
    /// the logon box, shown instead of the desktop until someone is signed in.
    login: Option<LoginBox>,
    signed_in: Option<SignedIn>,
    /// the screen root lands on to create the first administrator, before the desktop.
    admin_setup: Option<AdminSetup>,
    /// the update notice beside the clock.
    updates: Updates,
    /// the checkout a development build rebuilds from, and the build that is running.
    dev: Option<PathBuf>,
    exe: Option<PathBuf>,
    /// a rebuild in progress; it answers once: `Ok` only when the new build is up and still running.
    rebuilding: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
    /// how long the new build must stay alive before this window may close.
    relaunch_grace: update::Grace,
    /// the Developer submenu is open.
    dev_open: bool,
    /// the title the window has been given, so it is only sent when it changes.
    window_title: String,
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
            nt4: Nt4Window::default(),
            dns: DnsPanel::default(),
            commands: CommandPanel::default(),
            confirm: None,
            password: None,
            add_host: None,
            auth: None,
            login: None,
            signed_in: None,
            admin_setup: None,
            updates: Updates::quiet(),
            dev: None,
            exe: None,
            rebuilding: None,
            relaunch_grace: update::Grace::default(),
            dev_open: false,
            window_title: "LANlord".into(),
        }
    }

    /// look for updates in the background, and offer a rebuild when this is a development build.
    pub fn watch_updates(&mut self, updates: Updates, dev: Option<PathBuf>, exe: Option<PathBuf>) {
        self.updates = updates;
        self.dev = dev;
        self.exe = exe;
    }

    /// the logon box for `host`, signing in at once with whatever the agent offers.
    pub fn resume(&mut self, host: &str) {
        let Some(auth) = self.auth.clone() else {
            return;
        };
        let mut login = LoginBox::new(auth, &self.services.ctx, Some(host));
        login.submit();
        self.login = Some(login);
    }

    /// start at the logon box. lanlord on the gateway itself has one server and no logon.
    pub fn require_login(&mut self, auth: Arc<dyn Auth>) {
        if self.services.is_local() {
            return;
        }
        let last = self.hosts.selected().map(String::from);
        self.login = Some(LoginBox::new(
            auth.clone(),
            &self.services.ctx,
            last.as_deref(),
        ));
        self.auth = Some(auth);
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

    /// open the windows named in `spec`, like `gateway,nt4-domain@1`: ids from the desktop
    /// icons, and for the nt4 window the number of the tab. it lets the real desktop be opened
    /// on a chosen view, for screenshots.
    pub fn open_spec(&mut self, spec: &str) {
        for item in spec.split(',').map(str::trim).filter(|i| !i.is_empty()) {
            let (id, tab) = item.split_once('@').unwrap_or((item, ""));
            if id == "menu" {
                self.start_open = true;
            } else if id == "developer" {
                self.start_open = true;
                self.dev_open = true;
            } else if let Some(target) = self.icons.iter().copied().find(|t| t.id() == id) {
                self.open(target);
                if id == "nt4-domain" {
                    self.nt4.tab = match tab {
                        "1" => crate::nt4_window::Tab::Logins,
                        "2" => crate::nt4_window::Tab::Logon,
                        _ => crate::nt4_window::Tab::Settings,
                    };
                }
            }
        }
    }

    /// the window is titled with the signed-in server, once someone is signed in.
    fn sync_window_title(&mut self, ctx: &egui::Context) {
        let title = match (self.signed_in.as_ref(), self.host()) {
            (Some(_), Some(host)) => format!("LANlord - {host}"),
            _ => "LANlord".to_string(),
        };
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    /// the host every action targets.
    fn host(&self) -> Option<String> {
        match self.hosts.selected() {
            Some(host) => Some(host.to_string()),
            // lanlord on the gateway itself has one server and no list
            None if self.services.is_local() => Some("local".into()),
            None => None,
        }
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
        self.sync_window_title(&ctx);

        if self.login.is_some() {
            self.show_login(ui, &ctx);
            return;
        }
        if self.admin_setup.is_some() {
            self.show_admin_setup(ui, &ctx);
            return;
        }
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
        self.updates.tick(&ctx);
        self.poll_rebuild(&ctx);
        self.settings.poll();
        self.nt4.poll();
        self.dns.poll();
        self.commands.poll();
        self.show_windows(&ctx);
        if self.start_open {
            self.start_menu(&ctx);
        }
        self.dialogs(&ctx);
    }

    /// the logon box on the bare desktop colour: no icons or taskbar until someone is signed in.
    fn show_login(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(Frame::new().fill(TEAL))
            .show(ui, |_| {});
        let known = self.hosts.hosts().to_vec();
        let outcome = self
            .login
            .as_mut()
            .and_then(|login| login.show(ctx, &known));
        match outcome {
            Some(Outcome::SignedIn(signed_in)) => self.finish_login(signed_in),
            Some(Outcome::ShutDown) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            None => {}
        }
    }

    /// someone signed in: the server joins the list and every action targets it. root goes to
    /// the screen that creates the first administrator before anything else.
    fn finish_login(&mut self, signed_in: SignedIn) {
        self.login = None;
        if let Err(error) = self.hosts.add(&signed_in.target) {
            emit(Kind::Err, error);
        }
        self.windows.clear();
        self.active = None;
        self.selected = None;
        self.start_open = false;
        if signed_in.identity.is_root() {
            self.admin_setup = Some(AdminSetup::new());
        }
        self.signed_in = Some(signed_in);
    }

    /// the create-an-administrator screen, on the bare desktop colour.
    fn show_admin_setup(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(Frame::new().fill(TEAL))
            .show(ui, |_| {});
        let Some(target) = self.signed_in.as_ref().map(|s| s.target.clone()) else {
            self.admin_setup = None;
            return;
        };
        let outcome = self
            .admin_setup
            .as_mut()
            .and_then(|setup| setup.show(ctx, &self.services, &target));
        match outcome {
            Some(admin_setup::Outcome::Skipped) => self.admin_setup = None,
            Some(admin_setup::Outcome::Created(name)) => self.admin_created(&name, &target),
            None => {}
        }
        self.dialogs(ctx);
    }

    /// the administrator exists: forget the root entry and ask for a logon as the new account.
    fn admin_created(&mut self, name: &str, root_target: &str) {
        self.admin_setup = None;
        if self.hosts.selected() == Some(root_target) {
            let _ = self.hosts.remove_selected();
        }
        let host = root_target.split_once('@').map_or(root_target, |(_, h)| h);
        self.return_to_login(
            Some(format!("{name}@{host}")),
            Some(format!("Created {name}. Log on with it.")),
        );
    }

    /// back to the logon box, closing the connection that was kept open for this server.
    fn log_off(&mut self) {
        let last = self.signed_in.as_ref().map(|s| s.target.clone());
        self.return_to_login(last, None);
    }

    /// show the logon box again with `last` filled in and `note` in its status bar.
    fn return_to_login(&mut self, last: Option<String>, note: Option<String>) {
        let Some(auth) = self.auth.clone() else {
            return;
        };
        if let Some(target) = self.signed_in.take().map(|s| s.target) {
            let closer = auth.clone();
            std::thread::spawn(move || closer.sign_out(&target));
        }
        self.windows.clear();
        self.active = None;
        self.start_open = false;
        let mut login = LoginBox::new(auth, &self.services.ctx, last.as_deref());
        if let Some(note) = note {
            login.note(&note);
        }
        self.login = Some(login);
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
        self.settings.dialog(ctx);
        let host = self.host();
        self.nt4.dialog(ctx, &self.services, host.as_deref());
        self.dns.dialog(ctx);
        self.confirmation(ctx);
        self.update_notice(ctx);
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

// updates and rebuilding a development build
impl Desktop {
    /// the balloon and the box of the update notice, and what they ask for.
    fn update_notice(&mut self, ctx: &egui::Context) {
        if self.updates.balloon(ctx, TASKBAR) {
            self.updates.open_dialog();
        }
        if self.updates.dialog(ctx) == Some(Choice::Install) {
            self.install_update();
        }
    }

    /// install what the notice found: rebuild from trunk, or open the release page.
    fn install_update(&mut self) {
        let Some(found) = self.updates.found().cloned() else {
            return;
        };
        match found.source {
            update::Source::Git => self.start_rebuild(true),
            update::Source::Release { url } => {
                if let Err(error) = update::open_release(&url) {
                    emit(
                        Kind::Err,
                        format!("could not open the download page: {error}"),
                    );
                }
            }
        }
    }

    /// is a rebuild running?
    pub(super) fn is_rebuilding(&self) -> bool {
        self.rebuilding.is_some()
    }

    /// rebuild this build in the background, with its output in the console. this window only
    /// closes once the new build has started and is still running; every other outcome leaves
    /// it open with the reason in the console and in a box.
    pub(super) fn start_rebuild(&mut self, pull: bool) {
        let (Some(repo), Some(exe)) = (self.dev.clone(), self.exe.clone()) else {
            emit(Kind::Err, "this build cannot be rebuilt from here");
            return;
        };
        if self.is_rebuilding() {
            return;
        }
        // before anything is touched: is the checkout still there, and can the tools be found?
        if let Err(error) = update::preflight(&repo, &exe, pull) {
            self.rebuild_failed(&format!("{error:#}"));
            return;
        }
        self.open(Target::Console);
        let host = self.signed_in.as_ref().map(|s| s.target.clone());
        let grace = self.relaunch_grace;
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.services.ctx.clone();
        std::thread::spawn(move || {
            let result = update::install_build(&repo, &exe, host.as_deref(), pull, grace);
            let _ = tx.send(result.map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        self.rebuilding = Some(rx);
    }

    /// say that a rebuild did not work, in the console and in a box, and carry on.
    fn rebuild_failed(&mut self, error: &str) {
        emit(Kind::Err, format!("the rebuild failed: {error}"));
        let text = format!("The rebuild did not work, so this window stays open.\n{error}");
        self.confirm = Some((
            Confirm::Failure,
            Form::new("rebuild-failed", "Rebuild and relaunch", &text),
        ));
    }

    /// once a frame: when the rebuild answered, quit if the new build is running, or say why not.
    fn poll_rebuild(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.rebuilding else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("the rebuild stopped unexpectedly".into())
            }
        };
        self.rebuilding = None;
        match result {
            Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(error) => self.rebuild_failed(&error),
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
                1.0,
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
            let start = self.menu_button(ui);
            if start.clicked() {
                self.start_open = !self.start_open;
            }
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
                    icons::small(window.target.id()),
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
                // the tray: a shallow sunken panel holding the update shield and the clock, whose
                // text is flat inside it
                let shield = self.updates.visible();
                let width = 6.0 + CLOCK + if shield { TRAY_ICON } else { 0.0 };
                let (tray, _) = ui.allocate_exact_size(Vec2::new(width, 22.0), Sense::hover());
                controls::thin_bevel(ui, tray, false);
                let clock_at = Rect::from_min_size(
                    Pos2::new(tray.right() - 3.0 - CLOCK, tray.top() + 2.0),
                    Vec2::new(CLOCK, 18.0),
                );
                ui.painter().text(
                    clock_at.center(),
                    Align2::CENTER_CENTER,
                    clock,
                    theme::font(),
                    BLACK,
                );
                if shield {
                    let icon_at = Rect::from_min_size(
                        Pos2::new(clock_at.left() - TRAY_ICON, tray.top()),
                        Vec2::splat(TRAY_ICON),
                    );
                    let clicked = ui
                        .scope_builder(egui::UiBuilder::new().max_rect(icon_at), |ui| {
                            self.updates.shield(ui)
                        })
                        .inner;
                    if clicked {
                        self.updates.open_dialog();
                    }
                }
            });
        });
    }
}

/// the part of the screen above the taskbar.
fn desktop_area(ctx: &egui::Context) -> Rect {
    let screen = ctx.content_rect();
    Rect::from_min_max(screen.min, screen.max - Vec2::new(0.0, TASKBAR))
}

/// how big a window opens, before it is cut down to fit the desktop.
fn window_size(target: Target) -> Vec2 {
    match target {
        Target::Module(i) => match MODULES[i].id {
            "nt4-domain" => Vec2::new(600.0, 600.0),
            "dns" => Vec2::new(620.0, 580.0),
            "remote-commands" => Vec2::new(580.0, 580.0),
            _ => Vec2::new(480.0, 320.0),
        },
        Target::Console => Vec2::new(600.0, 300.0),
        Target::Hosts => Vec2::new(380.0, 280.0),
        Target::Settings => Vec2::new(600.0, 520.0),
    }
}

/// where the `n`th window opens: stepped down and right of the last one, and never so far
/// that it hangs off the desktop or covers the icons.
fn cascade(n: usize, area: Rect, size: Vec2) -> Pos2 {
    let step = (n % 8) as f32;
    let x = (area.left() + 110.0 + step * 28.0).min(area.right() - size.x);
    let y = (area.top() + 10.0 + step * 26.0).min(area.bottom() - size.y);
    Pos2::new(x.max(area.left()), y.max(area.top()))
}

// the windows
impl Desktop {
    fn show_windows(&mut self, ctx: &egui::Context) {
        let mut windows = std::mem::take(&mut self.windows);
        let mut close = Vec::new();
        let area = desktop_area(ctx);
        for (n, window) in windows.iter_mut().enumerate() {
            if window.minimized {
                continue;
            }
            let target = window.target;
            let size = window_size(target).min(area.size() - Vec2::new(116.0, 8.0));
            let shown = windows::show(
                ctx,
                target.id(),
                target.title(),
                cascade(n, area, size),
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
            icons::draw(ui.painter(), ui.cursor().min, icons::art(module.id), 1.0);
            ui.add_space(40.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(module.name).size(16.0).strong());
                let status = match module.status {
                    Status::Available => "available",
                    Status::Planned => "planned - not built yet",
                };
                ui.label(format!("Status: {status}"));
            });
        });
        ui.add_space(2.0);
        ui.label(module.summary);
        ui.add_space(4.0);
        controls::separator(ui);
        ui.add_space(4.0);
        let host = self.host();
        match module.id {
            "nt4-domain" => self.nt4.show(ui, &self.services, host.as_deref()),
            "dns" => self.dns.show(ui, &self.services, host.as_deref()),
            "remote-commands" => {
                let machines = self.nt4.settings.machines();
                self.commands
                    .show(ui, &self.services, host.as_deref(), machines);
            }
            _ => self.plain_module(ui, module, host),
        }
    }

    /// a module with nothing to edit yet: its example settings and an Apply button.
    fn plain_module(&mut self, ui: &mut Ui, module: &'static Module, host: Option<String>) {
        controls::group(ui, "Settings (in your settings file)", |ui| {
            theme::sunken(ui, WHITE, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(module.example).monospace());
            });
        });
        ui.add_space(6.0);
        let busy = host.as_deref().is_some_and(|h| self.services.busy(h));
        let ready = module.status == Status::Available && !busy;
        controls::buttons(ui, |ui| {
            let apply = theme::action(ui, "Apply", ready && host.is_some());
            if let (true, Some(host)) = (apply.clicked(), &host) {
                self.services.apply(host, vec![module.id.to_string()]);
            }
        });
        let status = match (&host, busy) {
            (None, _) => "No host selected".to_string(),
            (Some(host), true) => format!("Host: {host} - working..."),
            (Some(host), false) => format!("Host: {host}"),
        };
        controls::status_bar(ui, &status, false);
    }

    fn console_window(&mut self, ui: &mut Ui) {
        let height = controls::fill_height(ui);
        theme::sunken(ui, BLACK, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .max_height(height)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height(height);
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
        if self.services.is_local() {
            ui.label("This computer is the server; there are no other hosts to pick.");
            return;
        }
        let mut picked = None;
        controls::group(ui, "Gateways lanlord manages", |ui| {
            theme::sunken(ui, WHITE, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(100.0);
                if self.hosts.hosts().is_empty() {
                    ui.label("No gateways yet. Click Add... and type user@gateway.");
                }
                for (i, host) in self.hosts.hosts().iter().enumerate() {
                    if theme::list_row(ui, host, self.hosts.selected_index() == i).clicked() {
                        picked = Some(i);
                    }
                }
            });
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
        ui.add_space(6.0);
        controls::status_bar(ui, if busy { "working..." } else { "ready" }, false);
    }
}

#[cfg(test)]
mod harness;
#[cfg(test)]
mod login_tests;
#[cfg(test)]
mod menu_tests;
#[cfg(test)]
mod update_tests;
#[cfg(test)]
mod window_tests;
#[cfg(test)]
mod tests {
    use super::harness::*;
    use super::*;
    use lanlord_core::log::Kind;

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
    fn windows_cascade_inside_the_desktop_and_clear_of_the_icons() {
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0));
        let size = Vec2::new(600.0, 500.0);
        let mut last = Pos2::ZERO;
        for n in 0..20 {
            let pos = cascade(n, area, size);
            assert!(
                area.contains_rect(Rect::from_min_size(pos, size)),
                "{n}: {pos:?}"
            );
            if n > 0 && n % 8 != 0 {
                assert!(pos.y >= last.y, "steps down");
            }
            last = pos;
        }
        assert!(
            cascade(0, area, size).x >= 100.0,
            "the first window leaves the icons visible"
        );
        // a small desktop pins the window to its corner instead of pushing it out of sight
        let tiny = Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 300.0));
        assert_eq!(cascade(3, tiny, Vec2::new(500.0, 300.0)), Pos2::ZERO);
    }

    #[test]
    fn every_window_has_a_size_that_fits_the_default_desktop() {
        let (mut desktop, ctx, _) = desktop();
        frames(&mut desktop, &ctx, 1);
        let area = desktop_area(&ctx);
        for target in desktop.icons.clone() {
            let size = window_size(target);
            assert!(
                size.x + 116.0 <= area.width() && size.y + 8.0 <= area.height(),
                "{}",
                target.title()
            );
        }
    }

    #[test]
    fn a_spec_opens_the_named_windows_menu_and_tab() {
        let (mut desktop, _, _) = desktop();
        desktop.open_spec("gateway, nt4-domain@2 ,menu,nothing,,");
        let open: Vec<_> = desktop.windows.iter().map(|w| w.target.id()).collect();
        assert_eq!(open, ["gateway", "nt4-domain"]);
        assert!(desktop.start_open);
        assert_eq!(desktop.nt4.tab, crate::nt4_window::Tab::Logon);
        desktop.open_spec("nt4-domain@1");
        assert_eq!(desktop.nt4.tab, crate::nt4_window::Tab::Logins);
        desktop.open_spec("nt4-domain");
        assert_eq!(desktop.nt4.tab, crate::nt4_window::Tab::Settings);
        desktop.start_open = false;
        desktop.open_spec("developer");
        assert!(desktop.start_open && desktop.dev_open);
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
    fn the_menu_opens_settings() {
        let (mut desktop, ctx, _) = desktop();
        desktop.start_open = true;
        frames(&mut desktop, &ctx, 3);
        assert!(desktop.start_open);
        let at = desktop
            .menu_item_at(screen(), &menu::Action::Open(Target::Settings))
            .unwrap();
        click_at(&mut desktop, &ctx, at);
        assert_eq!(desktop.active, Some(Target::Settings));
        assert!(!desktop.start_open);
    }

    #[test]
    fn the_lanlord_button_opens_the_menu() {
        let (mut desktop, ctx, _) = desktop();
        frames(&mut desktop, &ctx, 2);
        let button = Pos2::new(2.0 + MENU_BUTTON / 2.0, SCREEN.y - TASKBAR / 2.0);
        click_at(&mut desktop, &ctx, button);
        assert!(desktop.start_open);
        click_at(&mut desktop, &ctx, Pos2::new(700.0, 300.0));
        assert!(!desktop.start_open, "a click elsewhere closes it");
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
            MENU_BUTTON + 4.0 + 8.0 + 2.0 + 20.0 + 75.0,
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
            Pos2::new(add.left() + 10.0, add.top() - 110.0),
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
    fn a_module_without_a_host_cannot_apply() {
        let (mut desktop, ctx, fake) = desktop();
        desktop.open(Target::Module(0));
        frames(&mut desktop, &ctx, 2);
        assert_eq!(enabled(&ctx, "Apply"), Some(false), "no host");
        assert!(!press(&mut desktop, &ctx, "Apply"));
        assert!(fake.calls().is_empty());
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
    fn secrets_are_set_through_the_masked_dialog_and_saved() {
        let (mut desktop, ctx, fake) = desktop();
        *fake.stored.lock().unwrap() = "nt4-domain:\n  name: RETRO\n".into();
        with_host(&mut desktop, &ctx);
        desktop.open(Target::Settings);
        until(&mut desktop, &ctx, |d| d.settings.belongs_to("me@gw"));
        frames(&mut desktop, &ctx, 2);
        assert!(press(&mut desktop, &ctx, "Set..."));
        assert!(desktop.settings.secret_dialog_open());
        type_text(&mut desktop, &ctx, "s3cret");
        assert!(desktop.settings.pending_update("nt4-domain.admin_password"));
        assert!(press(&mut desktop, &ctx, "Save"));
        until(&mut desktop, &ctx, |d| !d.settings.dirty());
        let secrets = fake.secrets.lock().unwrap().clone();
        assert_eq!(
            secrets,
            [(
                "nt4-domain.admin_password".to_string(),
                "s3cret".to_string()
            )]
        );
        assert!(!fake.stored.lock().unwrap().contains("s3cret"));
    }

    #[test]
    fn a_secret_typed_into_the_plain_text_disables_saving() {
        let (mut desktop, ctx, fake) = desktop();
        with_host(&mut desktop, &ctx);
        desktop.open(Target::Settings);
        until(&mut desktop, &ctx, |d| d.settings.belongs_to("me@gw"));
        desktop.settings.text = "a:\n  password: x\n".into();
        frames(&mut desktop, &ctx, 2);
        assert_eq!(enabled(&ctx, "Save"), Some(false));
        assert_eq!(fake.calls(), ["load me@gw"]);
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
