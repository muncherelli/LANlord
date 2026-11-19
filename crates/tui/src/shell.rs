//! the terminal desktop: the same desktop, menu, windows and boxes the gui shows, drawn in
//! character cells. the windows come from `lanlord_app` as layout trees, so this file only
//! places them, routes the mouse and keyboard to them and hands their messages back.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use lanlord_app::account_window::AccountWindow;
use lanlord_app::admin_setup::{self, AdminSetup};
use lanlord_app::clock_window::{ClockFeed, ClockPanel};
use lanlord_app::command_panel::CommandPanel;
use lanlord_app::components::{Components, Selection};
use lanlord_app::console::Console;
use lanlord_app::desktop::{self, Target};
use lanlord_app::dns_panel::DnsPanel;
use lanlord_app::form::{Form, FormAnswer};
use lanlord_app::gateway::Gateway;
use lanlord_app::hostlist::HostList;
use lanlord_app::login::{self, Auth, LoginBox, SignedIn};
use lanlord_app::logon_panel::LogonPanel;
use lanlord_app::menu::{self, Action, Row};
use lanlord_app::monitor::{self, Monitor};
use lanlord_app::my_server::MyServer;
use lanlord_app::networks_window::{self, NetworksWindow};
use lanlord_app::nt4_panel::Nt4Panel;
use lanlord_app::permissions_window::PermissionsWindow;
use lanlord_app::rebuild::{Rebuild, Takeover};
use lanlord_app::services::Services;
use lanlord_app::settings_window::SettingsWindow;
use lanlord_app::tasks::PasswordRequest;
use lanlord_app::updates::Notice;
use lanlord_app::users_panel::UsersPanel;
use lanlord_app::view::{Dialog, Node};
use lanlord_app::wake::Wake;
use lanlord_app::welcome::Welcome;
use lanlord_app::{hosts_window, module_window};
use lanlord_core::modules::MODULES;
use lanlord_core::power::Power;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::Widget;
use zeroize::Zeroizing;

use crate::layout;
use crate::paint::chrome::{Menu, TaskButton, Taskbar, TaskbarLayout, WindowFrame, WindowLayout};
use crate::paint::desktop::{icon_rect, Desktop as Teal, GlyphTile};
use crate::paint::mouse::{Gesture, Mouse};
use crate::paint::palette::{Palette, BLACK, GRAY, NAVY, WHITE};
use crate::paint::{fill, style, text};
use crate::view::{self, Input, Painter, Part, Region, ViewState};

/// the warning item in the taskbar (F8 opens it too).
const COMPAT_ALERT: &str = "! Compatibility (F8)";

/// the desktop, and for each window its target, whether it is minimised and whether it is
/// maximised, as they were when the windows were last fitted.
type Fitted = (Rect, Vec<(Target, bool, bool)>);

/// one open window: where it is and whether it is tucked into the taskbar.
struct Win {
    target: Target,
    rect: Rect,
    minimized: bool,
    /// where it was before it was maximised.
    restore: Option<Rect>,
}

/// a box that belongs to the desktop rather than to a window.
enum Modal {
    Confirm(Power, String, Form),
    About(Form),
    Password(PasswordRequest, Form),
    AddHost(Form),
    Secret(Form),
    /// rebuild this build and start it again, with a tick for pulling trunk first.
    Rebuild(Form),
    /// a rebuild that did not work; the box only says why.
    Failure(Form),
}

/// who a region or a keystroke belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner {
    Window(Target),
    Dialog,
}

/// an open drop-down's owner, the path of its choice and each option's row and key.
type OpenDropdown = (Owner, Vec<usize>, Vec<(Rect, usize)>);

/// what was drawn last frame, for hit testing.
#[derive(Default)]
struct Hits {
    desktop: Rect,
    icons: Vec<Rect>,
    taskbar: Option<TaskbarLayout>,
    task_targets: Vec<Target>,
    menu: Option<(Rect, Vec<(Rect, usize)>)>,
    /// the Developer submenu beside it.
    submenu: Option<(Rect, Vec<(Rect, usize)>)>,
    /// bottom to top.
    windows: Vec<(Target, Rect, WindowLayout, Vec<Region>)>,
    dialog: Option<(Rect, Vec<Region>)>,
    dropdown: Option<OpenDropdown>,
}

/// one pass over a window's tree: draw it, act on it, or ask about one of its nodes.
enum Pass<'b> {
    Draw {
        area: Rect,
        buf: &'b mut Buffer,
        active: bool,
    },
    Act {
        path: Vec<usize>,
        input: Input,
    },
    Query {
        path: Vec<usize>,
    },
    /// how many rows the tree takes at `width`.
    Measure {
        width: u16,
    },
}

/// what a pass found.
#[derive(Default)]
struct Answer {
    regions: Vec<Region>,
    want_focus: Option<String>,
    field: Option<view::FieldValue>,
    list: Option<(Option<usize>, usize)>,
    tab: Option<(usize, usize)>,
    choice: Option<(Vec<(usize, String)>, String)>,
    /// the field at the queried path sends its own message on Enter.
    enters: bool,
    /// the calendar at the queried path: its month, selected day and whether it can be picked.
    calendar: Option<(i32, u32, chrono::NaiveDate, bool)>,
    height: u16,
    /// the columns the tree wants when nothing limits it.
    width: u16,
}

/// run `pass` over `nodes`; the message it produced, if any.
fn drive<M: Clone>(
    nodes: &[Node<M>],
    pass: &mut Pass,
    state: &ViewState,
    palette: Palette,
) -> (Answer, Option<M>) {
    drive_at(nodes, pass, state, palette, &[])
}

/// like `drive`, for a tree whose regions carry `prefix` in front of their paths, so that two
/// trees can share one window (a tab strip and the page under it). an action or a question
/// about a path without the prefix is not for this tree.
fn drive_at<M: Clone>(
    nodes: &[Node<M>],
    pass: &mut Pass,
    state: &ViewState,
    palette: Palette,
    prefix: &[usize],
) -> (Answer, Option<M>) {
    let mut answer = Answer::default();
    let own = |path: &[usize]| path.strip_prefix(prefix).map(<[usize]>::to_vec);
    match pass {
        Pass::Draw { area, buf, active } => {
            let area = *area;
            let mut painter = Painter::new(palette, state, *active);
            let total = painter.height(nodes, area.width, Some(area.height));
            if total <= area.height {
                painter.draw(nodes, area, buf, prefix);
            } else {
                // a body taller than its window scrolls with the wheel
                let offset = state
                    .scroll
                    .get("#body")
                    .copied()
                    .unwrap_or(0)
                    .min(usize::from(total - area.height)) as u16;
                let mut tall = Buffer::empty(Rect::new(0, 0, area.width, total));
                let whole = tall.area;
                fill(&mut tall, whole, palette, GRAY);
                painter.draw(nodes, whole, &mut tall, prefix);
                view::blit(&tall, offset, area, buf);
                painter
                    .regions
                    .retain(|r| r.rect.y >= offset && r.rect.bottom() <= offset + area.height);
                for region in &mut painter.regions {
                    region.rect.x += area.x;
                    region.rect.y = region.rect.y - offset + area.y;
                }
                painter.regions.push(Region {
                    rect: area,
                    path: prefix.to_vec(),
                    part: Part::Scroll,
                    focusable: false,
                    key: "#body".into(),
                });
            }
            answer.want_focus = view::wants_focus(nodes, &painter.regions, prefix);
            answer.regions = painter.regions;
            (answer, None)
        }
        Pass::Act { path, input } => {
            let msg = own(path).and_then(|path| view::act(nodes, &path, input.clone()));
            (answer, msg)
        }
        Pass::Measure { width } => {
            let painter = Painter::new(palette, state, false);
            answer.height = painter.height(nodes, *width, None);
            answer.width = painter.natural_width(nodes);
            (answer, None)
        }
        Pass::Query { path } => {
            if let Some(path) = own(path) {
                answer.field = view::field_value(nodes, &path);
                answer.list = view::list_state(nodes, &path);
                answer.tab = view::tab_state(nodes, &path);
                answer.choice = view::choice_state(nodes, &path);
                answer.enters = view::field_enters(nodes, &path);
                answer.calendar = view::calendar_state(nodes, &path);
            }
            (answer, None)
        }
    }
}

pub struct Shell {
    palette: Palette,
    services: Services,
    hosts: HostList,
    console: Console,
    /// when the shell started; the progress dialog times an apply on it.
    epoch: std::time::Instant,
    my_server: MyServer,
    settings: SettingsWindow,
    components: Components,
    welcome: Welcome,
    users: UsersPanel,
    networks: NetworksWindow,
    account: AccountWindow,
    permissions: PermissionsWindow,
    nt4: Nt4Panel,
    logon: LogonPanel,
    dns: DnsPanel,
    commands: CommandPanel,
    /// the open tab of the NT4 Domain window: Settings, Logins or Logon scripts.
    nt4_tab: usize,
    /// the Date/Time Properties window, and the server's time shown in the taskbar.
    clock_panel: ClockPanel,
    feed: ClockFeed,
    /// the Task Manager's numbers (read only while its window is open) and how its list is sorted.
    monitor: Monitor,
    top_order: monitor::Order,
    started: std::time::Instant,
    /// how to sign in; `None` when lanlord runs on the gateway itself (or in a test).
    auth: Option<Arc<dyn Auth>>,
    /// the logon box, shown instead of the desktop until someone is signed in.
    login: Option<LoginBox>,
    signed_in: Option<SignedIn>,
    /// showing the made-up demo server.
    demo: bool,
    /// the screen root lands on to create the first administrator, before the desktop.
    admin_setup: Option<AdminSetup>,
    icons: Vec<Target>,
    selected: Option<usize>,
    /// bottom to top; the last visible one has the keyboard.
    windows: Vec<Win>,
    active: Option<Target>,
    /// the LANlord menu is open, with the keyboard on the row of this action. the row is found
    /// again each frame, so the rows changing under it (the components arriving) keep it put.
    menu: Option<Action>,
    /// the Developer submenu is open beside it, with the keyboard on this row.
    submenu: Option<usize>,
    /// the update notice: the background check, what it found and the updater.
    updates: Notice,
    /// the checkout a development build rebuilds from, and the rebuild in progress.
    rebuild: Rebuild,
    /// the new build to start in this terminal once the tui has ended, and the server it signs
    /// in to again.
    restart: Option<(PathBuf, Option<String>)>,
    /// the developer tools of a development build; nothing in a release build.
    #[cfg_attr(not(feature = "devtools"), allow(dead_code))]
    devtools: devtools::State,
    modal: Option<Modal>,
    views: HashMap<&'static str, ViewState>,
    /// the desktop and the windows as they were when they were last fitted to their content.
    fitted: Option<Fitted>,
    /// an input event may have changed what a window shows, so the next frame fits again.
    refit_due: bool,
    /// the count of finished background tasks at the last fit.
    fitted_tasks: u64,
    dialog_view: ViewState,
    /// the box the dialog view belongs to, so a new box starts fresh.
    dialog_id: Option<&'static str>,
    /// a window being dragged by its title bar, and where in the bar it was grabbed.
    drag: Option<(Target, u16, u16)>,
    /// windows that asked to be closed from inside (OK or Cancel).
    close_requests: Vec<Target>,
    /// windows a click asked to open, opened after the frame.
    open_next: Vec<Target>,
    /// My Server opens on its Compatibility tab at the next chance.
    compat_next: bool,
    mouse: Mouse,
    hits: Hits,
    /// the clock text; the local time unless a test sets it.
    pub clock: Option<String>,
    pub quit: bool,
}

