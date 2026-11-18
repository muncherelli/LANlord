//! the desktop: icons, windows and a taskbar with a start menu. the windows
//! show the real hosts, console and settings; gateway work runs in the
//! background through `Services`.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use eframe::egui::{self, Align, Align2, Frame, Id, Layout, Margin, Pos2, Rect, Sense, Ui, Vec2};
use lanlord_app::log::{emit, Kind};
#[cfg(feature = "native")]
use lanlord_core::update;
use lanlord_model::modules::{Module, MODULES};

#[cfg(feature = "devtools")]
mod devtools;
mod menu;

use menu::Confirm;

use crate::admin_setup::{self, AdminSetup};
use crate::clock_panel::{ClockFeed, ClockPanel};
use crate::command_panel::CommandPanel;
use crate::components_window::{self, Components};
use crate::controls;
use crate::dialogs::{Answer, TextDialog};
use crate::dns_panel::DnsPanel;
use crate::form::Form;
use crate::icons;
use crate::login::{Auth, LoginBox, Outcome, SignedIn};
use crate::my_server::{self, MyServer};
use crate::nt4_window::Nt4Window;
use crate::settings_editor::SettingsEditor;
use crate::theme::{self, bevel, BLACK, GRAY, NAVY, TEAL, WHITE};
#[cfg(feature = "native")]
use crate::update_panel::{Choice, Updates};
use crate::view;
use crate::windows;
use lanlord_app::account_window::AccountWindow;
use lanlord_app::console::Console;
use lanlord_app::desktop::Target;
use lanlord_app::gateway::Gateway;
use lanlord_app::hostlist::HostList;
use lanlord_app::login::WebSession;
use lanlord_app::monitor::{self, Backend, Monitor};
use lanlord_app::networks_window::{self, NetworksWindow};
use lanlord_app::permissions_window::PermissionsWindow;
use lanlord_app::remote::Remote;
use lanlord_app::services::Services;
use lanlord_app::tasks::PasswordRequest;
use lanlord_app::welcome::Welcome;
use lanlord_app::{hosts_window, module_window};
use lanlord_model::access::Grants;
use lanlord_model::identity::AccountInfo;

const CELL: Vec2 = Vec2::new(84.0, 84.0);
const TASKBAR: f32 = 30.0;
/// the desktop icons' margin and the gap after their column: windows open to the right of it.
const ICON_MARGIN: f32 = 12.0;
const ICON_GAP: f32 = 14.0;
const ICON_COLUMN: f32 = ICON_MARGIN + CELL.x + ICON_GAP;
/// the width of the LANlord button.
const MENU_BUTTON: f32 = 92.0;
/// the clock's width in the tray, and the width of one tray icon.
const CLOCK: f32 = 72.0;
const TRAY_ICON: f32 = 22.0;

/// what an icon or a window shows.
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
    /// the egui context background work wakes, and windows that need one (logon box, console).
    ctx: egui::Context,
    settings: SettingsEditor,
    nt4: Nt4Window,
    account: AccountWindow,
    permissions: PermissionsWindow,
    /// what the gateway says the signed-in account may do, fetched once after logon.
    grants_remote: Remote<AccountInfo>,
    dns: DnsPanel,
    commands: CommandPanel,
    my_server: MyServer,
    networks: NetworksWindow,
    /// the components wizard, and which components the server has switched on.
    components: Components,
    welcome: Welcome,
    /// the host's live numbers, read all the time for the tray icon, and the Task Manager window.
    monitor: Monitor,
    task_manager: crate::task_manager::TaskManager,
    /// made-up numbers for the demo desktop, instead of a host's.
    synthetic: Option<std::sync::Arc<lanlord_app::demo_monitor::Synthetic>>,
    /// tests pin the switched-on components here instead of reading them from the server.
    #[cfg(test)]
    pinned: Option<lanlord_app::components::Selection>,
    /// a window a window asked for; it opens once all the windows are drawn.
    open_next: Option<Target>,
    /// the window opened from `open_next` shows My Server's Compatibility tab.
    compat_next: bool,
    /// the taskbar alert for what the components need and the server lacks.
    compat_alert: crate::compat_alert::Alert,
    /// the Date/Time Properties window, and the server's time shown in the taskbar.
    clock: ClockPanel,
    feed: ClockFeed,
    /// windows that asked to be closed from inside (OK or Cancel).
    close_requests: Vec<Target>,
    /// the box asking before a reboot or power off, or showing the version.
    confirm: Option<(Confirm, Form)>,
    /// a task is waiting for this password.
    password: Option<(PasswordRequest, TextDialog)>,
    add_host: Option<TextDialog>,
    /// how to sign in; `None` when lanlord runs on the gateway itself (or in a test).
    auth: Option<Arc<dyn Auth>>,
    /// the logon box, shown instead of the desktop until someone is signed in.
    login: Option<LoginBox>,
    /// how the browser logs on and off; `None` everywhere else.
    web: Option<Rc<dyn WebSession>>,
    signed_in: Option<SignedIn>,
    /// each window as drawn last frame: its rectangle and the height its contents asked for.
    measured: Vec<(Target, Rect, f32)>,
    /// the Welcome topics' heights for the width and scale they were measured at, so they are
    /// not laid out again every frame.
    welcome_heights: Option<(f32, f32, Vec<f32>)>,
    /// windows a `?open=` link asked for, opened once someone is logged on.
    open_after_logon: Option<String>,
    /// the browser's logout, waiting for the server to say the session is gone.
    logging_off: Option<std::sync::mpsc::Receiver<Result<bool, String>>>,
    /// the screen root lands on to create the first administrator, before the desktop.
    admin_setup: Option<AdminSetup>,
    /// the update notice beside the clock.
    #[cfg(feature = "native")]
    updates: Updates,
    /// the checkout a development build rebuilds from, and the build that is running.
    dev: Option<PathBuf>,
    #[cfg(feature = "native")]
    exe: Option<PathBuf>,
    /// a rebuild in progress; it answers once: `Ok` only when the new build is up and still running.
    #[cfg(feature = "native")]
    rebuilding: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
    /// how long the new build must stay alive before this window may close.
    #[cfg(feature = "native")]
    relaunch_grace: update::Grace,
    /// the Developer submenu is open.
    dev_open: bool,
    /// the title the window has been given, so it is only sent when it changes.
    window_title: String,
    /// the made-up demo server, not a real gateway: the title and the taskbar say so.
    demo: bool,
    #[cfg(feature = "devtools")]
    devtools: devtools::Devtools,
}

