//! the terminal desktop: the same desktop, menu, windows and boxes the gui shows, drawn in
//! character cells. the windows come from `lanlord_app` as layout trees, so this file only
//! places them, routes the mouse and keyboard to them and hands their messages back.

use std::collections::HashMap;
use std::sync::Arc;

use lanlord_app::console::Console;
use lanlord_app::desktop::{self, Target};
use lanlord_app::form::{Form, FormAnswer};
use lanlord_app::gateway::Gateway;
use lanlord_app::hostlist::HostList;
use lanlord_app::menu::{self, Action, Row};
use lanlord_app::my_server::MyServer;
use lanlord_app::services::Services;
use lanlord_app::settings_window::SettingsWindow;
use lanlord_app::tasks::PasswordRequest;
use lanlord_app::users_panel::UsersPanel;
use lanlord_app::view::{Dialog, Node};
use lanlord_app::wake::Wake;
use lanlord_app::{hosts_window, module_window};
use lanlord_core::modules::MODULES;
use lanlord_core::power::Power;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::Widget;

use crate::paint::chrome::{
    Menu, MenuRow, TaskButton, Taskbar, TaskbarLayout, WindowFrame, WindowLayout,
};
use crate::paint::desktop::{icon_rect, Desktop as Teal, GlyphTile};
use crate::paint::mouse::{Gesture, Mouse};
use crate::paint::palette::{Palette, BLACK, GRAY, NAVY, WHITE};
use crate::paint::{fill, style, text};
use crate::view::{self, Input, Painter, Part, Region, ViewState};

/// what a window body says when its window has not moved to the layout tree yet.
pub const NOT_YET: &str = "This window is not available in the text view yet. Open it in the LANlord window or the web UI.";

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
    field: Option<(String, bool)>,
    list: Option<(Option<usize>, usize)>,
    tab: Option<(usize, usize)>,
    choice: Option<(Vec<(usize, String)>, String)>,
    /// the field at the queried path sends its own message on Enter.
    enters: bool,
    height: u16,
}

/// run `pass` over `nodes`; the message it produced, if any.
fn drive<M: Clone>(
    nodes: &[Node<M>],
    pass: &mut Pass,
    state: &ViewState,
    palette: Palette,
) -> (Answer, Option<M>) {
    let mut answer = Answer::default();
    match pass {
        Pass::Draw { area, buf, active } => {
            let area = *area;
            let mut painter = Painter::new(palette, state, *active);
            let total = painter.height(nodes, area.width, Some(area.height));
            if total <= area.height {
                painter.draw(nodes, area, buf, &[]);
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
                painter.draw(nodes, whole, &mut tall, &[]);
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
                    path: Vec::new(),
                    part: Part::Scroll,
                    focusable: false,
                    key: "#body".into(),
                });
            }
            answer.want_focus = view::wants_focus(nodes, &painter.regions);
            answer.regions = painter.regions;
            (answer, None)
        }
        Pass::Act { path, input } => {
            let msg = view::act(nodes, path, input.clone());
            (answer, msg)
        }
        Pass::Measure { width } => {
            answer.height = Painter::new(palette, state, false).height(nodes, *width, None);
            (answer, None)
        }
        Pass::Query { path } => {
            answer.field = view::field_value(nodes, path);
            answer.list = view::list_state(nodes, path);
            answer.tab = view::tab_state(nodes, path);
            answer.choice = view::choice_state(nodes, path);
            answer.enters = view::field_enters(nodes, path);
            (answer, None)
        }
    }
}

/// a placeholder body for a window that is still drawn by egui alone.
fn not_yet<M>() -> Vec<Node<M>> {
    vec![Node::Text(NOT_YET.into())]
}

pub struct Shell {
    palette: Palette,
    services: Services,
    hosts: HostList,
    console: Console,
    my_server: MyServer,
    settings: SettingsWindow,
    users: UsersPanel,
    icons: Vec<Target>,
    selected: Option<usize>,
    /// bottom to top; the last visible one has the keyboard.
    windows: Vec<Win>,
    active: Option<Target>,
    /// the LANlord menu is open, with the keyboard on this row.
    menu: Option<usize>,
    modal: Option<Modal>,
    views: HashMap<&'static str, ViewState>,
    dialog_view: ViewState,
    /// the box the dialog view belongs to, so a new box starts fresh.
    dialog_id: Option<&'static str>,
    /// a window being dragged by its title bar, and where in the bar it was grabbed.
    drag: Option<(Target, u16, u16)>,
    mouse: Mouse,
    hits: Hits,
    /// the clock text; the local time unless a test sets it.
    pub clock: Option<String>,
    pub quit: bool,
}