/// the actions of the menu's item rows, top to bottom.
fn menu_actions(rows: &[Row]) -> impl Iterator<Item = &Action> {
    rows.iter().filter_map(|row| match row {
        Row::Item(item) => Some(&item.action),
        Row::Separator => None,
    })
}

/// the row holding the highlighted action; the first row when it is gone.
fn menu_row_of(rows: &[Row], action: &Action) -> usize {
    rows.iter()
        .position(|row| matches!(row, Row::Item(item) if item.action == *action))
        .unwrap_or(0)
}

/// the console, hearing the log. tests detach it as plain hardening, so a desktop hears nothing
/// unless the test asks to.
fn listening() -> Console {
    if cfg!(test) {
        Console::detached()
    } else {
        Console::new(Wake::none())
    }
}

impl Shell {
    /// a desktop that talks to gateways through `gateway` and keeps its hosts in `hosts`.
    pub fn new(gateway: Arc<dyn Gateway>, hosts: HostList, palette: Palette) -> Self {
        Self {
            palette,
            services: Services::new(gateway, Wake::none()),
            hosts,
            console: listening(),
            epoch: std::time::Instant::now(),
            my_server: MyServer::default(),
            settings: SettingsWindow::default(),
            components: Components::default(),
            welcome: Welcome::new(None),
            users: UsersPanel::default(),
            networks: NetworksWindow::default(),
            account: AccountWindow::default(),
            permissions: PermissionsWindow::default(),
            nt4: Nt4Panel::default(),
            logon: LogonPanel::default(),
            dns: DnsPanel::default(),
            commands: CommandPanel::default(),
            nt4_tab: 0,
            clock_panel: ClockPanel::default(),
            feed: ClockFeed::default(),
            monitor: Monitor::default(),
            top_order: monitor::Order::default(),
            started: std::time::Instant::now(),
            auth: None,
            login: None,
            signed_in: None,
            demo: false,
            admin_setup: None,
            icons: desktop::icons(None),
            selected: None,
            windows: Vec::new(),
            active: None,
            menu: None,
            submenu: None,
            updates: Notice::quiet(),
            rebuild: Rebuild::new(Takeover::Terminal),
            restart: None,
            devtools: devtools::State::new(),
            modal: None,
            views: HashMap::new(),
            fitted: None,
            refit_due: false,
            fitted_tasks: 0,
            dialog_view: ViewState::default(),
            dialog_id: None,
            drag: None,
            close_requests: Vec::new(),
            open_next: Vec::new(),
            compat_next: false,
            mouse: Mouse::new(),
            hits: Hits::default(),
            clock: None,
            quit: false,
        }
    }