impl Desktop {
    /// a desktop that keeps its hosts in `hosts_path` and talks to gateways through `gateway`;
    /// background work wakes `ctx` when it finishes.
    pub fn new(
        gateway: Arc<dyn Gateway>,
        ctx: &egui::Context,
        hosts_path: Option<PathBuf>,
    ) -> Self {
        let services = Services::new(gateway, crate::wake(ctx));
        let icons = lanlord_app::desktop::icons(None);
        Self {
            icons,
            selected: None,
            windows: Vec::new(),
            active: None,
            start_open: false,
            hosts: HostList::new(hosts_path),
            console: Console::new(crate::wake(ctx)),
            ctx: ctx.clone(),
            services,
            settings: SettingsEditor::default(),
            nt4: Nt4Window::default(),
            account: AccountWindow::default(),
            permissions: PermissionsWindow::default(),
            grants_remote: Remote::default(),
            dns: DnsPanel::default(),
            commands: CommandPanel::default(),
            my_server: MyServer::default(),
            networks: NetworksWindow::default(),
            components: Components::default(),
            welcome: Welcome::new(None),
            monitor: Monitor::default(),
            task_manager: crate::task_manager::TaskManager::default(),
            synthetic: None,
            #[cfg(test)]
            pinned: None,
            open_next: None,
            compat_next: false,
            compat_alert: Default::default(),
            clock: ClockPanel::default(),
            feed: ClockFeed::default(),
            close_requests: Vec::new(),
            confirm: None,
            password: None,
            add_host: None,
            auth: None,
            login: None,
            web: None,
            logging_off: None,
            open_after_logon: None,
            signed_in: None,
            measured: Vec::new(),
            welcome_heights: None,
            admin_setup: None,
            #[cfg(feature = "native")]
            updates: Updates::quiet(),
            dev: None,
            #[cfg(feature = "native")]
            exe: None,
            #[cfg(feature = "native")]
            rebuilding: None,
            #[cfg(feature = "native")]
            relaunch_grace: update::Grace::default(),
            dev_open: false,
            window_title: "LANlord".into(),
            demo: false,
            #[cfg(feature = "devtools")]
            devtools: Default::default(),
        }
    }

    /// look for updates in the background, and offer a rebuild when this is a development build.
    #[cfg(feature = "native")]
    pub fn watch_updates(&mut self, updates: Updates, dev: Option<PathBuf>, exe: Option<PathBuf>) {
        self.updates = updates;
        if let (Some(repo), Some(exe)) = (&dev, &exe) {
            self.updates.watch_checkout(repo.clone(), exe.clone());
        }
        self.dev = dev;
        self.exe = exe;
    }

    /// this desktop shows the made-up demo server (the browser demo); say so in the title and
    /// the taskbar, so nobody mistakes it for a real gateway.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub fn mark_demo(&mut self) {
        self.demo = true;
        self.synthetic = Some(Default::default());
    }

    /// act as if signed in to a made-up server (`LANLORD_FAKE_SERVER=1`), so the server windows
    /// can be looked at without one; the host list lives in memory only.
    #[cfg_attr(not(feature = "native"), allow(dead_code))]
    pub fn use_demo_server(&mut self) {
        self.hosts = HostList::new(None);
        let _ = self.hosts.add("demo@gateway");
        self.synthetic = Some(Default::default());
    }

    /// the logon box for `host`, signing in at once with whatever the agent offers.
    #[cfg_attr(not(feature = "native"), allow(dead_code))]
    pub fn resume(&mut self, host: &str) {
        let Some(auth) = self.auth.clone() else {
            return;
        };
        let mut login = LoginBox::new(auth, &self.ctx, Some(host));
        login.submit();
        self.login = Some(login);
    }

    /// the browser starts at the logon box too, unless the page's session is still good: the
    /// gateway has one server, and its own accounts log on through `session`.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub fn require_web_login(&mut self, session: Rc<dyn WebSession>) {
        let mut login = LoginBox::web(session.clone(), &self.ctx, None);
        login.resume_web();
        self.login = Some(login);
        self.web = Some(session);
    }

    /// start at the logon box. lanlord on the gateway itself has one server and no logon.
    #[cfg_attr(not(feature = "native"), allow(dead_code))]
    pub fn require_login(&mut self, auth: Arc<dyn Auth>) {
        if self.services.is_local() {
            return;
        }
        let last = self.hosts.selected().map(String::from);
        self.login = Some(LoginBox::new(auth.clone(), &self.ctx, last.as_deref()));
        self.auth = Some(auth);
    }
}

impl Desktop {
    /// is the account an administrator? ending a process is for them alone, as on the server.
    /// without a sign-in only the demo and a shell on the gateway itself count.
    fn is_admin(&self) -> bool {
        self.signed_in
            .as_ref()
            .map_or(self.demo || self.services.is_local(), |s| s.identity.admin)
    }

    /// what the account may do. nothing beyond the Users role until the gateway has said so (and
    /// the demo server, which is made up, may do all); the server enforces it either way.
    fn grants(&self) -> Grants {
        match &self.signed_in {
            Some(s) => s.identity.grants(),
            None => self
                .grants_remote
                .data
                .as_ref()
                .map_or_else(|| Grants::fallback(self.demo), |i| i.grants.clone()),
        }
    }

    /// ask the gateway once what the account may do, and keep the answer for the menus.
    fn refresh_grants(&mut self) {
        if self.grants_remote.poll() {
            let grants = self.grants_remote.data.as_ref().map(|i| i.grants.clone());
            // an answer never replaces grants the account already has
            if let Some(signed_in) = self
                .signed_in
                .as_mut()
                .filter(|s| s.identity.grants.is_none())
            {
                signed_in.identity.grants = grants;
            }
        }
        let Some(host) = self.host() else { return };
        let pending = self
            .signed_in
            .as_ref()
            .map_or(self.grants_remote.data.is_none(), |s| {
                s.identity.grants.is_none()
            });
        if pending && self.grants_remote.wants_load(&host, false) {
            let target = host.clone();
            let job = self
                .services
                .quiet(move |gateway| gateway.account_info(&target, &mut lanlord_app::ask::never));
            self.grants_remote.track(&host, Some(job), "", "", true);
        }
    }