impl Shell {
    /// a desktop that talks to gateways through `gateway` and keeps its hosts in `hosts`.
    pub fn new(gateway: Arc<dyn Gateway>, hosts: HostList, palette: Palette) -> Self {
        Self {
            palette,
            services: Services::new(gateway, Wake::none()),
            hosts,
            console: Console::new(Wake::none()),
            my_server: MyServer::default(),
            settings: SettingsWindow::default(),
            users: UsersPanel::default(),
            icons: desktop::icons(),
            selected: None,
            windows: Vec::new(),
            active: None,
            menu: None,
            modal: None,
            views: HashMap::new(),
            dialog_view: ViewState::default(),
            dialog_id: None,
            drag: None,
            mouse: Mouse::new(),
            hits: Hits::default(),
            clock: None,
            quit: false,
        }
    }

    /// act as if signed in to a made-up server (`LANLORD_FAKE_SERVER=1`), like the gui.
    pub fn use_demo_server(&mut self) {
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
        let host = self.host();
        desktop::window_title(self.services.is_local(), host.as_deref())
    }

    pub fn open(&mut self, target: Target) {
        if let Some(i) = self.windows.iter().position(|w| w.target == target) {
            let mut win = self.windows.remove(i);
            win.minimized = false;
            self.windows.push(win);
        } else {
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

    /// where a new window goes: right of the icons, cascading, cut down to fit.
    fn place(&self, target: Target) -> Rect {
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
        let (w, h) = target.size();
        let n = self.windows.len() as u16;
        let x = (icons_right + 1 + n * 2).min(area.right().saturating_sub(20));
        let y = area.y + (n % 6);
        let width = ((w / 7.0) as u16)
            .min(area.right().saturating_sub(x))
            .max(20);
        let height = ((h / 16.0) as u16 + 3)
            .min(area.bottom().saturating_sub(y))
            .max(8);
        Rect::new(x, y, width, height)
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

    fn menu_state(&self) -> Vec<Row> {
        let host = self.host();
        menu::rows(&menu::State {
            host: host.as_deref(),
            signed_in: None,
            ready: host.as_deref().is_some_and(|h| !self.services.busy(h)),
            updates: false,
            developer: false,
        })
    }

    fn do_action(&mut self, action: Action) {
        self.menu = None;
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
            // the logon box, updates and the developer menu live in the gui for now
            _ => {}
        }
    }
}

// background work, once per frame
impl Shell {
    pub fn poll(&mut self) {
        self.console.drain();
        self.settings.poll();
        self.users.poll();
        self.my_server.refresh_purge();
        if let Some(host) = self.my_server.take_purged() {
            let _ = host;
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
                    "nt4-domain" => {
                        let mut nodes = module_window::header(module);
                        nodes.extend(self.users.view(&self.services, host.as_deref()));
                        nodes.push(Node::Text(
                            "The domain settings and logon scripts are not in the text view yet."
                                .into(),
                        ));
                        let (answer, msg) = drive(&nodes, &mut pass, &state, palette);
                        if let (Some(msg), Some(host)) = (msg, host.as_deref()) {
                            self.users.update(msg, &self.services, host);
                        }
                        answer
                    }
                    "dns" | "remote-commands" => {
                        let mut nodes = module_window::header::<()>(module);
                        nodes.extend(not_yet());
                        drive(&nodes, &mut pass, &state, palette).0
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
            Target::Networks | Target::DateTime => {
                drive::<()>(&not_yet(), &mut pass, &state, palette).0
            }
        };
        if state.focus.is_none() {
            state.focus = answer.want_focus.clone();
        }
        self.views.insert(key, state);
        answer
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
            });
        }
        if let Some(dialog) = self.users.dialog() {
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
                | Modal::Secret(f) => f,
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
        } else if let Some(dialog) = self.users.dialog() {
            let (answer, msg) = drive_dialog(&dialog, &mut pass, &state, palette, escape);
            let title = dialog.title.clone();
            if let Some(msg) = msg {
                let host = self.host();
                self.users
                    .update_dialog(msg, &self.services, host.as_deref());
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
                let password = (answer == FormAnswer::Ok).then(|| form.value(0).to_string());
                let _ = request.reply.send(password);
            }
            (Modal::AddHost(mut form), FormAnswer::Ok) => {
                let text = form.value(0).to_string();
                if let Err(error) = self.hosts.add(&text) {
                    form.error = Some(error);
                    self.modal = Some(Modal::AddHost(form));
                }
            }
            (Modal::Secret(form), answer) => {
                let value = (answer == FormAnswer::Ok).then(|| form.value(0).to_string());
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
        let desktop = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        self.hits = Hits {
            desktop,
            ..Hits::default()
        };
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
            }
            .render(rect, buf);
            let layout = WindowFrame::layout(rect);
            let body = Rect::new(
                layout.body.x + 1,
                layout.body.y,
                layout.body.width.saturating_sub(2),
                layout.body.height,
            );
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

        self.render_taskbar(area, buf);
        if let Some(hover) = self.menu {
            self.render_menu(hover, buf);
        }
        self.render_dialog(desktop, buf);
        self.render_dropdown(buf);
    }

    fn render_taskbar(&mut self, area: Rect, buf: &mut Buffer) {
        let clock = self
            .clock
            .clone()
            .unwrap_or_else(|| chrono::Local::now().format("%-I:%M %p").to_string());
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
        let taskbar = Taskbar {
            menu_label: "LANlord",
            menu_icon: Some("lanlord"),
            menu_open: self.menu.is_some(),
            buttons: &buttons,
            clock: &clock,
            palette: self.palette,
        };
        self.hits.taskbar = Some(taskbar.layout(bar));
        self.hits.task_targets = shown.iter().map(|(t, _)| *t).collect();
        taskbar.render(bar, buf);
    }

    fn render_menu(&mut self, hover: usize, buf: &mut Buffer) {
        let rows = self.menu_state();
        let items: Vec<MenuRow> = rows
            .iter()
            .map(|row| match row {
                Row::Item(item) => MenuRow::Item {
                    label: &item.label,
                    ticked: item.ticked,
                    arrow: item.arrow,
                    enabled: item.enabled,
                    header: item.header,
                },
                Row::Separator => MenuRow::Separator,
            })
            .collect();
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
        let row_rects = menu.row_rects(rect).into_iter().zip(0..).collect();
        menu.render(rect, buf);
        self.hits.menu = Some((rect, row_rects));
    }

    fn render_dialog(&mut self, desktop: Rect, buf: &mut Buffer) {
        let width = 60.min(desktop.width.saturating_sub(4)).max(20);
        let Some((title, measured)) = self.run_dialog(Pass::Measure { width: width - 4 }) else {
            return;
        };
        let height = (measured.height + 4).min(desktop.height);
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
        }
        .render(rect, buf);
        let layout = WindowFrame::layout(rect);
        let body = Rect::new(
            layout.body.x + 1,
            layout.body.y + 1,
            layout.body.width.saturating_sub(2),
            layout.body.height.saturating_sub(1),
        );
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
                self.menu = Some(0);
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
            Part::Row(_) => Some(focus_key(&region.path, "List")),
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
            Part::Tab(n) => Input::Tab(n),
            Part::Open => {
                self.open_dropdown(owner, &region);
                return;
            }
            Part::Fold => {
                self.toggle_fold(owner, &region.key);
                return;
            }
            _ => return,
        };
        self.send(owner, region.path, input);
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
            .filter(|r| r.rect.contains(at) && matches!(r.part, Part::Scroll | Part::List))
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
        self.view_mut(owner)
            .scroll_by(&region.key, i32::from(lines) * 3);
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
        if key.code == KeyCode::F(10) {
            self.menu = if self.menu.is_some() { None } else { Some(0) };
            return;
        }
        if let Some(hover) = self.menu {
            let count = self.menu_state().len();
            match key.code {
                KeyCode::Esc => self.menu = None,
                KeyCode::Up => self.menu = Some(hover.checked_sub(1).unwrap_or(count - 1)),
                KeyCode::Down => self.menu = Some((hover + 1) % count),
                KeyCode::Enter => self.pick_menu(hover),
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
            (Part::List, KeyCode::Up) => self.list_step(owner, &region, -1),
            (Part::List, KeyCode::Down) => self.list_step(owner, &region, 1),
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
                let Some((mut value, multi)) = answer.field else {
                    return;
                };
                match code {
                    KeyCode::Char(c) if !ctrl => value.push(c),
                    KeyCode::Backspace => {
                        value.pop();
                    }
                    KeyCode::Enter if multi => value.push('\n'),
                    KeyCode::Enter if answer.enters => {
                        self.send(owner, region.path, Input::Enter);
                        return;
                    }
                    KeyCode::Enter => {
                        // Enter in a box's one-line field presses its default button: the last
                        // one of its bottom row, which the gui draws leftmost (OK before Cancel)
                        if owner == Owner::Dialog {
                            let default = regions
                                .iter()
                                .filter(|r| matches!(r.part, Part::Press(_)))
                                .max_by_key(|r| {
                                    (
                                        r.rect.y,
                                        match r.part {
                                            Part::Press(n) => n,
                                            _ => 0,
                                        },
                                    )
                                })
                                .cloned();
                            if let Some(Region {
                                path,
                                part: Part::Press(n),
                                ..
                            }) = default
                            {
                                self.send(owner, path, Input::Press(n));
                            }
                        }
                        return;
                    }
                    _ => return,
                }
                self.send(owner, region.path, Input::Text(value));
            }
            _ => {}
        }
    }
}

/// the focus key of the whole list or tab row at `path`.
fn focus_key(path: &[usize], part: &str) -> String {
    let mut key = path
        .iter()
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(".");
    key.push('#');
    key.push_str(part);
    key
}

#[cfg(test)]
mod tests;