    /// act as if signed in to a made-up server (`LANLORD_FAKE_SERVER=1`), like the gui.
    pub fn use_demo_server(&mut self) {
        self.demo = true;
        self.hosts = HostList::new(None);
        let _ = self.hosts.add("demo@gateway");
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

    /// the title of the whole app, as the gui sets it: "LANlord Client v…" or "LANlord Server v…".
    pub fn title(&self) -> String {
        let host = self.signed_in.as_ref().and(self.host());
        desktop::window_title(self.services.is_local(), host.as_deref())
    }

    /// start at the logon box, like the gui. lanlord on the gateway itself has one server and
    /// no logon.
    pub fn require_login(&mut self, auth: Arc<dyn Auth>) {
        if self.services.is_local() {
            return;
        }
        let last = self
            .hosts
            .last()
            .or(self.hosts.selected())
            .map(String::from);
        self.login = Some(LoginBox::new(auth.clone(), Wake::none(), last.as_deref()));
        self.auth = Some(auth);
    }

    /// the logon box or the first-administrator screen is up instead of the desktop.
    fn before_desktop(&self) -> bool {
        self.login.is_some() || self.admin_setup.is_some()
    }

    /// someone signed in: the server joins the list and every action targets it. root goes to
    /// the screen that creates the first administrator before anything else.
    fn finish_login(&mut self, signed_in: SignedIn) {
        self.login = None;
        if let Err(error) = self.hosts.add(&signed_in.target) {
            lanlord_core::log::emit(lanlord_core::log::Kind::Err, error);
        }
        self.hosts.remember(&signed_in.target);
        self.windows.clear();
        self.active = None;
        self.selected = None;
        self.menu = None;
        self.reset_windows();
        if signed_in.identity.is_root() {
            self.admin_setup = Some(AdminSetup::new());
        }
        let account = signed_in.identity.user.clone();
        self.signed_in = Some(signed_in);
        if self.welcome.logon(&account) {
            self.open(Target::Welcome);
        }
    }

    /// forget what the windows of the last account showed, so the next one starts clean and
    /// cannot answer a box the last one opened.
    fn reset_windows(&mut self) {
        self.networks = NetworksWindow::default();
        self.account = AccountWindow::default();
        self.permissions = PermissionsWindow::default();
        self.my_server = MyServer::default();
        self.settings = SettingsWindow::default();
        self.components = Components::default();
        self.users = UsersPanel::default();
        self.nt4 = Nt4Panel::default();
        self.nt4_tab = 0;
        self.logon = LogonPanel::default();
        self.dns = DnsPanel::default();
        self.commands = CommandPanel::default();
        self.clock_panel = ClockPanel::default();
        self.feed = ClockFeed::default();
        self.monitor = Monitor::default();
        self.top_order = monitor::Order::default();
        self.drag = None;
        self.close_requests.clear();
        self.open_next.clear();
        self.compat_next = false;
    }

    /// keep the Welcome screen's tick box in the preferences file at `path`.
    pub fn set_prefs(&mut self, path: Option<std::path::PathBuf>) {
        self.welcome = Welcome::new(path);
    }

    fn admin_outcome(&mut self, outcome: admin_setup::Outcome) {
        let Some(target) = self.signed_in.as_ref().map(|s| s.target.clone()) else {
            self.admin_setup = None;
            return;
        };
        match outcome {
            // the first-run setup is the web ui's; there is no other way to get one here
            admin_setup::Outcome::Skipped | admin_setup::Outcome::FirstRun { .. } => {
                self.admin_setup = None;
            }
            admin_setup::Outcome::Created(name) => {
                // forget the root entry and ask for a logon as the new account
                self.admin_setup = None;
                if self.hosts.selected() == Some(target.as_str()) {
                    let _ = self.hosts.remove_selected();
                }
                let host = target.split_once('@').map_or(target.as_str(), |(_, h)| h);
                self.return_to_login(
                    Some(format!("{name}@{host}")),
                    Some(format!("Created {name}. Log on with it.")),
                );
            }
        }
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
        let last = self.signed_in.as_ref().map(|s| s.target.clone());
        self.return_to_login(last, None);
    }

    /// show the logon box again with `last` filled in and `note` in its status bar.
    fn return_to_login(&mut self, last: Option<String>, note: Option<String>) {
        if self.leave_demo() {
            return;
        }
        let Some(auth) = self.auth.clone() else {
            return;
        };
        if let Some(target) = self.signed_in.take().map(|s| s.target) {
            let closer = auth.clone();
            std::thread::spawn(move || closer.sign_out(&target));
        }
        self.windows.clear();
        self.active = None;
        self.menu = None;
        self.reset_windows();
        let mut login = LoginBox::new(auth, Wake::none(), last.as_deref());
        if let Some(note) = note {
            login.note(&note);
        }
        self.login = Some(login);
    }

    pub fn open(&mut self, target: Target) {
        if !target.visible(&self.grants()) {
            return;
        }
        if let Some(i) = self.windows.iter().position(|w| w.target == target) {
            let mut win = self.windows.remove(i);
            win.minimized = false;
            self.windows.push(win);
        } else {
            if target == Target::DateTime {
                // a fresh reading of the server every time the window opens
                self.clock_panel.reset();
            }
            if target == Target::Components {
                self.components.open();
            }
            if target == Target::Welcome {
                self.welcome.reopen();
            }
            let rect = self.place(target);
            self.windows.push(Win {
                target,
                rect,
                minimized: false,
                restore: None,
            });
        }
        self.active = Some(target);
        self.menu = None;
    }

    /// where a new window goes: right of the icons, cascading, as big as its content.
    fn place(&mut self, target: Target) -> Rect {
        let area = self.hits.desktop;
        let area = if area.is_empty() {
            Rect::new(0, 0, 100, 30)
        } else {
            area
        };
        let icons_right = (0..self.icons.len())
            .map(|i| icon_rect(area, i).right())
            .max()
            .unwrap_or(0);
        let n = self.windows.len() as u16;
        let x = icons_right + 1 + n * 2;
        let y = area.y + (n % 6);
        let size = self.fit_size(target, area);
        layout::place(size, x, y, area)
    }

    /// how big a window is that fits its content on `desktop`: the width the window asks for,
    /// and as many rows as its tree takes at that width. a growable window (a console, a list)
    /// is never shorter than the least height it opens at. the desktop cuts it down later; what
    /// still does not fit scrolls.
    fn fit_size(&mut self, target: Target, desktop: Rect) -> (u16, u16) {
        let (asked, size_height) = target.size();
        let width = layout::frame_width((asked / 7.0).round() as u16).min(desktop.width);
        // a fixed window is the one size it asks for, whatever it shows
        if target.fixed() {
            let rows = (size_height / 16.0).round() as u16;
            return (width, layout::frame_height(rows).min(desktop.height));
        }
        let measured = self
            .run_window(
                target,
                Pass::Measure {
                    width: layout::content_width(width),
                },
            )
            .height;
        let floor = match target {
            Target::Console => lanlord_app::desktop::CONSOLE_ROWS as u16,
            _ if target.growable() => (target.floor().min(size_height) / 16.0) as u16,
            _ => 0,
        };
        (width, layout::frame_height(measured.max(floor)))
    }

    /// resize every window that has not been maximised to its content, keeping its corner.
    fn refit_windows(&mut self, desktop: Rect) {
        let now = (
            desktop,
            self.windows
                .iter()
                .map(|w| (w.target, w.minimized, w.restore.is_some()))
                .collect::<Vec<_>>(),
        );
        // what a window shows can change with a keypress or when a background task finishes
        let tasks = self.services.finished();
        let moved = std::mem::replace(&mut self.fitted_tasks, tasks) != tasks;
        if !std::mem::take(&mut self.refit_due) && !moved && self.fitted.as_ref() == Some(&now) {
            return;
        }
        self.fitted = Some(now);
        for i in 0..self.windows.len() {
            let (target, rect, hidden) = {
                let win = &self.windows[i];
                (win.target, win.rect, win.minimized || win.restore.is_some())
            };
            if hidden {
                continue;
            }
            let size = self.fit_size(target, desktop);
            self.windows[i].rect = layout::place(size, rect.x, rect.y, desktop);
        }
    }

    fn close(&mut self, target: Target) {
        self.windows.retain(|w| w.target != target);
        if self.active == Some(target) {
            self.active = self
                .windows
                .iter()
                .rev()
                .find(|w| !w.minimized)
                .map(|w| w.target);
        }
    }

    fn raise(&mut self, target: Target) {
        if let Some(i) = self.windows.iter().position(|w| w.target == target) {
            let mut win = self.windows.remove(i);
            win.minimized = false;
            self.windows.push(win);
        }
        self.active = Some(target);
    }

    fn minimize(&mut self, target: Target) {
        if let Some(win) = self.windows.iter_mut().find(|w| w.target == target) {
            win.minimized = true;
        }
        if self.active == Some(target) {
            self.active = self
                .windows
                .iter()
                .rev()
                .find(|w| !w.minimized)
                .map(|w| w.target);
        }
    }

    fn toggle_maximise(&mut self, target: Target) {
        let full = self.hits.desktop;
        if target.fixed() {
            return;
        }
        if let Some(win) = self.windows.iter_mut().find(|w| w.target == target) {
            match win.restore.take() {
                Some(rect) => win.rect = rect,
                None => {
                    win.restore = Some(win.rect);
                    win.rect = full;
                }
            }
        }
    }

    /// keep the components of the server in step, and the desktop icons with them: the
    /// built-in windows plus the components it has switched on.
    fn sync_components(&mut self) {
        if let Some(host) = self.host() {
            self.components.refresh(&self.services, &host);
        }
        let icons = desktop::icons(self.enabled().as_ref());
        if icons != self.icons {
            self.icons = icons;
            self.selected = None;
        }
    }

    /// what the server has switched on, once its settings were read.
    fn enabled(&self) -> Option<Selection> {
        let host = self.host()?;
        self.components
            .settings
            .belongs_to(&host)
            .then(|| self.components.current().cloned())
            .flatten()
    }

    /// what the signed-in account may do. without a sign-in only the demo and a shell on the
    /// gateway itself (where the server decides anyway) show everything.
    fn grants(&self) -> lanlord_core::access::Grants {
        self.signed_in.as_ref().map_or_else(
            || lanlord_core::access::Grants::fallback(self.demo || self.services.is_local()),
            |s| s.identity.grants(),
        )
    }

    fn menu_state(&self) -> Vec<Row> {
        let host = self.host();
        menu::rows(&menu::State {
            host: host.as_deref(),
            signed_in: self.signed_in.as_ref().map(|s| s.identity.user.as_str()),
            switch_server: true,
            grants: self.grants(),
            ready: host.as_deref().is_some_and(|h| !self.services.busy(h)),
            updates: Some(self.updates.enabled()),
            developer: self.rebuild.repo().is_some(),
            modules: desktop::modules(self.enabled().as_ref()),
        })
    }

    fn do_action(&mut self, action: Action) {
        if action == Action::Developer {
            // the submenu opens beside the row; the menu stays up
            self.submenu = self.submenu.is_none().then_some(0);
            return;
        }
        self.menu = None;
        self.submenu = None;
        match action {
            Action::Open(target) => self.open(target),
            Action::Power(power) => {
                if let Some(host) = self.host() {
                    let (title, verb) = match power {
                        Power::Reboot => ("Reboot server", "Restart"),
                        Power::PowerOff => ("Shut down server", "Shut down"),
                    };
                    let form = Form::new("power", title, &format!("{verb} {host} now?"));
                    self.modal = Some(Modal::Confirm(power, host, form));
                }
            }
            Action::About => {
                let about = format!(
                    "{}\nRetro networks, run from one desktop.",
                    desktop::app_title(self.services.is_local())
                );
                self.modal = Some(Modal::About(Form::new("about", "About LANlord", &about)));
            }
            Action::Quit => self.quit = true,
            Action::SwitchServer | Action::LogOff => self.log_off(),
            Action::ToggleUpdates => self.toggle_updates(),
            Action::CheckUpdates => self.check_updates(),
            Action::Rebuild => self.ask_rebuild(),
            Action::Developer | Action::ServerLine => {}
        }
    }
}

// background work, once per frame
impl Shell {
    pub fn poll(&mut self) {
        for target in std::mem::take(&mut self.close_requests) {
            self.close(target);
            if target == Target::DateTime {
                // what the window changed shows on the taskbar clock
                self.feed.refresh();
            }
        }
        for target in std::mem::take(&mut self.open_next) {
            self.open(target);
        }
        if std::mem::take(&mut self.compat_next) {
            if let Some(host) = self.host() {
                self.my_server.update(
                    lanlord_app::my_server::Msg::Tab(lanlord_app::my_server::COMPATIBILITY),
                    &self.services,
                    &host,
                );
            }
        }
        self.poll_maintenance();
        self.refit_due |= self.console.drain();
        self.console.apply.tick(self.epoch.elapsed().as_secs_f64());
        if self.console.apply.take_started() {
            self.open(Target::Progress);
        }
        self.settings.poll();
        if self.settings.take_saved() {
            self.components.invalidate();
        }
        self.sync_components();
        self.users.poll();
        self.networks.poll();
        self.account.poll();
        self.permissions.poll();
        self.nt4.poll();
        self.logon.poll();
        self.dns.poll();
        self.commands.poll();
        self.poll_clock();
        self.poll_monitor();
        self.my_server.refresh_purge();
        if let Some(host) = self.my_server.take_purged() {
            self.after_purge(&host);
        }
        if let Some(signed_in) = self.login.as_mut().and_then(LoginBox::poll) {
            self.finish_login(signed_in);
        }
        if let Some(outcome) = self.admin_setup.as_mut().and_then(AdminSetup::poll) {
            self.admin_outcome(outcome);
        }
        if self.modal.is_none() {
            if let Some(request) = self.services.next_request() {
                let form = Form::new("password", "Password", &request.prompt).secret("Password");
                self.modal = Some(Modal::Password(request, form));
            } else if let Some(path) = self.settings.asking().map(String::from) {
                let form = Form::new("secret", "Set a secret", &path).secret("Value");
                self.modal = Some(Modal::Secret(form));
            }
        }
    }
}

// each window's tree, run through one pass
impl Shell {
    fn networks_body(&mut self, host: Option<&str>, pass: &mut Pass, state: &ViewState) -> Answer {
        let now = self.epoch.elapsed().as_secs_f64();
        let nodes = self.networks.view(&self.services, host, now);
        let (answer, msg) = drive(&nodes, pass, state, self.palette);
        if let (Some(msg), Some(host)) = (msg, host) {
            let ask = self.networks.update(msg, &self.services, host, now);
            if ask == Some(networks_window::Ask::Dns) {
                if let Some(dns) = MODULES.iter().position(|m| m.id == "dns") {
                    self.open_next.push(Target::Module(dns));
                }
            }
        }
        answer
    }

    fn account_body(&mut self, host: Option<&str>, pass: &mut Pass, state: &ViewState) -> Answer {
        let nodes = self.account.view(&self.services, host);
        let (answer, msg) = drive(&nodes, pass, state, self.palette);
        if let (Some(msg), Some(host)) = (msg, host) {
            self.account.update(msg, &self.services, host);
        }
        answer
    }

    fn permissions_body(
        &mut self,
        host: Option<&str>,
        pass: &mut Pass,
        state: &ViewState,
    ) -> Answer {
        let writable = Target::Permissions.writable(&self.grants());
        let nodes = self.permissions.view(&self.services, host, writable);
        let (answer, msg) = drive(&nodes, pass, state, self.palette);
        if let (Some(msg), Some(host)) = (msg, host) {
            self.permissions.update(msg, &self.services, host, writable);
        }
        answer
    }

    /// the open box of the Networks, My Account or Permissions window, drawn and answered.
    fn window_dialog(
        &mut self,
        pass: &mut Pass,
        state: &ViewState,
        escape: bool,
    ) -> Option<(String, Answer)> {
        if let Some(found) = self.update_dialog(pass, state, escape) {
            return Some(found);
        }
        let palette = self.palette;
        let host = self.host();
        if let Some(dialog) = self.networks.dialog() {
            let (answer, msg) = drive_dialog(&dialog, pass, state, palette, escape);
            if let (Some(msg), Some(host)) = (msg, host.as_deref()) {
                let now = self.epoch.elapsed().as_secs_f64();
                self.networks.update(msg, &self.services, host, now);
            }
            return Some((dialog.title, answer));
        }
        if let Some(dialog) = self.account.dialog() {
            let (answer, msg) = drive_dialog(&dialog, pass, state, palette, escape);
            if let (Some(msg), Some(host)) = (msg, host.as_deref()) {
                self.account.update_dialog(msg, &self.services, host);
            }
            return Some((dialog.title, answer));
        }
        let dialog = self.permissions.dialog()?;
        let (answer, msg) = drive_dialog(&dialog, pass, state, palette, escape);
        if let Some(msg) = msg {
            self.permissions.update_dialog(msg);
        }
        Some((dialog.title, answer))
    }