    /// drop everything one session left open: windows, dialogs and the state behind them, so the
    /// next account starts clean and cannot answer a box the last one opened.
    fn reset_session_ui(&mut self) {
        // a logon or log off ends the demo desktop; the real one comes back
        #[cfg(feature = "devtools")]
        self.end_demo();
        self.windows.clear();
        self.active = None;
        self.selected = None;
        self.start_open = false;
        self.confirm = None;
        self.password = None;
        self.add_host = None;
        self.open_next = None;
        self.compat_next = false;
        self.close_requests.clear();
        self.settings = SettingsEditor::default();
        self.nt4 = Nt4Window::default();
        self.account = AccountWindow::default();
        self.permissions = PermissionsWindow::default();
        self.grants_remote = Remote::default();
        self.dns = DnsPanel::default();
        self.commands = CommandPanel::default();
        self.my_server = MyServer::default();
        self.networks = NetworksWindow::default();
        self.components = Components::default();
        self.clock.reset();
        self.monitor = Monitor::default();
        self.task_manager = crate::task_manager::TaskManager::default();
    }

    fn open(&mut self, target: Target) {
        if !target.visible(&self.grants()) {
            return;
        }
        match self.windows.iter_mut().find(|w| w.target == target) {
            Some(window) => window.minimized = false,
            None => {
                if target == Target::DateTime {
                    // a fresh reading of the server every time the window opens
                    self.clock.reset();
                }
                if target == Target::MyServer {
                    self.my_server
                        .set_read_only(!Target::MyServer.writable(&self.grants()));
                }
                if target == Target::Components {
                    self.components.open();
                }
                if target == Target::Welcome {
                    self.welcome.reopen();
                }
                self.windows.push(Window {
                    target,
                    minimized: false,
                });
            }
        }
        self.active = Some(target);
        self.start_open = false;
    }

    /// put My Server on its Compatibility tab.
    fn show_compat_tab(&mut self) {
        let host = self.host().unwrap_or_default();
        self.my_server.update(
            my_server::Msg::Tab(my_server::COMPATIBILITY),
            &self.services,
            &host,
        );
    }

    /// the failing or warning checks that what the server has switched on needs.
    fn compat_problems(&self) -> Vec<lanlord_model::doctor::Check> {
        let Some(host) = self.host() else {
            return Vec::new();
        };
        let (Some(report), Some(enabled)) = (self.services.compat().report(&host), self.enabled())
        else {
            return Vec::new();
        };
        lanlord_app::compat::problems(&report, &enabled)
    }