    fn run_window(&mut self, target: Target, mut pass: Pass) -> Answer {
        let key = target.id();
        let mut state = self.views.remove(key).unwrap_or_default();
        let palette = self.palette;
        let host = self.host();
        let answer = match target {
            Target::MyServer => match host.as_deref() {
                Some(host) => {
                    if matches!(pass, Pass::Draw { .. }) {
                        self.my_server.refresh(&self.services, host);
                    }
                    let nodes = self.my_server.view(!self.services.busy(host));
                    let (answer, msg) = drive(&nodes, &mut pass, &state, palette);
                    if let Some(msg) = msg {
                        self.my_server.update(msg, &self.services, host);
                    }
                    answer
                }
                None => {
                    let nodes: Vec<Node<()>> =
                        vec![Node::Text("No server yet. Sign in to one first.".into())];
                    drive(&nodes, &mut pass, &state, palette).0
                }
            },
            Target::Console => drive::<()>(&self.console.view(), &mut pass, &state, palette).0,
            Target::Changelog => {
                drive::<()>(&lanlord_app::changelog::view(), &mut pass, &state, palette).0
            }
            Target::Hosts => {
                let nodes =
                    hosts_window::view(&self.hosts, &self.services, self.services.is_local());
                let (answer, msg) = drive(&nodes, &mut pass, &state, palette);
                if let Some(msg) = msg {
                    let ask = hosts_window::update(msg, &mut self.hosts, &self.services);
                    if ask == Some(hosts_window::Ask::AddHost) && self.modal.is_none() {
                        let form = Form::new("add-host", "Add a host", "Gateway as user@host:")
                            .text("Host", "");
                        self.modal = Some(Modal::AddHost(form));
                    }
                }
                answer
            }
            Target::Settings => {
                self.settings.admin = self.signed_in.as_ref().is_some_and(|s| s.identity.admin);
                let nodes = self.settings.view(&self.services, host.as_deref());
                let (answer, msg) = drive(&nodes, &mut pass, &state, palette);
                if let (Some(msg), Some(host)) = (msg, host.as_deref()) {
                    self.settings.update(msg, &self.services, host);
                }
                answer
            }
            Target::Module(i) => {
                let module = &MODULES[i];
                match module.id {
                    "nt4-domain" => self.nt4_window(module, pass, &state),
                    "dns" => {
                        let mut nodes = module_window::header(module);
                        nodes.extend(self.dns.view(&self.services, host.as_deref()));
                        let (answer, msg) = drive(&nodes, &mut pass, &state, palette);
                        if let (Some(msg), Some(host)) = (msg, host.as_deref()) {
                            self.dns.update(msg, &self.services, host);
                        }
                        answer
                    }
                    "remote-commands" => {
                        let mut nodes = module_window::header(module);
                        let machines = self.nt4.machines().to_vec();
                        nodes.extend(self.commands.view(
                            &self.services,
                            host.as_deref(),
                            &machines,
                        ));
                        let (answer, msg) = drive(&nodes, &mut pass, &state, palette);
                        if let (Some(msg), Some(host)) = (msg, host.as_deref()) {
                            self.commands.update(msg, &self.services, host);
                        }
                        answer
                    }
                    _ => {
                        let mut nodes = module_window::header(module);
                        nodes.extend(module_window::plain(
                            module,
                            &self.services,
                            host.as_deref(),
                        ));
                        let (answer, msg) = drive(&nodes, &mut pass, &state, palette);
                        if let Some(msg) = msg {
                            module_window::update(msg, module, &self.services, host.as_deref());
                        }
                        answer
                    }
                }
            }
            Target::DateTime => {
                let nodes = self.clock_panel.view(&self.services, host.as_deref());
                let (answer, msg) = drive(&nodes, &mut pass, &state, palette);
                if let (Some(msg), Some(host)) = (msg, host.as_deref()) {
                    if self.clock_panel.update(msg, &self.services, host) {
                        self.close_requests.push(Target::DateTime);
                    }
                } else if self.clock_panel.closing() {
                    self.close_requests.push(Target::DateTime);
                }
                answer
            }
            Target::TaskManager => {
                let nodes = monitor::top_view(&self.monitor, self.top_order);
                let (answer, msg) = drive(&nodes, &mut pass, &state, palette);
                if let Some(monitor::TopMsg::Sort(column)) = msg {
                    self.top_order.click(column);
                }
                answer
            }
            Target::Welcome => {
                let windows = desktop::windows(&self.icons);
                let nodes = self.welcome.view(&windows);
                let (answer, msg) = drive(&nodes, &mut pass, &state, palette);
                if let Some(msg) = msg {
                    self.welcome.update(msg, &windows);
                }
                if self.welcome.take_compat() {
                    self.compat_next = true;
                }
                if let Some(target) = self.welcome.take_open() {
                    self.open_next.push(target);
                }
                if self.welcome.take_done() {
                    self.close_requests.push(Target::Welcome);
                }
                answer
            }
            Target::Components => match host.as_deref() {
                Some(host) => {
                    let nodes = self.components.view(!self.services.busy(host));
                    let (answer, msg) = drive(&nodes, &mut pass, &state, palette);
                    if let Some(msg) = msg {
                        self.components.update(msg, &self.services, host);
                    }
                    if self.components.take_done() {
                        self.close_requests.push(Target::Components);
                    }
                    if self.components.take_compat() {
                        self.open_next.push(Target::MyServer);
                        self.compat_next = true;
                    }
                    answer
                }
                None => {
                    let nodes: Vec<Node<()>> =
                        vec![Node::Text("No server yet. Sign in to one first.".into())];
                    drive(&nodes, &mut pass, &state, palette).0
                }
            },
            Target::Networks => self.networks_body(host.as_deref(), &mut pass, &state),
            Target::MyAccount => self.account_body(host.as_deref(), &mut pass, &state),
            Target::Permissions => self.permissions_body(host.as_deref(), &mut pass, &state),
            Target::Progress => {
                let nodes = self.console.apply.view();
                let (answer, msg) = drive(&nodes, &mut pass, &state, palette);
                if let Some(msg) = msg {
                    self.console.apply.update(msg);
                }
                if self.console.apply.take_done() {
                    self.close_requests.push(Target::Progress);
                }
                answer
            }
        };
        if state.focus.is_none() {
            state.focus = answer.want_focus.clone();
        }
        self.views.insert(key, state);
        answer
    }

    /// read the host for the Task Manager while its window is open.
    fn poll_monitor(&mut self) {
        let backend = if self.services.is_local() {
            monitor::Backend::Local
        } else {
            monitor::Backend::Off(
                "Monitoring another server needs the bridge connection, which is not available yet."
                    .into(),
            )
        };
        self.monitor.use_backend(backend);
        self.refit_due |= self.monitor.poll();
        if self.windows.iter().any(|w| w.target == Target::TaskManager) {
            let now = self.started.elapsed().as_secs_f64();
            self.monitor.tick(now, monitor::FAST, true, &Wake::none());
        }
    }

    /// keep the taskbar clock on the server's time: read it once, and take what the Date/Time
    /// window reads or changes.
    fn poll_clock(&mut self) {
        let host = self.host();
        self.feed.watch(&self.services, host.as_deref());
        if let (Some(state), Some(host)) = (self.clock_panel.poll(), host.as_deref()) {
            self.feed.set(host, state);
        }
    }

    /// the NT4 Domain window: its header and tab strip, and the open tab's page under them.
    /// the page's regions carry `PAGE` in front of their paths.
    fn nt4_window(
        &mut self,
        module: &'static lanlord_core::modules::Module,
        mut pass: Pass,
        state: &ViewState,
    ) -> Answer {
        const PAGE: usize = 9;
        let palette = self.palette;
        let host = self.host();
        let mut top: Vec<Node<usize>> = module_window::header(module);
        top.push(Node::Tabs {
            titles: lanlord_app::view::titles(&["Settings", "Logins", "Logon scripts"]),
            current: self.nt4_tab,
            pick: |tab| tab,
            body: Vec::new(),
        });
        // where the strip ends and the page starts
        let (mut top_pass, mut page_pass) = match &mut pass {
            Pass::Draw { area, buf, active } => {
                let painter = Painter::new(palette, state, *active);
                let strip = painter.height(&top[..top.len() - 1], area.width, None) + 1;
                let strip = strip.min(area.height);
                let page = Rect::new(area.x, area.y + strip, area.width, area.height - strip);
                let face = page.intersection(buf.area);
                if face.height >= 3 {
                    crate::paint::chrome::Frame { palette }.render(face, buf);
                }
                let inner = crate::paint::chrome::Frame::inner(page);
                let top_area = Rect::new(area.x, area.y, area.width, strip);
                let active = *active;
                // two draws into one buffer, one after the other
                let mut regions = {
                    let mut p = Pass::Draw {
                        area: top_area,
                        buf,
                        active,
                    };
                    drive(&top, &mut p, state, palette).0.regions
                };
                let page_answer = {
                    let mut p = Pass::Draw {
                        area: inner,
                        buf,
                        active,
                    };
                    self.nt4_page(&mut p, state, host.as_deref(), &[PAGE])
                };
                regions.extend(page_answer.regions);
                return Answer {
                    regions,
                    want_focus: page_answer.want_focus,
                    ..Answer::default()
                };
            }
            Pass::Act { path, input } => (
                Pass::Act {
                    path: path.clone(),
                    input: input.clone(),
                },
                Pass::Act {
                    path: path.clone(),
                    input: input.clone(),
                },
            ),
            Pass::Query { path } => (
                Pass::Query { path: path.clone() },
                Pass::Query { path: path.clone() },
            ),
            Pass::Measure { width } => (
                Pass::Measure { width: *width },
                Pass::Measure { width: *width },
            ),
        };
        if let Pass::Measure { width } = &pass {
            // the header and strip, then the page in its frame
            let width = *width;
            let strip =
                Painter::new(palette, state, false).height(&top[..top.len() - 1], width, None) + 1;
            let mut inner = Pass::Measure {
                width: width.saturating_sub(2),
            };
            let page = self.nt4_page(&mut inner, state, host.as_deref(), &[PAGE]);
            return Answer {
                height: strip + page.height + 2,
                width: page.width + 2,
                ..Answer::default()
            };
        }
        let on_page = match &page_pass {
            Pass::Act { path, .. } | Pass::Query { path } => path.first() == Some(&PAGE),
            _ => false,
        };
        if on_page {
            return self.nt4_page(&mut page_pass, state, host.as_deref(), &[PAGE]);
        }
        let (answer, tab) = drive(&top, &mut top_pass, state, palette);
        if let Some(tab) = tab {
            self.nt4_tab = tab;
        }
        answer
    }