    /// like `open_spec`, but when the logon box is up the windows wait until someone is on.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub fn open_spec_after_logon(&mut self, spec: &str) {
        if self.login.is_some() {
            self.open_after_logon = Some(spec.to_string());
        } else {
            self.open_spec(spec);
        }
    }

    /// open the windows named in `spec`, like `gateway,nt4-domain@1`: ids from the desktop
    /// icons, and for the nt4 window the number of the tab. it lets the real desktop be opened
    /// on a chosen view, for screenshots.
    pub fn open_spec(&mut self, spec: &str) {
        for item in spec.split(',').map(str::trim).filter(|i| !i.is_empty()) {
            let (id, tab) = item.split_once('@').unwrap_or((item, ""));
            if id == "menu" {
                self.start_open = true;
            } else if id == "all" && cfg!(feature = "devtools") {
                #[cfg(feature = "devtools")]
                self.open_all();
            } else if id == "datetime" {
                self.open(Target::DateTime);
                self.clock.open_tab(tab);
            } else if id == "developer" {
                self.start_open = true;
                self.dev_open = true;
            } else if id == "components" {
                self.open(Target::Components);
            } else if id == "welcome" {
                self.open(Target::Welcome);
            } else if id == "changelog" {
                self.open(Target::Changelog);
            } else if id == "taskmgr" {
                self.open(Target::TaskManager);
                self.task_manager.tab = tab.parse().unwrap_or(0).min(2);
            } else if let Some(target) = self
                .icons
                .iter()
                .copied()
                .find(|t| t.id() == id)
                .or_else(|| MODULES.iter().position(|m| m.id == id).map(Target::Module))
            {
                self.open(target);
                if id == "my-server" {
                    // @1 is the Advanced tab, @2 also asks before a purge
                    let page = match tab {
                        "1" | "2" => my_server::ADVANCED,
                        "3" | "compat" => my_server::COMPATIBILITY,
                        _ => my_server::GENERAL,
                    };
                    let host = self.host().unwrap_or_default();
                    self.my_server
                        .update(my_server::Msg::Tab(page), &self.services, &host);
                    if let (true, Some(host)) = (tab == "2", self.host()) {
                        self.my_server
                            .update(my_server::Msg::AskPurge, &self.services, &host);
                    }
                }
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

    /// the window is titled with the app and its version, plus the signed-in server once someone
    /// is signed in.
    fn sync_window_title(&mut self, ctx: &egui::Context) {
        let host = self.signed_in.as_ref().and(self.host());
        let mut title =
            lanlord_app::desktop::window_title(self.services.is_local(), host.as_deref());
        if self.demo {
            title.push_str(" (demo)");
        }
        #[cfg(feature = "devtools")]
        if self.in_demo() {
            title = "LANlord (demo)".into();
        }
        if title != self.window_title {
            crate::platform::set_title(ctx, &title);
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
        self.poll_log_off();
        if self.login.is_some() {
            self.console.follow_server(false, &crate::wake(&ctx));
            self.show_login(ui, &ctx);
            #[cfg(feature = "devtools")]
            self.devtools_corner(&ctx);
            return;
        }
        if self.web.is_some() && (self.services.session_ended() || self.monitor.signed_out()) {
            // any call the server turned down for want of a login sends the person back to the box
            self.monitor.clear_signed_out();
            self.console.follow_server(false, &crate::wake(&ctx));
            self.return_to_web_login(Some("Your session ended. Log on again."), false);
            self.show_login(ui, &ctx);
            #[cfg(feature = "devtools")]
            self.devtools_corner(&ctx);
            return;
        }
        if self.admin_setup.is_some() {
            self.show_admin_setup(ui, &ctx);
            return;
        }
        self.sync_components();
        self.monitor_tick(&ctx);
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
        #[cfg(feature = "devtools")]
        self.devtools_menu(&ctx, TASKBAR + 2.0);

        self.console.drain();
        // only an account that may see apply output asks for it
        let follows = self.web.is_some()
            && self.grants().allows(
                lanlord_model::ops::Op::Subscribe {
                    topic: lanlord_model::ops::topic::APPLY.into(),
                }
                .requires(),
            );
        self.console.follow_server(follows, &crate::wake(&ctx));
        self.console.apply.tick(ctx.input(|i| i.time));
        if self.console.apply.running() {
            // the elapsed time on the progress dialog keeps counting between events
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        if self.console.apply.take_started() {
            self.open(Target::Progress);
        }
        #[cfg(feature = "native")]
        {
            self.updates.tick(&ctx);
            self.poll_rebuild(&ctx);
        }
        self.refresh_grants();
        self.account.poll();
        self.networks.poll();
        self.permissions.poll();
        self.settings.poll();
        self.nt4.poll();
        self.dns.poll();
        self.commands.poll();
        self.poll_clock();
        self.show_windows(&ctx);
        if self.start_open {
            self.start_menu(&ctx);
        }
        self.dialogs(&ctx);
    }

    /// keep the taskbar clock on the server's time: read it once, and take what the Date/Time
    /// window reads or changes.
    fn poll_clock(&mut self) {
        let host = self.host();
        self.feed.watch(&self.services, host.as_deref());
        if let (Some(state), Some(host)) = (self.clock.poll(), host.as_deref()) {
            self.feed.set(host, state);
        }
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
        // the browser's one server is the gateway itself, not an entry in the list
        if self.web.is_none() {
            if let Err(error) = self.hosts.add(&signed_in.target) {
                emit(Kind::Err, error);
            }
        }
        self.services.session_restored();
        self.reset_session_ui();
        if signed_in.identity.is_root() {
            self.admin_setup = Some(AdminSetup::new());
        }
        let account = signed_in.identity.user.clone();
        self.signed_in = Some(signed_in);
        self.compat_alert.reset();
        if self.welcome.logon(&account) {
            self.open(Target::Welcome);
        }
        if let Some(spec) = self.open_after_logon.take() {
            self.open_spec(&spec);
        }
    }

    /// keep the Welcome screen's tick box in the preferences file at `path`.
    #[cfg(feature = "native")]
    pub fn set_prefs(&mut self, path: Option<std::path::PathBuf>) {
        self.welcome = Welcome::new(path);
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

    /// lanlord was removed from `host`: forget it and go back to the logon box.
    fn after_purge(&mut self, host: &str) {
        if let Some(i) = self.hosts.hosts().iter().position(|h| h == host) {
            self.hosts.select(i);
            let _ = self.hosts.remove_selected();
        }
        self.log_off();
    }

    /// back to the logon box, closing the connection that was kept open for this server.
    fn log_off(&mut self) {
        if self.web.is_some() {
            self.return_to_web_login(None, true);
            return;
        }
        let last = self.signed_in.as_ref().map(|s| s.target.clone());
        self.return_to_login(last, None);
    }

    /// the browser's way back to the logon box: the name stays filled in, and `log_off` says the
    /// server still has the session, so it is ended first. the box waits for the server's answer:
    /// a logout that failed leaves the person signed in, and one still in flight cannot clear
    /// the cookie of a later logon.
    fn return_to_web_login(&mut self, note: Option<&str>, log_off: bool) {
        let Some(session) = self.web.clone() else {
            return;
        };
        if log_off {
            if self.logging_off.is_some() {
                return;
            }
            let (tx, rx) = std::sync::mpsc::channel();
            let ctx = self.ctx.clone();
            lanlord_app::login::spawn_local(async move {
                session.logout().await;
                // the logout is done when the server says it no longer knows the session; if it
                // could not be asked, the session may well still be there
                let answer = match session.current().await {
                    Ok(now) => Ok(now.is_none()),
                    Err(e) => Err(e.to_string()),
                };
                let _ = tx.send(answer);
                ctx.request_repaint();
            });
            self.logging_off = Some(rx);
            return;
        }
        self.show_web_login(session, note);
    }

    /// once a frame: when the logout answered, show the box, or say it did not work.
    fn poll_log_off(&mut self) {
        let Some(rx) = &self.logging_off else {
            return;
        };
        let ended = match rx.try_recv() {
            Ok(answer) => answer,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Ok(false),
        };
        self.logging_off = None;
        if ended == Ok(true) {
            if let Some(session) = self.web.clone() {
                self.show_web_login(session, None);
            }
        } else {
            let text = match ended {
                Err(_) => {
                    "Could not reach the server to end your session, so you are still logged on."
                }
                _ => "The server did not end your session, so you are still logged on.",
            };
            emit(Kind::Err, text);
            self.confirm = Some((
                Confirm::Failure,
                Form::new("log-off-failed", "Log off", text),
            ));
        }
    }

    fn show_web_login(&mut self, session: Rc<dyn WebSession>, note: Option<&str>) {
        let last = self.signed_in.take().map(|s| s.identity.user);
        self.reset_session_ui();
        let mut login = LoginBox::web(session, &self.ctx, last.as_deref());
        if let Some(note) = note {
            login.note(note);
        }
        self.login = Some(login);
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
        self.reset_session_ui();
        let mut login = LoginBox::new(auth, &self.ctx, last.as_deref());
        if let Some(note) = note {
            login.note(&note);
        }
        self.login = Some(login);
    }

    /// the modal boxes: a task's password prompt and the add-host box.
    /// the Change Password box and the boxes of the permissions window.
    fn account_dialogs(&mut self, ctx: &egui::Context, host: Option<&str>) {
        if let Some(dialog) = self.account.dialog() {
            if let (Some(msg), Some(host)) = (view::dialog(ctx, &dialog), host) {
                self.account.update_dialog(msg, &self.services, host);
            }
        }
        if let Some(dialog) = self.permissions.dialog() {
            if let Some(msg) = view::dialog(ctx, &dialog) {
                self.permissions.update_dialog(msg);
            }
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.password.is_none() {
            self.password = self.services.next_request().map(|request| {
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
        self.account_dialogs(ctx, host.as_deref());
        self.network_dialog(ctx);
        my_server::dialog(&mut self.my_server, ctx, &self.services);
        components_window::dialog(&mut self.components, ctx, &self.services, host.as_deref());
        if let Some(host) = self.my_server.take_purged() {
            self.after_purge(&host);
        }
        self.confirmation(ctx);
        let raised = !self.compat_problems().is_empty();
        self.compat_alert.tick(raised, ctx.input(|i| i.time));
        if self.compat_alert.balloon(ctx, TASKBAR) {
            self.open(Target::MyServer);
            self.show_compat_tab();
        }
        #[cfg(feature = "native")]
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
#[cfg(feature = "native")]
impl Desktop {
    /// the balloon and the box of the update notice, and what they ask for.
    fn update_notice(&mut self, ctx: &egui::Context) {
        if self.updates.balloon(ctx, TASKBAR) {
            self.updates.open_dialog();
        }
        match self.updates.dialog(ctx) {
            Some(Choice::Install) => self.install_update(),
            Some(Choice::Pause) => self.updates.pause(),
            Some(Choice::Resume) => self.updates.resume(),
            Some(Choice::Cancel) => self.updates.cancel(),
            Some(Choice::Restart) => self.restart_into_new_build(),
            Some(Choice::Later) | None => {}
        }
    }

    /// start the build that was just installed and quit, once it is up and still running; the
    /// same checks as a rebuild, so a build that dies at once leaves this window open.
    fn restart_into_new_build(&mut self) {
        let Some(exe) = self.exe.clone() else {
            emit(Kind::Err, "this build cannot be restarted from here");
            return;
        };
        if self.is_rebuilding() {
            return;
        }
        let host = self.signed_in.as_ref().map(|s| s.target.clone());
        let grace = self.relaunch_grace;
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = update::verify_built(&exe)
                .and_then(|()| update::relaunch(&exe, host.as_deref(), grace));
            let _ = tx.send(result.map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        self.rebuilding = Some(rx);
    }

    /// install what the notice found: rebuild from trunk, or open the release page.
    fn install_update(&mut self) {
        let Some(found) = self.updates.found().cloned() else {
            return;
        };
        match found.source {
            update::Source::Git => {
                // the pull and the build run in the background, and say so in the console
                self.open(Target::Console);
                self.updates.install();
            }
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
    pub(super) fn is_rebuilding_native(&self) -> bool {
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
        let ctx = self.ctx.clone();
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
                    ICON_MARGIN + (i / per_column) as f32 * CELL.x,
                    ICON_MARGIN + (i % per_column) as f32 * CELL.y,
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
                icons::draw_small(
                    ui.painter(),
                    rect.left_top() + Vec2::new(4.0, 3.0),
                    window.target.id(),
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
                let clock = self.feed.text();
                // the tray: a shallow sunken panel holding the update shield and the clock, whose
                // text is flat inside it
                let shield = self.update_shield();
                let warn = !self.compat_problems().is_empty();
                let net = self.monitor.now().is_some();
                let width = 6.0
                    + CLOCK
                    + if shield { TRAY_ICON } else { 0.0 }
                    + if warn { TRAY_ICON } else { 0.0 }
                    + if net { TRAY_ICON } else { 0.0 }
                    + if cfg!(feature = "devtools") {
                        TRAY_ICON
                    } else {
                        0.0
                    };
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
                let clock_hit = ui
                    .interact(clock_at, ui.scope_id().with("tray-clock"), Sense::click())
                    .on_hover_text(self.feed.tooltip());
                #[cfg(test)]
                theme::probe::record(ui, "clock", clock_at, true);
                if clock_hit.double_clicked() {
                    self.open(Target::DateTime);
                }
                if self.demo {
                    // a sunken note beside the tray: the data here is made up
                    let note = "Demo: made-up data";
                    let (at, _) = ui.allocate_exact_size(Vec2::new(124.0, 22.0), Sense::hover());
                    controls::thin_bevel(ui, at, false);
                    ui.painter().text(
                        at.center(),
                        Align2::CENTER_CENTER,
                        note,
                        theme::font(),
                        BLACK,
                    );
                }
                if warn {
                    let sign_at = Rect::from_min_size(
                        Pos2::new(clock_at.left() - TRAY_ICON, tray.top()),
                        Vec2::splat(TRAY_ICON),
                    );
                    let tip =
                        "This server is missing features some components need. Click for details.";
                    if crate::compat_alert::tray_sign(ui, sign_at, tip) {
                        self.open(Target::MyServer);
                        self.show_compat_tab();
                    }
                }
                if net {
                    // leftmost in the tray, so the icons that come and go do not move it around
                    let shift = (u8::from(warn) + u8::from(shield)) as f32 * TRAY_ICON;
                    let icon_at = Rect::from_min_size(
                        Pos2::new(clock_at.left() - TRAY_ICON - shift, tray.top()),
                        Vec2::splat(TRAY_ICON),
                    );
                    let rates = self.monitor.now();
                    let (rx, tx) = rates.map_or((false, false), |r| {
                        (
                            r.rx_total() > monitor::ACTIVE,
                            r.tx_total() > monitor::ACTIVE,
                        )
                    });
                    let response = crate::task_manager::tray_icon(ui, icon_at, rx, tx)
                        .on_hover_text(monitor::tooltip(rates));
                    if response.double_clicked() {
                        self.open(Target::TaskManager);
                        self.task_manager.tab = crate::task_manager::NETWORKING;
                    }
                }
                #[cfg(feature = "devtools")]
                {
                    let shift =
                        (u8::from(warn) + u8::from(shield) + u8::from(net)) as f32 * TRAY_ICON;
                    let bug_at = Rect::from_min_size(
                        Pos2::new(clock_at.left() - TRAY_ICON - shift, tray.top()),
                        Vec2::splat(TRAY_ICON),
                    );
                    self.devtools_tray(ui, bug_at);
                }
                if shield {
                    let shift = if warn { TRAY_ICON } else { 0.0 };
                    let icon_at = Rect::from_min_size(
                        Pos2::new(clock_at.left() - TRAY_ICON - shift, tray.top()),
                        Vec2::splat(TRAY_ICON),
                    );
                    #[cfg(feature = "native")]
                    {
                        let clicked = ui
                            .scope_builder(egui::UiBuilder::new().max_rect(icon_at), |ui| {
                                self.updates.shield(ui)
                            })
                            .inner;
                        if clicked {
                            self.updates.open_dialog();
                        }
                    }
                    #[cfg(not(feature = "native"))]
                    let _ = icon_at;
                }
            });
        });
    }
}

// the live numbers behind the tray icon and the Task Manager
impl Desktop {
    /// where this desktop's numbers come from.
    fn monitor_backend(&self) -> Backend {
        if let Some(demo) = &self.synthetic {
            return Backend::Demo(demo.clone());
        }
        #[cfg(target_arch = "wasm32")]
        return Backend::Web;
        #[cfg(all(not(target_arch = "wasm32"), feature = "native"))]
        if self.services.is_local() {
            return Backend::Local;
        }
        #[cfg(not(target_arch = "wasm32"))]
        Backend::Off(
            "Monitoring another server needs the bridge connection, which is not available yet."
                .into(),
        )
    }

    /// take in what the background read brought and ask for the next sample: every second while
    /// the Task Manager is showing, every other second for the tray alone.
    fn monitor_tick(&mut self, ctx: &egui::Context) {
        let backend = self.monitor_backend();
        self.monitor.use_backend(backend);
        if self.monitor.poll() {
            ctx.request_repaint();
        }
        let shown = self
            .windows
            .iter()
            .any(|w| w.target == Target::TaskManager && !w.minimized);
        let (every, procs) = monitor::cadence(shown);
        let now = ctx.input(|i| i.time);
        self.monitor.tick(now, every, procs, &crate::wake(ctx));
    }
}

// what the taskbar and the menu ask about updates, on builds with or without them
impl Desktop {
    /// is the update shield showing in the tray?
    fn update_shield(&self) -> bool {
        #[cfg(feature = "native")]
        return self.updates.visible();
        #[cfg(not(feature = "native"))]
        false
    }

    /// is a rebuild running?
    pub(super) fn is_rebuilding(&self) -> bool {
        #[cfg(feature = "native")]
        return self.is_rebuilding_native();
        #[cfg(not(feature = "native"))]
        false
    }

    /// are automatic updates on? `None` where there are no updates to turn on (the browser).
    fn updates_enabled(&self) -> Option<bool> {
        #[cfg(feature = "native")]
        return Some(self.updates.enabled());
        #[cfg(not(feature = "native"))]
        None
    }
}

/// the part of the screen above the taskbar.
fn desktop_area(ctx: &egui::Context) -> Rect {
    let screen = ctx.content_rect();
    Rect::from_min_max(screen.min, screen.max - Vec2::new(0.0, TASKBAR))
}

/// how big a window opens, before it is cut down to fit the desktop: the console as 80 by 24
/// cells of its own font, the Welcome window as wide as its two columns, any other as wide as
/// the target gives. a height is only worked out for the console, the rest are as tall as their
/// contents.
fn window_size(ctx: &egui::Context, target: Target) -> Vec2 {
    let (width, height) = target.size();
    let style = ctx.global_style();
    match target {
        Target::Console => {
            let font = egui::TextStyle::Monospace.resolve(&style);
            let cell = ctx.fonts_mut(|fonts| {
                let one = fonts.layout_no_wrap("M".into(), font.clone(), egui::Color32::BLACK);
                Vec2::new(one.size().x, fonts.row_height(&font))
            });
            // the window's frame and margins, the black box's own border and its scroll bar
            let bar = style.spacing.scroll.allocated_width();
            let round =
                windows::chrome() + Vec2::new(2.0 * theme::SUNKEN + bar, 2.0 * theme::SUNKEN);
            Vec2::new(
                lanlord_app::desktop::CONSOLE_COLS * cell.x,
                lanlord_app::desktop::CONSOLE_ROWS * cell.y,
            ) + round
        }
        Target::Welcome => Vec2::new(
            windows::chrome().x
                + lanlord_app::welcome::PANEL
                + lanlord_app::welcome::TOPIC_BUTTON
                + style.spacing.item_spacing.x,
            height,
        ),
        _ => Vec2::new(width, height),
    }
}

/// where the `n`th window opens: stepped down and right of the last one, and never so far
/// that it hangs off the desktop or covers the icons.
fn cascade(n: usize, area: Rect, size: Vec2) -> Pos2 {
    // each window sits one title bar lower and, as far, to the right of the one before
    let step = (n % 8) as f32;
    let x = (area.left() + ICON_COLUMN + step * windows::CHROME).min(area.right() - size.x);
    let y = (area.top() + ICON_MARGIN + step * windows::CHROME).min(area.bottom() - size.y);
    Pos2::new(x.max(area.left()), y.max(area.top()))
}

// the windows
impl Desktop {
    fn show_windows(&mut self, ctx: &egui::Context) {
        let mut windows = std::mem::take(&mut self.windows);
        let before = std::mem::take(&mut self.measured);
        let mut close = Vec::new();
        let area = desktop_area(ctx);
        for (n, window) in windows.iter_mut().enumerate() {
            if window.minimized {
                continue;
            }
            let target = window.target;
            // only the width is chosen; the height is whatever the contents need
            let size = window_size(ctx, target);
            let width = size.x.min(area.width() - ICON_COLUMN - ICON_MARGIN);
            // only the console has a height of its own; the rest are as tall as their contents
            let height = (target == Target::Console).then_some(size.y);
            let start = if target.is_dialog() {
                area.center()
            } else {
                // where it sits depends on how tall it is: what it was last frame, or the least
                // a window can be
                let tall = height
                    .or_else(|| before.iter().find(|m| m.0 == target).map(|m| m.1.height()))
                    .unwrap_or(windows::chrome().y + controls::FILL_MIN);
                cascade(n, area, Vec2::new(width, tall))
            };
            let place = windows::Placement {
                start,
                width,
                area,
                growable: target.growable(),
                height,
                // an empty list would open as a strip: the hosts list opens at its usual height
                floor: target.floor(),
            };
            let shown = windows::show(
                ctx,
                target.id(),
                target.title(),
                place,
                self.active == Some(target),
                target.is_dialog(),
                |ui| self.contents(ui, target),
            );
            if let Some(shown) = shown {
                self.measured.push((target, shown.rect, shown.content));
                if shown.actions.close {
                    close.push(target);
                } else if shown.actions.minimize {
                    window.minimized = true;
                } else if shown.clicked {
                    self.active = Some(target);
                }
            }
        }
        #[cfg(feature = "devtools")]
        self.wm_overlay(ctx);
        close.append(&mut self.close_requests);
        windows.retain(|w| !close.contains(&w.target));
        self.windows = windows;
        if let Some(target) = self.open_next.take() {
            self.open(target);
            if std::mem::take(&mut self.compat_next) {
                self.show_compat_tab();
            }
        }
        if close.contains(&Target::DateTime) {
            // what the window changed shows on the taskbar clock, and so does the server's own drift
            self.feed.refresh();
        }
    }

    fn account_window(&mut self, ui: &mut Ui) {
        let host = self.host();
        let nodes = self.account.view(&self.services, host.as_deref());
        if let (Some(msg), Some(host)) = (view::show(ui, &nodes), host.as_deref()) {
            self.account.update(msg, &self.services, host);
        }
    }

    /// the Networks window; what it asks the desktop to do. an open window reads the status
    /// again every few seconds, so it asks to be drawn again.
    fn networks_window(&mut self, ui: &mut Ui) -> Option<networks_window::Ask> {
        let host = self.host();
        let now = ui.input(|i| i.time);
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(
                networks_window::STATUS_EVERY,
            ));
        let nodes = self.networks.view(&self.services, host.as_deref(), now);
        match (view::show(ui, &nodes), host.as_deref()) {
            (Some(msg), Some(host)) => self.networks.update(msg, &self.services, host, now),
            _ => None,
        }
    }

    /// the properties box and the Network Connection Details list.
    fn network_dialog(&mut self, ctx: &egui::Context) {
        let host = self.host();
        if let Some(dialog) = self.networks.dialog() {
            if let (Some(msg), Some(host)) = (view::dialog(ctx, &dialog), host.as_deref()) {
                let now = ctx.input(|i| i.time);
                self.networks.update(msg, &self.services, host, now);
            }
        }
    }

    fn permissions_window(&mut self, ui: &mut Ui) {
        let (host, writable) = (self.host(), Target::Permissions.writable(&self.grants()));
        let nodes = self
            .permissions
            .view(&self.services, host.as_deref(), writable);
        if let (Some(msg), Some(host)) = (view::show(ui, &nodes), host.as_deref()) {
            self.permissions.update(msg, &self.services, host, writable);
        }
    }

    fn progress_window(&mut self, ui: &mut Ui) {
        if let Some(msg) = view::show(ui, &self.console.apply.view()) {
            self.console.apply.update(msg);
        }
        if self.console.apply.take_done() {
            self.close_requests.push(Target::Progress);
        }
    }

    fn contents(&mut self, ui: &mut Ui, target: Target) {
        match target {
            Target::Module(i) => self.module_window(ui, &MODULES[i]),
            Target::MyServer => {
                let host = self.host();
                self.my_server
                    .set_read_only(!Target::MyServer.writable(&self.grants()));
                my_server::show(&mut self.my_server, ui, &self.services, host.as_deref());
            }
            Target::Networks => {
                let ask = self.networks_window(ui);
                if ask == Some(networks_window::Ask::Dns) {
                    self.open_next = MODULES
                        .iter()
                        .position(|m| m.id == "dns")
                        .map(Target::Module);
                }
            }
            Target::Console => self.console_window(ui),
            Target::Changelog => {
                view::show(ui, &lanlord_app::changelog::view::<()>());
            }
            Target::Hosts => self.hosts_window(ui),
            Target::Settings => {
                let host = self.host();
                self.settings.show(ui, &self.services, host.as_deref());
                if self.settings.take_saved() {
                    self.components.invalidate();
                }
            }
            Target::DateTime => {
                let host = self.host();
                if self.clock.show(ui, &self.services, host.as_deref()) {
                    self.close_requests.push(Target::DateTime);
                }
            }
            Target::TaskManager => {
                let admin = self.is_admin();
                if let Some(crate::task_manager::Ask::End(pid)) =
                    self.task_manager.show(ui, &self.monitor, admin)
                {
                    // the server checks again; this only keeps a non-admin from asking
                    if admin {
                        self.monitor.end(pid, &crate::wake(ui.ctx()));
                    }
                }
            }
            Target::Welcome => {
                let windows = lanlord_app::desktop::windows(&self.icons);
                // measure every topic, so the box is as tall as the tallest one's. the heights
                // change only with the width or the scale, so they are kept
                let key = (ui.available_width(), ui.ctx().pixels_per_point());
                let heights = match &self.welcome_heights {
                    Some((w, p, heights)) if (*w, *p) == key => heights.clone(),
                    _ => {
                        let heights: Vec<f32> = (0..Welcome::topics())
                            .map(|t| {
                                let mut probe = ui.new_child(
                                    egui::UiBuilder::new()
                                        .id_salt(("welcome-probe", t))
                                        .max_rect(ui.available_rect_before_wrap())
                                        .invisible(),
                                );
                                view::show(&mut probe, &self.welcome.view_of(t, &windows, 0.0));
                                probe.min_rect().height()
                            })
                            .collect();
                        self.welcome_heights = Some((key.0, key.1, heights.clone()));
                        heights
                    }
                };
                let tallest = heights.iter().copied().fold(0.0, f32::max);
                let slack = tallest - heights[self.welcome.topic()];
                if let Some(msg) = view::show(
                    ui,
                    &self.welcome.view_of(self.welcome.topic(), &windows, slack),
                ) {
                    self.welcome.update(msg, &windows);
                }
                if self.welcome.take_compat() {
                    self.compat_next = true;
                }
                if let Some(target) = self.welcome.take_open() {
                    self.open_next = Some(target);
                }
                if self.welcome.take_done() {
                    self.close_requests.push(Target::Welcome);
                }
            }
            Target::MyAccount => self.account_window(ui),
            Target::Permissions => self.permissions_window(ui),
            Target::Progress => self.progress_window(ui),
            Target::Components => {
                let host = self.host();
                components_window::show(&mut self.components, ui, &self.services, host.as_deref());
                if self.components.take_done() {
                    self.close_requests.push(Target::Components);
                }
                if self.components.take_compat() {
                    self.open_next = Some(Target::MyServer);
                    self.compat_next = true;
                }
            }
        }
    }

    /// keep the components of the server in step, and the desktop icons with them: the
    /// built-in windows plus the components it has switched on.
    fn sync_components(&mut self) {
        let host = self.host();
        #[cfg(test)]
        if let Some(pinned) = &self.pinned {
            let icons = lanlord_app::desktop::icons(Some(pinned));
            if icons != self.icons {
                self.icons = icons;
            }
            return;
        }
        if let Some(host) = &host {
            self.components.refresh(&self.services, host);
        }
        let icons = lanlord_app::desktop::icons(self.enabled().as_ref());
        if icons != self.icons {
            self.icons = icons;
            self.selected = None;
        }
    }

    /// what the server has switched on, once its settings were read.
    fn enabled(&self) -> Option<lanlord_app::components::Selection> {
        #[cfg(test)]
        if let Some(pinned) = &self.pinned {
            return Some(pinned.clone());
        }
        let host = self.host()?;
        self.components
            .settings
            .belongs_to(&host)
            .then(|| self.components.current().cloned())
            .flatten()
    }

    /// the positions in `MODULES` of the components the server has switched on.
    fn enabled_modules(&self) -> Vec<usize> {
        lanlord_app::desktop::modules(self.enabled().as_ref())
    }

    fn module_window(&mut self, ui: &mut Ui, module: &'static Module) {
        view::show::<()>(ui, &module_window::header(module));
        let host = self.host();
        match module.id {
            "nt4-domain" => self.nt4.show(ui, &self.services, host.as_deref()),
            "dns" => self.dns.show(ui, &self.services, host.as_deref()),
            "remote-commands" => {
                let machines = self.nt4.settings.machines();
                self.commands
                    .show(ui, &self.services, host.as_deref(), machines);
            }
            _ => {
                let nodes = module_window::plain(module, &self.services, host.as_deref());
                if let Some(msg) = view::show(ui, &nodes) {
                    module_window::update(msg, module, &self.services, host.as_deref());
                }
            }
        }
    }

    fn console_window(&mut self, ui: &mut Ui) {
        view::show::<()>(ui, &self.console.view());
    }

    fn hosts_window(&mut self, ui: &mut Ui) {
        let nodes = hosts_window::view(&self.hosts, &self.services, self.services.is_local());
        let Some(msg) = view::show(ui, &nodes) else {
            return;
        };
        let ask = hosts_window::update(msg, &mut self.hosts, &self.services);
        if ask == Some(hosts_window::Ask::AddHost) && self.add_host.is_none() {
            self.add_host = Some(TextDialog::new(
                "add-host",
                "Add a host",
                "Gateway as user@host:",
                false,
            ));
        }
    }
}

#[cfg(test)]
mod clock_tests;
#[cfg(test)]
mod components_tests;
#[cfg(test)]
mod harness;
#[cfg(test)]
mod login_tests;
#[cfg(test)]
mod menu_tests;
#[cfg(test)]
mod server_tests;
#[cfg(test)]
mod update_tests;
#[cfg(test)]
mod window_tests;
#[cfg(test)]
mod tests {
    use super::harness::*;
    use super::*;
    use lanlord_app::log::Kind;

    #[test]
    fn the_empty_desktop_draws() {
        let (mut desktop, ctx, _) = desktop();
        frames(&mut desktop, &ctx, 3);
        assert_eq!(desktop.icons.len(), MODULES.len() + 5);
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
    fn every_window_opens_as_tall_as_its_contents_and_no_taller_than_the_desktop() {
        for size in [Vec2::new(1280.0, 630.0), Vec2::new(800.0, 500.0)] {
            let (mut desktop, ctx, _) = desktop();
            with_host(&mut desktop, &ctx);
            frames_on(&mut desktop, &ctx, size, 6);
            let mut targets = desktop.icons.clone();
            targets.extend([
                Target::DateTime,
                Target::Components,
                Target::Welcome,
                Target::Changelog,
            ]);
            for target in targets {
                desktop.windows.clear();
                desktop.open(target);
                frames_on(&mut desktop, &ctx, size, 9);
                let area = desktop_area(&ctx);
                let &(_, rect, content) = desktop
                    .measured
                    .iter()
                    .find(|m| m.0 == target)
                    .unwrap_or_else(|| panic!("{} was not drawn", target.title()));
                let name = target.title();
                // a 20 pixel title bar and a 3 pixel border all round, around the contents
                let wanted = if target == Target::Console {
                    window_size(&ctx, target).y.min(area.height())
                } else {
                    content + windows::CHROME
                };
                if wanted <= area.height() {
                    assert!(
                        (rect.height() - wanted).abs() <= 1.0,
                        "{name}: {rect:?} {wanted}"
                    );
                } else {
                    assert!(rect.height() <= area.height() + 1.0, "{name}: {rect:?}");
                }
                assert!(
                    area.expand(1.0).contains_rect(rect),
                    "{name} {rect:?} {area:?}"
                );
                if target.is_dialog() {
                    let (cx, cy) = (area.center().x, area.center().y);
                    let off = (rect.center().x - cx)
                        .abs()
                        .max((rect.center().y - cy).abs());
                    assert!(
                        off < 2.0 || rect.height() >= area.height() - 2.0,
                        "{name} {rect:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_welcome_window_opens_the_changelog_window() {
        let (mut desktop, ctx, _) = desktop();
        desktop.open(Target::Welcome);
        frames(&mut desktop, &ctx, 3);
        assert!(press(&mut desktop, &ctx, "Changelog"), "the topic button");
        assert!(press(&mut desktop, &ctx, "Open Changelog"));
        frames(&mut desktop, &ctx, 3);
        assert!(desktop
            .windows
            .iter()
            .any(|w| w.target == Target::Changelog));
    }

    #[test]
    fn every_window_has_a_width_that_fits_the_default_desktop() {
        let (mut desktop, ctx, _) = desktop();
        frames(&mut desktop, &ctx, 1);
        let area = desktop_area(&ctx);
        for target in desktop.icons.clone() {
            assert!(
                window_size(&ctx, target).x + ICON_COLUMN + ICON_MARGIN <= area.width(),
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
        // the list starts under the title bar, the label and the window margin, and the window
        // is taller than its contents, so it is found from the top
        let top = desktop
            .measured
            .iter()
            .find(|m| m.0 == Target::Hosts)
            .unwrap()
            .1;
        click_at(
            &mut desktop,
            &ctx,
            Pos2::new(top.left() + 30.0, top.top() + 48.0),
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
        let seen = lanlord_app::log::subscribe();
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
            lanlord_app::log::emit(kind, text);
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

    #[test]
    fn an_apply_opens_its_progress_dialog_and_close_ends_it() {
        use lanlord_app::apply_progress::{ApplyEvent, Msg, Stats};
        let (mut desktop, ctx, _) = desktop();
        let open = |d: &Desktop| d.windows.iter().any(|w| w.target == Target::Progress);
        desktop.console.apply.event(&ApplyEvent::Total { n: 2 });
        desktop.console.apply.event(&ApplyEvent::TaskStart {
            name: "write the config".into(),
        });
        frames(&mut desktop, &ctx, 3);
        assert!(open(&desktop), "the first event opens the dialog");
        let finished = ApplyEvent::Finished {
            stats: Stats::default(),
        };
        desktop.console.apply.event(&finished);
        frames(&mut desktop, &ctx, 2);
        desktop.console.apply.update(Msg::Close);
        frames(&mut desktop, &ctx, 2);
        assert!(!open(&desktop), "Close closes it");
    }
}