    /// the open tab of the NT4 Domain window, run through `pass`.
    fn nt4_page(
        &mut self,
        pass: &mut Pass,
        state: &ViewState,
        host: Option<&str>,
        prefix: &[usize],
    ) -> Answer {
        let palette = self.palette;
        match self.nt4_tab {
            0 => {
                let nodes = self.nt4.view(&self.services, host);
                let (answer, msg) = drive_at(&nodes, pass, state, palette, prefix);
                if let (Some(msg), Some(host)) = (msg, host) {
                    self.nt4.update(msg, &self.services, host);
                }
                answer
            }
            1 => {
                let nodes = self.users.view(&self.services, host);
                let (answer, msg) = drive_at(&nodes, pass, state, palette, prefix);
                if let (Some(msg), Some(host)) = (msg, host) {
                    self.users.update(msg, &self.services, host);
                }
                answer
            }
            _ => {
                let nodes = self.logon.view(&self.services, host);
                let (answer, msg) = drive_at(&nodes, pass, state, palette, prefix);
                if let (Some(msg), Some(host)) = (msg, host) {
                    self.logon.update(msg, &self.services, host);
                }
                answer
            }
        }
    }

    /// the id of the open box of the update, Networks, My Account or Permissions window.
    fn window_dialog_id(&mut self) -> Option<&'static str> {
        if self.update_dialog_open() {
            return Some("update-box");
        }
        self.networks
            .dialog()
            .map(|dialog| dialog.id)
            .or_else(|| self.account.dialog().map(|dialog| dialog.id))
            .or_else(|| self.permissions.dialog().map(|dialog| dialog.id))
    }

    /// the box on top of everything, if there is one: the desktop's own, then a window's.
    fn dialog_id(&mut self) -> Option<&'static str> {
        if let Some(modal) = &self.modal {
            return Some(match modal {
                Modal::Confirm(..) => "power",
                Modal::About(_) => "about",
                Modal::Password(..) => "password",
                Modal::AddHost(_) => "add-host",
                Modal::Secret(_) => "secret",
                Modal::Rebuild(_) => "rebuild",
                Modal::Failure(_) => "rebuild-failed",
            });
        }
        if let Some(login) = &self.login {
            return Some(login.overlay().map_or("logon", |d| d.id));
        }
        if let Some(setup) = &self.admin_setup {
            return Some(setup.warning().map_or("admin-setup", |d| d.id));
        }
        if let Some(dialog) = self.users.dialog() {
            return Some(dialog.id);
        }
        if let Some(id) = self.window_dialog_id() {
            return Some(id);
        }
        if let Some(dialog) = self.nt4.dialog() {
            return Some(dialog.id);
        }
        if let Some(dialog) = self.logon.dialog() {
            return Some(dialog.id);
        }
        if let Some(dialog) = self.dns.dialog() {
            return Some(dialog.id);
        }
        if let Some(dialog) = self.components.dialog() {
            return Some(dialog.id);
        }
        self.my_server.dialog(&self.services).map(|d| d.id)
    }

    fn run_dialog(&mut self, mut pass: Pass) -> Option<(String, Answer)> {
        let id = self.dialog_id()?;
        if self.dialog_id != Some(id) {
            self.dialog_id = Some(id);
            self.dialog_view = ViewState::default();
        }
        let state = std::mem::take(&mut self.dialog_view);
        let palette = self.palette;
        let escape = matches!(
            &pass,
            Pass::Act {
                input: Input::Press(usize::MAX),
                ..
            }
        );
        let (title, answer) = if let Some(mut modal) = self.modal.take() {
            let form = match &mut modal {
                Modal::Confirm(_, _, f)
                | Modal::About(f)
                | Modal::Password(_, f)
                | Modal::AddHost(f)
                | Modal::Secret(f)
                | Modal::Rebuild(f)
                | Modal::Failure(f) => f,
            };
            let dialog = form.view();
            let (answer, msg) = drive_dialog(&dialog, &mut pass, &state, palette, escape);
            let title = dialog.title.clone();
            let done = msg.and_then(|m| form.update(m));
            self.modal = Some(modal);
            if let Some(result) = done {
                self.answer_modal(result);
            }
            (title, answer)
        } else if let Some(overlay) = self.login.as_ref().and_then(LoginBox::overlay) {
            let (answer, msg) = drive_dialog(&overlay, &mut pass, &state, palette, escape);
            if let (Some(button), Some(login)) = (msg, self.login.as_mut()) {
                login.answer_overlay(button);
            }
            (overlay.title.clone(), answer)
        } else if let Some(login) = self.login.as_mut() {
            let known = self.hosts.hosts().to_vec();
            let nodes = login.view(&known);
            let title = login.title().to_string();
            let (answer, msg) = drive(&nodes, &mut pass, &state, palette);
            match msg.and_then(|msg| login.update(msg, &known)) {
                Some(login::Outcome::SignedIn(signed_in)) => self.finish_login(signed_in),
                Some(login::Outcome::ShutDown) => self.quit = true,
                None => {}
            }
            (title, answer)
        } else if let Some(warning) = self.admin_setup.as_ref().and_then(AdminSetup::warning) {
            let (answer, msg) = drive_dialog(&warning, &mut pass, &state, palette, escape);
            if let (Some(button), Some(setup)) = (msg, self.admin_setup.as_mut()) {
                setup.answer_warning(button);
            }
            (warning.title.clone(), answer)
        } else if self.admin_setup.is_some() {
            let target = self
                .signed_in
                .as_ref()
                .map(|s| s.target.clone())
                .unwrap_or_default();
            let setup = self.admin_setup.as_mut().expect("checked above");
            let nodes = setup.view();
            let title = setup.title().to_string();
            let (answer, msg) = drive(&nodes, &mut pass, &state, palette);
            if let Some(outcome) = msg.and_then(|msg| setup.update(msg, &self.services, &target)) {
                self.admin_outcome(outcome);
            }
            (title, answer)
        } else if let Some(dialog) = self.users.dialog() {
            let (answer, msg) = drive_dialog(&dialog, &mut pass, &state, palette, escape);
            let title = dialog.title.clone();
            if let Some(msg) = msg {
                let host = self.host();
                self.users
                    .update_dialog(msg, &self.services, host.as_deref());
            }
            (title, answer)
        } else if let Some(found) = self.window_dialog(&mut pass, &state, escape) {
            found
        } else if let Some(dialog) = self.nt4.dialog() {
            let (answer, msg) = drive_dialog(&dialog, &mut pass, &state, palette, escape);
            let title = dialog.title.clone();
            if let Some(msg) = msg {
                let host = self.host();
                self.nt4.update_dialog(msg, &self.services, host.as_deref());
            }
            (title, answer)
        } else if let Some(dialog) = self.logon.dialog() {
            let (answer, msg) = drive_dialog(&dialog, &mut pass, &state, palette, escape);
            let title = dialog.title.clone();
            if let Some(msg) = msg {
                self.logon.update_dialog(msg);
            }
            (title, answer)
        } else if let Some(dialog) = self.dns.dialog() {
            let (answer, msg) = drive_dialog(&dialog, &mut pass, &state, palette, escape);
            let title = dialog.title.clone();
            if let Some(msg) = msg {
                self.dns.update_dialog(msg);
            }
            (title, answer)
        } else if let Some(dialog) = self.components.dialog() {
            let (answer, msg) = drive_dialog(&dialog, &mut pass, &state, palette, escape);
            let title = dialog.title.clone();
            if let (Some(msg), Some(host)) = (msg, self.host()) {
                self.components.update(msg, &self.services, &host);
            }
            (title, answer)
        } else {
            let dialog = self.my_server.dialog(&self.services)?;
            let (answer, msg) = drive_dialog(&dialog, &mut pass, &state, palette, escape);
            let title = dialog.title.clone();
            if let Some(msg) = msg {
                let host = self
                    .my_server
                    .purge()
                    .map(|p| p.host().to_string())
                    .unwrap_or_default();
                self.my_server.update(msg, &self.services, &host);
            }
            (title, answer)
        };
        let mut state = state;
        if state.focus.is_none() {
            state.focus = answer.want_focus.clone();
        }
        self.dialog_view = state;
        Some((title, answer))
    }

    /// a desktop box was answered.
    fn answer_modal(&mut self, result: FormAnswer) {
        let Some(modal) = self.modal.take() else {
            return;
        };
        match (modal, result) {
            (Modal::Confirm(power, host, _), FormAnswer::Ok) => {
                self.services.power(&host, power);
            }
            (Modal::Password(request, form), answer) => {
                // the password goes back zeroized, and is wiped here once the task has it
                let password =
                    (answer == FormAnswer::Ok).then(|| Zeroizing::new(form.value(0).to_string()));
                let _ = request.reply.send(password);
            }
            (Modal::AddHost(mut form), FormAnswer::Ok) => {
                let text = form.value(0).to_string();
                if let Err(error) = self.hosts.add(&text) {
                    form.error = Some(error);
                    self.modal = Some(Modal::AddHost(form));
                }
            }
            (Modal::Rebuild(form), FormAnswer::Ok) => self.start_rebuild(form.checked(0)),
            (Modal::Secret(form), answer) => {
                let value =
                    (answer == FormAnswer::Ok).then(|| Zeroizing::new(form.value(0).to_string()));
                self.settings.answer_secret(value);
            }
            _ => {}
        }
    }
}

/// like `drive`, but Escape (sent as `Press(usize::MAX)`) means the box's escape message.
fn drive_dialog<M: Clone>(
    dialog: &Dialog<M>,
    pass: &mut Pass,
    state: &ViewState,
    palette: Palette,
    escape: bool,
) -> (Answer, Option<M>) {
    if escape {
        return (Answer::default(), dialog.escape.clone());
    }
    drive(&dialog.body, pass, state, palette)
}

// drawing
impl Shell {
    /// draw the whole desktop into `buf`.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer) {
        let palette = self.palette;
        self.devtools_begin();
        let desktop = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        self.hits = Hits {
            desktop,
            ..Hits::default()
        };
        if self.before_desktop() {
            // the logon box and the first-administrator screen sit on the bare desktop colour,
            // with no icons or taskbar until someone is signed in
            Teal { palette }.render(area, buf);
            self.hits.desktop = area;
            self.render_dialog(area, buf);
            self.render_corner_bug(area, buf);
            self.render_debug_menu(buf);
            self.render_dropdown(buf);
            return;
        }
        Teal { palette }.render(desktop, buf);
        for (i, target) in self.icons.clone().into_iter().enumerate() {
            let rect = icon_rect(desktop, i);
            if rect.is_empty() {
                continue;
            }
            GlyphTile {
                id: target.id(),
                label: target.title(),
                selected: self.selected == Some(i),
                palette,
            }
            .render(rect, buf);
            self.hits.icons.push(rect);
        }

        // windows, bottom to top
        self.refit_windows(desktop);
        let order: Vec<(Target, Rect)> = self
            .windows
            .iter()
            .filter(|w| !w.minimized)
            .map(|w| (w.target, w.rect.intersection(desktop)))
            .collect();
        let dialog_open = self.dialog_id().is_some();
        for (target, rect) in order {
            if rect.width < 10 || rect.height < 4 {
                continue;
            }
            let active = self.active == Some(target);
            WindowFrame {
                title: target.title(),
                icon: Some(target.id()),
                active,
                palette,
                fixed: target.fixed(),
            }
            .render(rect, buf);
            let layout = WindowFrame::layout_for(rect, target.fixed());
            let body = layout::content(rect);
            let answer = self.run_window(
                target,
                Pass::Draw {
                    area: body,
                    buf: &mut *buf,
                    active: active && !dialog_open && self.menu.is_none(),
                },
            );
            self.hits
                .windows
                .push((target, rect, layout, answer.regions));
        }

        self.outline_windows(buf);
        self.render_taskbar(area, buf);
        match self.menu.clone() {
            Some(action) => self.render_menu(&action, buf),
            None => self.submenu = None,
        }
        self.render_dialog(desktop, buf);
        self.render_debug_menu(buf);
        self.render_dropdown(buf);
    }

    /// does a switched-on component need something this server lacks? the same test as the
    /// gui's warning sign in the tray.
    pub fn compat_alert(&self) -> bool {
        let Some(host) = self.host() else {
            return false;
        };
        let (Some(report), Some(enabled)) = (self.services.compat().report(&host), self.enabled())
        else {
            return false;
        };
        !lanlord_app::compat::problems(&report, &enabled).is_empty()
    }

    /// open My Server on its Compatibility tab.
    fn open_compat(&mut self) {
        self.open(Target::MyServer);
        self.compat_next = true;
    }

    fn render_taskbar(&mut self, area: Rect, buf: &mut Buffer) {
        let clock = self.clock.clone().unwrap_or_else(|| self.feed.text());
        let shown: Vec<(Target, bool)> = self
            .windows
            .iter()
            .map(|w| (w.target, self.active == Some(w.target) && !w.minimized))
            .collect();
        let buttons: Vec<TaskButton> = shown
            .iter()
            .map(|(target, pressed)| TaskButton {
                icon: Some(target.id()),
                label: target.title(),
                pressed: *pressed,
            })
            .collect();
        let bar = Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1);
        let shield = self.update_label();
        let taskbar = Taskbar {
            menu_label: "LANlord",
            menu_icon: Some("lanlord"),
            menu_open: self.menu.is_some(),
            buttons: &buttons,
            clock: &clock,
            alert: self.compat_alert().then_some(COMPAT_ALERT),
            shield: shield.as_deref(),
            bug: self.bug_shown(),
            palette: self.palette,
        };
        self.hits.taskbar = Some(taskbar.layout(bar));
        self.hits.task_targets = shown.iter().map(|(t, _)| *t).collect();
        taskbar.render(bar, buf);
    }

    fn render_menu(&mut self, action: &Action, buf: &mut Buffer) {
        let rows = self.menu_state();
        let hover = menu_row_of(&rows, action);
        let items = maintenance::menu_rows(&rows);
        let menu = Menu {
            rows: &items,
            selected: Some(hover),
            palette: self.palette,
        };
        let (w, h) = menu.size();
        let desktop = self.hits.desktop;
        let rect = Rect::new(
            desktop.x,
            desktop.bottom().saturating_sub(h),
            w.min(desktop.width),
            h.min(desktop.height),
        );
        let row_rects: Vec<(Rect, usize)> = menu.row_rects(rect).into_iter().zip(0..).collect();
        menu.render(rect, buf);
        self.render_submenu(&rows, &row_rects, rect, buf);
        self.hits.menu = Some((rect, row_rects));
    }

    fn render_dialog(&mut self, desktop: Rect, buf: &mut Buffer) {
        let wide = if self.admin_setup.is_some() && self.login.is_none() {
            layout::DIALOG_WIDE
        } else {
            layout::DIALOG_WIDTH
        };
        let most = wide.min(desktop.width.saturating_sub(4)).max(20);
        // the box is as wide as its content wants, between the narrowest and the widest
        let Some((title, wanted)) = self.run_dialog(Pass::Measure { width: most }) else {
            return;
        };
        let asked = layout::frame_width(wanted.width)
            .max(layout::DIALOG_MIN)
            .max(title.chars().count() as u16 + layout::TITLE_ROOM);
        let width = asked.min(most);
        let Some((_, measured)) = self.run_dialog(Pass::Measure {
            width: layout::content_width(width),
        }) else {
            return;
        };
        let height = layout::frame_height(measured.height).min(desktop.height);
        let rect = Rect::new(
            desktop.x + desktop.width.saturating_sub(width) / 2,
            desktop.y + desktop.height.saturating_sub(height) / 2,
            width,
            height,
        );
        WindowFrame {
            title: &title,
            icon: None,
            active: true,
            palette: self.palette,
            fixed: false,
        }
        .render(rect, buf);
        let body = layout::content(rect);
        if let Some((_, answer)) = self.run_dialog(Pass::Draw {
            area: body,
            buf,
            active: true,
        }) {
            // a box with no field asking for the cursor starts on its first field or button
            if self.dialog_view.focus.is_none() {
                let first = answer
                    .regions
                    .iter()
                    .find(|r| r.part == Part::Field)
                    .or_else(|| answer.regions.iter().find(|r| r.focusable));
                self.dialog_view.focus = first.map(|r| r.key.clone());
            }
            self.hits.dialog = Some((rect, answer.regions));
        }
    }

    fn render_dropdown(&mut self, buf: &mut Buffer) {
        let owner_state = self
            .active_owner()
            .map(|o| (o, self.view_of(o).dropdown.clone()));
        let Some((owner, Some(dropdown))) = owner_state else {
            return;
        };
        let height = dropdown.options.len() as u16;
        let rect = Rect::new(
            dropdown.under.x,
            dropdown.under.bottom(),
            dropdown.under.width.max(12),
            height,
        )
        .intersection(buf.area);
        fill(buf, rect, self.palette, WHITE);
        let mut rows = Vec::new();
        for (n, (key, label)) in dropdown.options.iter().enumerate() {
            let line = Rect::new(rect.x, rect.y + n as u16, rect.width, 1).intersection(buf.area);
            if line.is_empty() {
                continue;
            }
            let (fg, bg) = if *label == dropdown.current {
                (WHITE, NAVY)
            } else {
                (BLACK, WHITE)
            };
            fill(buf, line, self.palette, bg);
            text(
                buf,
                line.x,
                line.y,
                line.width,
                label,
                style(self.palette, fg, bg),
            );
            rows.push((line, *key));
        }
        self.hits.dropdown = Some((owner, dropdown.path.clone(), rows));
    }
}

// input
impl Shell {
    /// the box or window the keyboard goes to.
    fn active_owner(&self) -> Option<Owner> {
        if self.hits.dialog.is_some() {
            return Some(Owner::Dialog);
        }
        self.active
            .filter(|t| self.windows.iter().any(|w| w.target == *t && !w.minimized))
            .map(Owner::Window)
    }

    fn view_of(&self, owner: Owner) -> &ViewState {
        static EMPTY: std::sync::OnceLock<ViewState> = std::sync::OnceLock::new();
        match owner {
            Owner::Dialog => &self.dialog_view,
            Owner::Window(t) => self
                .views
                .get(t.id())
                .unwrap_or_else(|| EMPTY.get_or_init(ViewState::default)),
        }
    }

    fn view_mut(&mut self, owner: Owner) -> &mut ViewState {
        match owner {
            Owner::Dialog => &mut self.dialog_view,
            Owner::Window(t) => self.views.entry(t.id()).or_default(),
        }
    }

    fn regions_of(&self, owner: Owner) -> Vec<Region> {
        match owner {
            Owner::Dialog => self
                .hits
                .dialog
                .as_ref()
                .map(|d| d.1.clone())
                .unwrap_or_default(),
            Owner::Window(t) => self
                .hits
                .windows
                .iter()
                .find(|w| w.0 == t)
                .map(|w| w.3.clone())
                .unwrap_or_default(),
        }
    }

    fn pass(&mut self, owner: Owner, pass: Pass) -> Answer {
        match owner {
            Owner::Dialog => self.run_dialog(pass).map(|(_, a)| a).unwrap_or_default(),
            Owner::Window(t) => self.run_window(t, pass),
        }
    }

    fn send(&mut self, owner: Owner, path: Vec<usize>, input: Input) {
        self.pass(owner, Pass::Act { path, input });
    }

    /// handle one terminal event.
    pub fn on_event(&mut self, event: Event, now: std::time::Instant) {
        self.refit_due = true;
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key),
            Event::Mouse(mouse) => {
                for gesture in self.mouse.feed(mouse, now) {
                    self.on_gesture(gesture);
                }
            }
            _ => {}
        }
    }

    fn on_gesture(&mut self, gesture: Gesture) {
        match gesture {
            Gesture::Press { at } => self.press(at),
            Gesture::Click { at } => self.click(at),
            Gesture::DoubleClick { at } => self.double_click(at),
            Gesture::Drag { to, .. } => {
                if let Some((target, dx, dy)) = self.drag {
                    let desktop = self.hits.desktop;
                    if let Some(win) = self.windows.iter_mut().find(|w| w.target == target) {
                        let x =
                            to.x.saturating_sub(dx)
                                .min(desktop.right().saturating_sub(4));
                        let y =
                            to.y.saturating_sub(dy)
                                .clamp(desktop.y, desktop.bottom().saturating_sub(1));
                        win.rect.x = x;
                        win.rect.y = y;
                        win.restore = None;
                    }
                }
            }
            Gesture::DragEnd { .. } => self.drag = None,
            Gesture::Scroll { at, lines } => self.scroll(at, lines),
            Gesture::RightClick { .. } => {}
        }
    }

    /// the left button went down: raise windows and start dragging title bars.
    fn press(&mut self, at: Position) {
        if self.hits.dialog.is_some() || self.menu.is_some() {
            return;
        }
        let hit = self
            .hits
            .windows
            .iter()
            .rev()
            .find(|w| w.1.contains(at))
            .map(|w| (w.0, w.1, w.2));
        if let Some((target, rect, layout)) = hit {
            self.raise(target);
            let on_button = [layout.minimise, layout.maximise, layout.close]
                .iter()
                .any(|r| r.contains(at));
            if layout.title_bar.contains(at) && !on_button {
                self.drag = Some((target, at.x - rect.x, at.y - rect.y));
            }
        }
    }

    /// a double-click comes after its own click, so it only does what a double-click adds:
    /// open an icon, maximise from the title bar, open the clock.
    fn double_click(&mut self, at: Position) {
        if self.hits.dialog.is_some() || self.menu.is_some() {
            return;
        }
        if self
            .hits
            .taskbar
            .as_ref()
            .is_some_and(|t| t.clock.contains(at))
        {
            self.open(Target::DateTime);
            return;
        }
        let title = self
            .hits
            .windows
            .iter()
            .rev()
            .find(|w| w.1.contains(at))
            .map(|w| (w.0, w.2.title_bar.contains(at)));
        match title {
            Some((target, true)) => self.toggle_maximise(target),
            Some(_) => {}
            None => {
                if let Some(i) = self.hits.icons.iter().position(|r| r.contains(at)) {
                    if let Some(target) = self.icons.get(i).copied() {
                        self.open(target);
                    }
                }
            }
        }
    }

    fn click(&mut self, at: Position) {
        if self.click_extras(at) {
            return;
        }
        // an open drop-down takes the click, or closes
        if let Some((owner, path, rows)) = self.hits.dropdown.take() {
            self.view_mut(owner).dropdown = None;
            if let Some((_, key)) = rows.iter().find(|(r, _)| r.contains(at)) {
                self.send(owner, path, Input::Pick(*key));
            }
            return;
        }
        // a box on top takes every click
        if let Some((rect, regions)) = self.hits.dialog.clone() {
            if rect.contains(at) {
                self.click_region(Owner::Dialog, &regions, at);
            }
            return;
        }
        if let Some((rect, rows)) = self.hits.menu.clone() {
            if rect.contains(at) {
                if let Some((_, n)) = rows.iter().find(|(r, _)| r.contains(at)) {
                    self.pick_menu(*n);
                }
                return;
            }
            self.menu = None;
            if self
                .hits
                .taskbar
                .as_ref()
                .is_some_and(|t| t.menu.contains(at))
            {
                return;
            }
        }
        if let Some(bar) = self.hits.taskbar.clone() {
            if bar.menu.contains(at) {
                self.open_menu();
                return;
            }
            if bar.alert.is_some_and(|a| a.contains(at)) {
                self.open_compat();
                return;
            }
            if bar.clock.contains(at) {
                return;
            }
            if let Some(n) = bar.buttons.iter().position(|r| r.contains(at)) {
                if let Some(target) = self.hits.task_targets.get(n).copied() {
                    let showing = self
                        .windows
                        .iter()
                        .any(|w| w.target == target && !w.minimized);
                    if showing && self.active == Some(target) {
                        self.minimize(target);
                    } else {
                        self.raise(target);
                    }
                }
                return;
            }
            if at.y >= bar.menu.y {
                return;
            }
        }
        let hit = self
            .hits
            .windows
            .iter()
            .rev()
            .find(|w| w.1.contains(at))
            .map(|w| (w.0, w.2, w.3.clone()));
        if let Some((target, layout, regions)) = hit {
            self.active = Some(target);
            if layout.close.contains(at) {
                self.close(target);
            } else if layout.minimise.contains(at) {
                self.minimize(target);
            } else if layout.maximise.contains(at) {
                self.toggle_maximise(target);
            } else {
                self.click_region(Owner::Window(target), &regions, at);
            }
            return;
        }
        // the desktop: icons select, double-click opens, blank space clears
        match self.hits.icons.iter().position(|r| r.contains(at)) {
            Some(i) => {
                self.selected = Some(i);
                self.active = None;
            }
            None => {
                self.selected = None;
                self.active = None;
            }
        }
    }

    fn click_region(&mut self, owner: Owner, regions: &[Region], at: Position) {
        // the smallest region under the click wins (a row before its list)
        let Some(region) = regions
            .iter()
            .filter(|r| r.rect.contains(at))
            .min_by_key(|r| u32::from(r.rect.width) * u32::from(r.rect.height))
            .cloned()
        else {
            return;
        };
        let focus_key = match region.part {
            Part::Row(_) | Part::Mark(_) => Some(focus_key(&region.path, "List")),
            Part::Tab(_) => Some(focus_key(&region.path, "TabBar")),
            _ if region.focusable => Some(region.key.clone()),
            _ => None,
        };
        if let Some(key) = focus_key {
            self.view_mut(owner).focus = Some(key);
        }
        let input = match region.part {
            Part::Press(n) => Input::Press(n),
            Part::Toggle => Input::Toggle,
            Part::Row(n) => Input::Row(n),
            Part::Mark(n) => Input::Mark(n),
            Part::Tab(n) => Input::Tab(n),
            Part::Open => {
                self.open_dropdown(owner, &region);
                return;
            }
            Part::Fold => {
                self.toggle_fold(owner, &region.key);
                return;
            }
            Part::Spin(dir) if dir != 0 => Input::Step(dir),
            Part::Day(n) => Input::Day(n),
            _ => return,
        };
        self.send(owner, region.path, input);
    }

    /// Enter in a box's field with nothing of its own presses the box's default button: the
    /// last one of its bottom row, which the gui draws leftmost (OK before Cancel).
    fn press_default(&mut self, owner: Owner, regions: &[Region]) {
        if owner != Owner::Dialog {
            return;
        }
        let default = regions
            .iter()
            .filter_map(|r| match r.part {
                Part::Press(n) => Some((r.rect.y, n, r.path.clone())),
                _ => None,
            })
            .max_by_key(|(y, n, _)| (*y, *n));
        if let Some((_, n, path)) = default {
            self.send(owner, path, Input::Press(n));
        }
    }

    /// move a calendar's selected day by `days`; the calendar's own message does the rest
    /// (a day in the next month moves the month with it).
    fn day_step(&mut self, owner: Owner, region: &Region, days: i32) {
        let answer = self.pass(
            owner,
            Pass::Query {
                path: region.path.clone(),
            },
        );
        let Some((year, month, selected, enabled)) = answer.calendar else {
            return;
        };
        if !enabled {
            return;
        }
        let target = selected + chrono::TimeDelta::days(i64::from(days));
        let grid = lanlord_app::clock_window::month_days(year, month);
        // a day off the shown grid only exists in another month, which the window's spin boxes
        // change; the selection stays on the last day shown
        if let Some(n) = grid.iter().position(|d| *d == target) {
            self.send(owner, region.path.clone(), Input::Day(n));
        }
    }

    /// open or close a fold, which only the front end remembers.
    fn toggle_fold(&mut self, owner: Owner, key: &str) {
        let open = &mut self.view_mut(owner).open;
        if !open.remove(key) {
            open.insert(key.to_string());
        }
    }

    fn open_dropdown(&mut self, owner: Owner, region: &Region) {
        let answer = self.pass(
            owner,
            Pass::Query {
                path: region.path.clone(),
            },
        );
        if let Some((options, current)) = answer.choice {
            self.view_mut(owner).dropdown = Some(view::Dropdown {
                path: region.path.clone(),
                under: region.rect,
                options,
                current,
            });
        }
    }

    fn scroll(&mut self, at: Position, lines: i8) {
        // a menu or a dialog is modal: the wheel does nothing behind it
        if self.menu.is_some() || self.dialog_id().is_some() {
            return;
        }
        let target = self
            .hits
            .windows
            .iter()
            .rev()
            .find(|w| w.1.contains(at))
            .map(|w| (w.0, w.3.clone()));
        let Some((target, regions)) = target else {
            return;
        };
        let Some(region) = regions
            .iter()
            .filter(|r| {
                r.rect.contains(at)
                    && matches!(
                        r.part,
                        Part::Scroll | Part::List | Part::Spin(0) | Part::Days
                    )
            })
            .min_by_key(|r| u32::from(r.rect.width) * u32::from(r.rect.height))
            .cloned()
        else {
            return;
        };
        let owner = Owner::Window(target);
        if region.part == Part::List {
            // the wheel moves a list's selection, like the arrow keys
            self.list_step(owner, &region, i32::from(lines));
            return;
        }
        if region.part == Part::Spin(0) {
            // the wheel steps a spin box: up (negative lines) counts up
            let dir = if lines < 0 { 1 } else { -1 };
            self.send(owner, region.path, Input::Step(dir));
            return;
        }
        if region.part == Part::Days {
            self.day_step(owner, &region, 7 * i32::from(lines));
            return;
        }
        self.view_mut(owner)
            .scroll_by(&region.key, i32::from(lines) * 3);
    }

    /// Space on a check list ticks or clears the selected row.
    fn list_mark(&mut self, owner: Owner, region: &Region) {
        let answer = self.pass(
            owner,
            Pass::Query {
                path: region.path.clone(),
            },
        );
        if let Some((Some(selected), _)) = answer.list {
            self.send(owner, region.path.clone(), Input::Mark(selected));
        }
    }

    fn list_step(&mut self, owner: Owner, region: &Region, step: i32) {
        let answer = self.pass(
            owner,
            Pass::Query {
                path: region.path.clone(),
            },
        );
        if let Some((selected, len)) = answer.list {
            if len == 0 {
                return;
            }
            let next = match selected {
                Some(s) => (s as i32 + step).clamp(0, len as i32 - 1) as usize,
                None => 0,
            };
            self.send(owner, region.path.clone(), Input::Row(next));
        }
    }

    /// open the menu with the keyboard on its first row.
    fn open_menu(&mut self) {
        self.menu = menu_actions(&self.menu_state()).next().cloned();
    }

    /// move the highlight one item up or down, wrapping and skipping the separators.
    fn step_menu(&mut self, forward: bool) {
        let rows = self.menu_state();
        let actions: Vec<&Action> = menu_actions(&rows).collect();
        let Some(current) = self.menu.as_ref() else {
            return;
        };
        let at = actions.iter().position(|a| *a == current).unwrap_or(0);
        let next = if forward {
            (at + 1) % actions.len()
        } else {
            at.checked_sub(1).unwrap_or(actions.len() - 1)
        };
        self.menu = Some(actions[next].clone());
    }

    fn pick_menu(&mut self, n: usize) {
        let rows = self.menu_state();
        if let Some(Row::Item(item)) = rows.get(n) {
            if item.enabled && !item.header {
                self.do_action(item.action.clone());
            }
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('q') {
            self.quit = true;
            return;
        }
        if self.key_extras(key) {
            return;
        }
        if key.code == KeyCode::F(8) && !self.before_desktop() && self.compat_alert() {
            self.open_compat();
            return;
        }
        if key.code == KeyCode::F(10) && !self.before_desktop() {
            if self.menu.is_some() {
                self.menu = None;
            } else {
                self.open_menu();
            }
            return;
        }
        if let Some(action) = self.menu.clone() {
            match key.code {
                KeyCode::Esc => self.menu = None,
                KeyCode::Up => self.step_menu(false),
                KeyCode::Down => self.step_menu(true),
                KeyCode::Enter => {
                    let row = menu_row_of(&self.menu_state(), &action);
                    self.pick_menu(row);
                }
                _ => {}
            }
            return;
        }
        match self.active_owner() {
            Some(owner) => self.key_in(owner, key),
            None => self.key_on_desktop(key),
        }
    }

    fn key_on_desktop(&mut self, key: KeyEvent) {
        let count = self.icons.len();
        let at = self.selected.unwrap_or(0);
        match key.code {
            KeyCode::Down | KeyCode::Right => self.selected = Some((at + 1) % count),
            KeyCode::Up | KeyCode::Left => {
                self.selected = Some(at.checked_sub(1).unwrap_or(count - 1))
            }
            KeyCode::Enter => {
                if let Some(target) = self.selected.and_then(|i| self.icons.get(i)).copied() {
                    self.open(target);
                }
            }
            KeyCode::Tab | KeyCode::F(6) => {
                if let Some(win) = self.windows.iter().rev().find(|w| !w.minimized) {
                    self.active = Some(win.target);
                }
            }
            _ => {}
        }
    }

    fn key_in(&mut self, owner: Owner, key: KeyEvent) {
        let regions = self.regions_of(owner);
        let focusable: Vec<Region> = regions.iter().filter(|r| r.focusable).cloned().collect();
        let focus = self.view_of(owner).focus.clone();
        let current = focus
            .as_deref()
            .and_then(|k| focusable.iter().position(|r| r.key == k));
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Tab | KeyCode::BackTab if !focusable.is_empty() => {
                let back =
                    key.code == KeyCode::BackTab || key.modifiers.contains(KeyModifiers::SHIFT);
                let n = focusable.len();
                let next = match (current, back) {
                    (Some(i), false) => (i + 1) % n,
                    (Some(i), true) => (i + n - 1) % n,
                    (None, false) => 0,
                    (None, true) => n - 1,
                };
                self.view_mut(owner).focus = Some(focusable[next].key.clone());
                return;
            }
            KeyCode::Esc => {
                match owner {
                    Owner::Dialog => self.send(owner, Vec::new(), Input::Press(usize::MAX)),
                    Owner::Window(target) => {
                        if self.view_of(owner).dropdown.is_some() {
                            self.view_mut(owner).dropdown = None;
                        } else {
                            let _ = target;
                            self.active = None;
                        }
                    }
                }
                return;
            }
            KeyCode::Char('w') if ctrl => {
                if let Owner::Window(target) = owner {
                    self.close(target);
                }
                return;
            }
            KeyCode::F(6) => {
                // the next window gets the keyboard
                let visible: Vec<Target> = self
                    .windows
                    .iter()
                    .filter(|w| !w.minimized)
                    .map(|w| w.target)
                    .collect();
                if let (Owner::Window(target), false) = (owner, visible.is_empty()) {
                    let i = visible.iter().position(|t| *t == target).unwrap_or(0);
                    let next = visible[(i + visible.len() - 1) % visible.len()];
                    self.raise(next);
                }
                return;
            }
            _ => {}
        }
        let Some(region) = current.map(|i| focusable[i].clone()) else {
            return;
        };
        match (&region.part, key.code) {
            (Part::Press(n), KeyCode::Enter | KeyCode::Char(' ')) => {
                self.send(owner, region.path, Input::Press(*n));
            }
            (Part::Toggle, KeyCode::Enter | KeyCode::Char(' ')) => {
                self.send(owner, region.path, Input::Toggle);
            }
            (Part::Fold, KeyCode::Enter | KeyCode::Char(' ')) => {
                self.toggle_fold(owner, &region.key);
            }
            (Part::List, KeyCode::Char(' ')) => self.list_mark(owner, &region),
            (Part::List, KeyCode::Up) => self.list_step(owner, &region, -1),
            (Part::List, KeyCode::Down) => self.list_step(owner, &region, 1),
            (Part::Spin(0), KeyCode::Up | KeyCode::Right) => {
                self.send(owner, region.path, Input::Step(1));
            }
            (Part::Spin(0), KeyCode::Down | KeyCode::Left) => {
                self.send(owner, region.path, Input::Step(-1));
            }
            (Part::Days, KeyCode::Left) => self.day_step(owner, &region, -1),
            (Part::Days, KeyCode::Right) => self.day_step(owner, &region, 1),
            (Part::Days, KeyCode::Up) => self.day_step(owner, &region, -7),
            (Part::Days, KeyCode::Down) => self.day_step(owner, &region, 7),
            (Part::TabBar, KeyCode::Left | KeyCode::Right) => {
                let answer = self.pass(
                    owner,
                    Pass::Query {
                        path: region.path.clone(),
                    },
                );
                if let Some((current, count)) = answer.tab {
                    let next = if key.code == KeyCode::Left {
                        current.checked_sub(1).unwrap_or(count - 1)
                    } else {
                        (current + 1) % count
                    };
                    self.send(owner, region.path, Input::Tab(next));
                }
            }
            (Part::Open, KeyCode::Enter | KeyCode::Char(' ')) => self.open_dropdown(owner, &region),
            (Part::Open, KeyCode::Up | KeyCode::Down) => {
                let answer = self.pass(
                    owner,
                    Pass::Query {
                        path: region.path.clone(),
                    },
                );
                if let Some((options, current)) = answer.choice {
                    let at = options.iter().position(|(_, l)| *l == current).unwrap_or(0);
                    let next = if key.code == KeyCode::Up {
                        at.saturating_sub(1)
                    } else {
                        (at + 1).min(options.len().saturating_sub(1))
                    };
                    if let Some((key, _)) = options.get(next) {
                        self.send(owner, region.path, Input::Pick(*key));
                    }
                }
            }
            (Part::Field, code) => {
                let answer = self.pass(
                    owner,
                    Pass::Query {
                        path: region.path.clone(),
                    },
                );
                let enters = answer.enters;
                let input = match answer.field {
                    Some(view::FieldValue::Secret(mut value)) => match code {
                        KeyCode::Char(c) if !ctrl => {
                            value.push(c);
                            Input::SecretText(value)
                        }
                        KeyCode::Backspace => {
                            value.pop();
                            Input::SecretText(value)
                        }
                        KeyCode::Enter if enters => Input::SecretEnter(value),
                        KeyCode::Enter => return self.press_default(owner, &regions),
                        _ => return,
                    },
                    Some(view::FieldValue::Plain { mut text, multi }) => match code {
                        KeyCode::Char(c) if !ctrl => {
                            text.push(c);
                            Input::Text(text)
                        }
                        KeyCode::Backspace => {
                            text.pop();
                            Input::Text(text)
                        }
                        KeyCode::Enter if multi => {
                            text.push('\n');
                            Input::Text(text)
                        }
                        // Enter carries the final text, like the gui
                        KeyCode::Enter if enters => Input::Enter(text),
                        KeyCode::Enter => return self.press_default(owner, &regions),
                        _ => return,
                    },
                    None => return,
                };
                self.send(owner, region.path, input);
            }
            _ => {}
        }
    }
}

/// the focus key of the whole list or tab row at `path`.
fn focus_key(path: &[usize], part: &str) -> String {
    let mut key = path
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(".");
    key.push('#');
    key.push_str(part);
    key
}

#[cfg(feature = "devtools")]
mod devtools;
#[cfg(not(feature = "devtools"))]
#[path = "shell/devtools_off.rs"]
mod devtools;
mod maintenance;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod screens;

#[cfg(test)]
mod menu_tests;

#[cfg(test)]
mod window_tests;
