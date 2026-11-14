//! the tui's state and what each key does; no terminal code lives here.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use lanlord_core::log::{emit, Kind, Line};
use lanlord_core::modules::{Module, Status, MODULES};
use lanlord_core::ssh::Session;
use lanlord_core::users::{self, Change, User};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::hosts;

/// how many console lines to keep.
const CONSOLE_LINES: usize = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// moving around the host list.
    Normal,
    /// typing a command to run on the selected host.
    Command,
    /// typing a new host to add.
    AddHost,
    /// typing a password a background task is waiting for.
    Password,
    /// a module's window is open.
    Window,
    /// typing the name of a new login.
    UserName,
    /// typing a login's new password.
    UserPassword,
    /// typing the password again.
    UserRepeat,
    /// asked whether to delete a login.
    UserDelete,
}

/// which pane the arrow keys move in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Hosts,
    Desktop,
}

/// a background task asking the user for a password; the answer, or `None`
/// when cancelled, goes back through `reply`.
pub struct PasswordRequest {
    pub prompt: String,
    pub reply: Sender<Option<String>>,
}

/// what the nt4-domain window shows of the logins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UsersState {
    Idle,
    Loading,
    Loaded(Vec<User>),
    Failed(String),
}

/// results that background tasks send back.
pub enum UiEvent {
    Users(Result<Vec<User>, String>),
}

/// fetches the logins of a host.
pub type UsersLoader = Box<dyn FnMut(String, Sender<PasswordRequest>, Sender<UiEvent>)>;

/// changes a login on a host and reports the logins that result.
pub type UsersChanger = Box<dyn FnMut(String, Change, Sender<PasswordRequest>, Sender<UiEvent>)>;

/// starts bootstrap on a host; it asks for passwords through the sender.
pub type Bootstrapper = Box<dyn FnMut(String, Sender<PasswordRequest>)>;

/// starts apply on a host for the given module ids (all when empty).
pub type Applier = Box<dyn FnMut(String, Vec<String>, Sender<PasswordRequest>)>;

/// starts `command` on `host`; it reports back through the log.
pub type Runner = Box<dyn FnMut(String, Vec<String>)>;

pub struct App {
    pub hosts: Vec<String>,
    pub selected: usize,
    pub mode: Mode,
    pub focus: Focus,
    /// the selected desktop icon.
    pub icon: usize,
    /// the module whose window is open.
    pub window: Option<&'static Module>,
    pub users: UsersState,
    pub user_selected: usize,
    pub input: String,
    pub console: VecDeque<Line>,
    pub quit: bool,
    /// the password a task is waiting for, while in `Mode::Password`.
    pub password: Option<PasswordRequest>,
    hosts_path: Option<PathBuf>,
    logs: Receiver<Line>,
    runner: Runner,
    bootstrapper: Bootstrapper,
    applier: Applier,
    users_loader: UsersLoader,
    users_changer: UsersChanger,
    pending_name: Option<String>,
    first_password: String,
    events: Receiver<UiEvent>,
    event_tx: Sender<UiEvent>,
    requests: Receiver<PasswordRequest>,
    request_tx: Sender<PasswordRequest>,
}

impl App {
    pub fn new(hosts_path: Option<PathBuf>, logs: Receiver<Line>, runner: Runner) -> Self {
        let hosts = hosts_path.as_deref().map(hosts::load).unwrap_or_default();
        let (request_tx, requests) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        Self {
            hosts,
            selected: 0,
            mode: Mode::Normal,
            focus: Focus::Hosts,
            icon: 0,
            window: None,
            users: UsersState::Idle,
            user_selected: 0,
            input: String::new(),
            console: VecDeque::new(),
            quit: false,
            password: None,
            hosts_path,
            logs,
            runner,
            bootstrapper: Box::new(|_, _| {
                emit(Kind::Err, "bootstrap is not available here");
            }),
            applier: Box::new(|_, _, _| {
                emit(Kind::Err, "apply is not available here");
            }),
            users_loader: Box::new(|_, _, _| emit(Kind::Err, "users are not available here")),
            users_changer: Box::new(|_, _, _, _| emit(Kind::Err, "users are not available here")),
            pending_name: None,
            first_password: String::new(),
            events,
            event_tx,
            requests,
            request_tx,
        }
    }

    /// the login a prompt is about.
    pub fn pending_name(&self) -> Option<&str> {
        self.pending_name.as_deref()
    }

    pub fn with_users(mut self, loader: UsersLoader, changer: UsersChanger) -> Self {
        self.users_loader = loader;
        self.users_changer = changer;
        self
    }

    pub fn with_applier(mut self, applier: Applier) -> Self {
        self.applier = applier;
        self
    }

    pub fn with_bootstrapper(mut self, bootstrapper: Bootstrapper) -> Self {
        self.bootstrapper = bootstrapper;
        self
    }

    /// the host commands run on.
    pub fn host(&self) -> Option<&str> {
        self.hosts.get(self.selected).map(String::as_str)
    }

    /// move everything logged since the last call into the console.
    pub fn drain(&mut self) {
        while let Ok(line) = self.logs.try_recv() {
            if self.console.len() == CONSOLE_LINES {
                self.console.pop_front();
            }
            self.console.push_back(line);
        }
        while let Ok(UiEvent::Users(result)) = self.events.try_recv() {
            self.users = match result {
                Ok(list) => {
                    self.user_selected = self.user_selected.min(list.len().saturating_sub(1));
                    UsersState::Loaded(list)
                }
                Err(err) => UsersState::Failed(err),
            };
        }
        while let Ok(request) = self.requests.try_recv() {
            // one prompt at a time; refuse a second until the first is answered
            if self.password.is_some() {
                let _ = request.reply.send(None);
                continue;
            }
            self.password = Some(request);
            self.input.clear();
            self.mode = Mode::Password;
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match self.mode {
            Mode::Normal => self.normal_key(key.code),
            Mode::Command
            | Mode::AddHost
            | Mode::UserName
            | Mode::UserPassword
            | Mode::UserRepeat => self.input_key(key.code),
            Mode::UserDelete => self.delete_key(key.code),
            Mode::Password => self.password_key(key.code),
            Mode::Window => self.window_key(key.code),
        }
    }

    fn normal_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Hosts => Focus::Desktop,
                    Focus::Desktop => Focus::Hosts,
                };
            }
            KeyCode::Up | KeyCode::Left | KeyCode::Char('k' | 'h') => self.step(-1),
            KeyCode::Down | KeyCode::Right | KeyCode::Char('j' | 'l') => self.step(1),
            KeyCode::Char('a') => self.mode = Mode::AddHost,
            KeyCode::Char('b') => self.bootstrap(),
            KeyCode::Char('p') => self.apply(Vec::new()),
            KeyCode::Enter if self.focus == Focus::Desktop => {
                self.window = MODULES.get(self.icon);
                self.mode = Mode::Window;
                if self.is_users_window() {
                    self.load_users();
                }
            }
            KeyCode::Enter | KeyCode::Char('c') => {
                if self.host().is_some() {
                    self.mode = Mode::Command;
                } else {
                    emit(Kind::Info, "no hosts yet; press a to add one");
                }
            }
            _ => {}
        }
    }

    /// move the selection in the focused pane, staying inside it.
    fn step(&mut self, delta: isize) {
        let (index, len) = match self.focus {
            Focus::Hosts => (&mut self.selected, self.hosts.len()),
            Focus::Desktop => (&mut self.icon, MODULES.len()),
        };
        *index = index
            .saturating_add_signed(delta)
            .min(len.saturating_sub(1));
    }

    fn is_users_window(&self) -> bool {
        self.window.is_some_and(|m| m.id == "nt4-domain")
    }

    /// the login selected in the nt4-domain window.
    pub fn selected_user(&self) -> Option<&str> {
        match &self.users {
            UsersState::Loaded(list) => list.get(self.user_selected).map(|u| u.name.as_str()),
            _ => None,
        }
    }

    fn load_users(&mut self) {
        let Some(host) = self.host().map(String::from) else {
            self.users = UsersState::Failed("no host selected".into());
            return;
        };
        self.users = UsersState::Loading;
        (self.users_loader)(host, self.request_tx.clone(), self.event_tx.clone());
    }

    fn change_user(&mut self, change: Change) {
        let Some(host) = self.host().map(String::from) else {
            return;
        };
        self.users = UsersState::Loading;
        self.mode = Mode::Window;
        (self.users_changer)(host, change, self.request_tx.clone(), self.event_tx.clone());
    }

    fn window_key(&mut self, code: KeyCode) {
        let users = self.is_users_window();
        match code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.window = None;
                self.users = UsersState::Idle;
                self.mode = Mode::Normal;
            }
            KeyCode::Up | KeyCode::Char('k') if users => {
                self.user_selected = self.user_selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') if users => {
                if let UsersState::Loaded(list) = &self.users {
                    if self.user_selected + 1 < list.len() {
                        self.user_selected += 1;
                    }
                }
            }
            KeyCode::Char('r') if users => self.load_users(),
            KeyCode::Char('n') if users => {
                self.input.clear();
                self.pending_name = None;
                self.mode = Mode::UserName;
            }
            KeyCode::Char('p') if users => {
                if let Some(name) = self.selected_user().map(String::from) {
                    self.input.clear();
                    self.pending_name = Some(name);
                    self.mode = Mode::UserPassword;
                }
            }
            KeyCode::Char('d') if users => {
                if let Some(name) = self.selected_user().map(String::from) {
                    self.pending_name = Some(name);
                    self.mode = Mode::UserDelete;
                }
            }
            KeyCode::Enter | KeyCode::Char('a') => {
                let Some(module) = self.window else { return };
                if module.status == Status::Available {
                    self.apply(vec![module.id.to_string()]);
                } else {
                    emit(Kind::Info, format!("{} is not built yet", module.name));
                }
            }
            _ => {}
        }
    }

    fn delete_key(&mut self, code: KeyCode) {
        let name = self.pending_name.take();
        match (code, name) {
            (KeyCode::Char('y'), Some(name)) => self.change_user(Change::Remove { name }),
            _ => self.mode = Mode::Window,
        }
    }

    fn submit_user_name(&mut self) {
        let name = self.input.trim().to_string();
        if !users::valid_name(&name) {
            emit(
                Kind::Err,
                "user names are lowercase letters, digits, _ . - and start with a letter",
            );
            return;
        }
        self.input.clear();
        self.pending_name = Some(name);
        self.mode = Mode::UserPassword;
    }

    fn submit_user_password(&mut self) {
        if self.input.is_empty() {
            emit(Kind::Err, "the password is empty");
            return;
        }
        self.first_password = std::mem::take(&mut self.input);
        self.mode = Mode::UserRepeat;
    }

    fn submit_user_repeat(&mut self) {
        let again = std::mem::take(&mut self.input);
        let password = std::mem::take(&mut self.first_password);
        let name = self.pending_name.take();
        if again != password {
            emit(Kind::Err, "the passwords do not match");
            self.mode = Mode::Window;
        } else if let Some(name) = name {
            self.change_user(Change::SetPassword { name, password });
        }
    }

    fn apply(&mut self, modules: Vec<String>) {
        match self.host().map(String::from) {
            Some(host) => (self.applier)(host, modules, self.request_tx.clone()),
            None => emit(Kind::Info, "no hosts yet; press a to add one"),
        }
    }

    fn input_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => {
                let back = match self.mode {
                    Mode::UserName | Mode::UserPassword | Mode::UserRepeat => Mode::Window,
                    _ => Mode::Normal,
                };
                self.input.clear();
                self.first_password.clear();
                self.pending_name = None;
                self.mode = back;
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(c) => self.input.push(c),
            KeyCode::Enter => match self.mode {
                Mode::Command => self.run_command(),
                Mode::UserName => self.submit_user_name(),
                Mode::UserPassword => self.submit_user_password(),
                Mode::UserRepeat => self.submit_user_repeat(),
                _ => self.add_host(),
            },
            _ => {}
        }
    }

    fn bootstrap(&mut self) {
        match self.host().map(String::from) {
            Some(host) => (self.bootstrapper)(host, self.request_tx.clone()),
            None => emit(Kind::Info, "no hosts yet; press a to add one"),
        }
    }

    fn password_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => self.answer(None),
            KeyCode::Enter => {
                let password = std::mem::take(&mut self.input);
                self.answer(Some(password));
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(c) => self.input.push(c),
            _ => {}
        }
    }

    /// reply to the waiting task and return to the host list.
    fn answer(&mut self, password: Option<String>) {
        if let Some(request) = self.password.take() {
            let _ = request.reply.send(password);
        }
        self.input.clear();
        self.mode = if self.window.is_some() {
            Mode::Window
        } else {
            Mode::Normal
        };
    }

    fn run_command(&mut self) {
        let command: Vec<String> = self.input.split_whitespace().map(String::from).collect();
        self.input.clear();
        let Some(host) = self.host().map(String::from) else {
            return;
        };
        if command.is_empty() {
            return;
        }
        emit(Kind::Info, format!("{host}$ {}", command.join(" ")));
        (self.runner)(host, command);
    }

    fn add_host(&mut self) {
        let host = self.input.trim().to_string();
        self.input.clear();
        if host.is_empty() {
            return;
        }
        if let Err(err) = Session::new(&host) {
            emit(Kind::Err, err.to_string());
            return;
        }
        if let Some(path) = &self.hosts_path {
            if let Err(err) = hosts::add(path, &host) {
                emit(Kind::Err, format!("could not save {host}: {err}"));
                return;
            }
            self.hosts = hosts::load(path);
        } else if !self.hosts.contains(&host) {
            self.hosts.push(host.clone());
        }
        self.selected = self.hosts.iter().position(|h| *h == host).unwrap_or(0);
        self.mode = Mode::Normal;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    type Calls = Rc<RefCell<Vec<(String, Vec<String>)>>>;

    fn app_with(hosts: &[&str]) -> (App, Calls) {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let seen = calls.clone();
        let (_tx, rx) = mpsc::channel();
        let mut app = App::new(
            None,
            rx,
            Box::new(move |host, command| seen.borrow_mut().push((host, command))),
        );
        app.hosts = hosts.iter().map(|h| h.to_string()).collect();
        (app, calls)
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn arrows_move_within_the_list() {
        let (mut app, _) = app_with(&["a@one", "a@two"]);
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.host(), Some("a@two"));
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.host(), Some("a@two"));
        app.on_key(key(KeyCode::Char('k')));
        assert_eq!(app.host(), Some("a@one"));
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn q_esc_and_ctrl_c_quit() {
        for k in [
            key(KeyCode::Char('q')),
            key(KeyCode::Esc),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            let (mut app, _) = app_with(&[]);
            app.on_key(k);
            assert!(app.quit);
        }
        // ctrl-c also quits while typing
        let (mut app, _) = app_with(&["a@one"]);
        app.on_key(key(KeyCode::Enter));
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.quit);
    }

    #[test]
    fn enter_runs_the_command_on_the_selected_host() {
        let (mut app, calls) = app_with(&["a@one", "a@two"]);
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Command);
        type_text(&mut app, "uptime  -p");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(
            calls.borrow().as_slice(),
            [(
                "a@two".to_string(),
                vec!["uptime".to_string(), "-p".to_string()]
            )]
        );
        assert!(app.input.is_empty());
        assert_eq!(app.mode, Mode::Command);
    }

    #[test]
    fn empty_commands_and_esc_do_nothing() {
        let (mut app, calls) = app_with(&["a@one"]);
        app.on_key(key(KeyCode::Char('c')));
        app.on_key(key(KeyCode::Enter));
        type_text(&mut app, "ls");
        app.on_key(key(KeyCode::Backspace));
        assert_eq!(app.input, "l");
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.input.is_empty());
        assert!(calls.borrow().is_empty());
    }

    #[test]
    fn command_mode_needs_a_host() {
        let (mut app, _) = app_with(&[]);
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn adding_a_host_selects_it() {
        let (mut app, _) = app_with(&["a@one"]);
        app.on_key(key(KeyCode::Char('a')));
        assert_eq!(app.mode, Mode::AddHost);
        type_text(&mut app, "b@two");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.hosts, ["a@one", "b@two"]);
        assert_eq!(app.host(), Some("b@two"));
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn bad_hosts_are_refused() {
        let (mut app, _) = app_with(&[]);
        app.on_key(key(KeyCode::Char('a')));
        type_text(&mut app, "-oProxyCommand=x");
        app.on_key(key(KeyCode::Enter));
        assert!(app.hosts.is_empty());
        assert_eq!(app.mode, Mode::AddHost);
    }

    #[test]
    fn hosts_are_saved_to_the_file() {
        let dir = std::env::temp_dir().join(format!("lanlord-app-{}", std::process::id()));
        let path = dir.join("hosts");
        let (_tx, rx) = mpsc::channel();
        let mut app = App::new(Some(path.clone()), rx, Box::new(|_, _| {}));
        app.on_key(key(KeyCode::Char('a')));
        type_text(&mut app, "me@gw");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(hosts::load(&path), ["me@gw"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn drain_collects_logged_lines_up_to_the_cap() {
        let (tx, rx) = mpsc::channel();
        let mut app = App::new(None, rx, Box::new(|_, _| {}));
        for i in 0..CONSOLE_LINES + 5 {
            tx.send(Line {
                kind: Kind::Out,
                text: i.to_string(),
            })
            .unwrap();
        }
        app.drain();
        assert_eq!(app.console.len(), CONSOLE_LINES);
        assert_eq!(app.console.front().unwrap().text, "5");
    }

    #[test]
    fn b_starts_bootstrap_on_the_selected_host() {
        let started = Rc::new(RefCell::new(Vec::new()));
        let seen = started.clone();
        let (_tx, rx) = mpsc::channel();
        let mut app = App::new(None, rx, Box::new(|_, _| {}))
            .with_bootstrapper(Box::new(move |host, _| seen.borrow_mut().push(host)));
        app.on_key(key(KeyCode::Char('b')));
        assert!(started.borrow().is_empty());
        app.hosts = vec!["a@one".into()];
        app.on_key(key(KeyCode::Char('b')));
        assert_eq!(started.borrow().as_slice(), ["a@one"]);
    }

    /// an app whose bootstrapper hands its request sender back to the test.
    fn asking_app() -> (App, Rc<RefCell<Option<Sender<PasswordRequest>>>>) {
        let slot = Rc::new(RefCell::new(None));
        let put = slot.clone();
        let (_tx, rx) = mpsc::channel();
        let mut app = App::new(None, rx, Box::new(|_, _| {}))
            .with_bootstrapper(Box::new(move |_, tx| *put.borrow_mut() = Some(tx)));
        app.hosts = vec!["a@one".into()];
        app.on_key(key(KeyCode::Char('b')));
        (app, slot)
    }

    fn ask(slot: &Rc<RefCell<Option<Sender<PasswordRequest>>>>) -> Receiver<Option<String>> {
        let (reply, answer) = mpsc::channel();
        slot.borrow()
            .as_ref()
            .unwrap()
            .send(PasswordRequest {
                prompt: "sudo password".into(),
                reply,
            })
            .unwrap();
        answer
    }

    #[test]
    fn a_password_request_opens_the_prompt_and_answers() {
        let (mut app, slot) = asking_app();
        let answer = ask(&slot);
        app.drain();
        assert_eq!(app.mode, Mode::Password);
        type_text(&mut app, "hunter2");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(answer.recv().unwrap(), Some("hunter2".to_string()));
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.input.is_empty() && app.password.is_none());
    }

    #[test]
    fn esc_cancels_the_prompt() {
        let (mut app, slot) = asking_app();
        let answer = ask(&slot);
        app.drain();
        type_text(&mut app, "oops");
        app.on_key(key(KeyCode::Backspace));
        app.on_key(key(KeyCode::Esc));
        assert_eq!(answer.recv().unwrap(), None);
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn a_second_request_is_refused_while_one_is_open() {
        let (mut app, slot) = asking_app();
        let first = ask(&slot);
        let second = ask(&slot);
        app.drain();
        assert_eq!(second.recv().unwrap(), None);
        assert!(first.try_recv().is_err());
        assert_eq!(app.mode, Mode::Password);
    }

    #[test]
    fn default_bootstrapper_reports_it_is_unavailable() {
        let (_tx, rx) = mpsc::channel();
        let mut app = App::new(None, rx, Box::new(|_, _| {}));
        app.hosts = vec!["a@one".into()];
        app.on_key(key(KeyCode::Char('b')));
    }

    type Applied = Rc<RefCell<Vec<(String, Vec<String>)>>>;

    fn applying_app() -> (App, Applied) {
        let applied = Rc::new(RefCell::new(Vec::new()));
        let seen = applied.clone();
        let (_tx, rx) = mpsc::channel();
        let mut app = App::new(None, rx, Box::new(|_, _| {})).with_applier(Box::new(
            move |host, modules, _| {
                seen.borrow_mut().push((host, modules));
            },
        ));
        app.hosts = vec!["a@one".into()];
        (app, applied)
    }

    #[test]
    fn tab_moves_focus_and_arrows_follow_it() {
        let (mut app, _) = applying_app();
        assert_eq!(app.focus, Focus::Hosts);
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.focus, Focus::Desktop);
        app.on_key(key(KeyCode::Right));
        app.on_key(key(KeyCode::Char('l')));
        assert_eq!(app.icon, 2);
        assert_eq!(app.selected, 0);
        for _ in 0..20 {
            app.on_key(key(KeyCode::Down));
        }
        assert_eq!(app.icon, MODULES.len() - 1);
        for _ in 0..20 {
            app.on_key(key(KeyCode::Left));
        }
        assert_eq!(app.icon, 0);
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.focus, Focus::Hosts);
    }

    #[test]
    fn enter_on_an_icon_opens_its_window_and_esc_closes_it() {
        let (mut app, _) = applying_app();
        app.on_key(key(KeyCode::Tab));
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Window);
        assert_eq!(app.window.unwrap().id, "gateway");
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.window.is_none());
    }

    #[test]
    fn the_window_applies_just_its_module() {
        let (mut app, applied) = applying_app();
        app.on_key(key(KeyCode::Tab));
        app.on_key(key(KeyCode::Enter));
        app.on_key(key(KeyCode::Char('a')));
        assert_eq!(
            applied.borrow().as_slice(),
            [("a@one".to_string(), vec!["gateway".to_string()])]
        );
    }

    #[test]
    fn planned_modules_do_not_apply() {
        let (mut app, applied) = applying_app();
        app.on_key(key(KeyCode::Tab));
        for _ in 0..4 {
            app.on_key(key(KeyCode::Right));
        }
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.window.unwrap().id, "remote-commands");
        app.on_key(key(KeyCode::Enter));
        assert!(applied.borrow().is_empty());
    }

    #[test]
    fn p_applies_everything_and_needs_a_host() {
        let (mut app, applied) = applying_app();
        app.on_key(key(KeyCode::Char('p')));
        assert_eq!(
            applied.borrow().as_slice(),
            [("a@one".to_string(), Vec::<String>::new())]
        );
        let (mut empty, applied) = applying_app();
        empty.hosts.clear();
        empty.on_key(key(KeyCode::Char('p')));
        assert!(applied.borrow().is_empty());
    }

    #[test]
    fn default_applier_reports_it_is_unavailable() {
        let (_tx, rx) = mpsc::channel();
        let mut app = App::new(None, rx, Box::new(|_, _| {}));
        app.hosts = vec!["a@one".into()];
        app.on_key(key(KeyCode::Char('p')));
    }

    type Loads = Rc<RefCell<Vec<String>>>;
    type Changes = Rc<RefCell<Vec<(String, Change)>>>;

    fn users_app() -> (App, Loads, Changes) {
        let loads = Rc::new(RefCell::new(Vec::new()));
        let changes = Rc::new(RefCell::new(Vec::new()));
        let (seen_loads, seen_changes) = (loads.clone(), changes.clone());
        let (_tx, rx) = mpsc::channel();
        let mut app = App::new(None, rx, Box::new(|_, _| {})).with_users(
            Box::new(move |host, _, _| seen_loads.borrow_mut().push(host)),
            Box::new(move |host, change, _, _| seen_changes.borrow_mut().push((host, change))),
        );
        app.hosts = vec!["a@one".into()];
        (app, loads, changes)
    }

    fn open_users_window(app: &mut App) {
        app.on_key(key(KeyCode::Tab));
        app.on_key(key(KeyCode::Right));
        app.on_key(key(KeyCode::Right));
        app.on_key(key(KeyCode::Enter));
    }

    fn deliver(app: &mut App, names: &[&str]) {
        let list = names
            .iter()
            .map(|n| User {
                name: n.to_string(),
            })
            .collect();
        app.event_tx.send(UiEvent::Users(Ok(list))).unwrap();
        app.drain();
    }

    #[test]
    fn opening_the_nt4_window_loads_the_users() {
        let (mut app, loads, _) = users_app();
        open_users_window(&mut app);
        assert_eq!(app.window.unwrap().id, "nt4-domain");
        assert_eq!(loads.borrow().as_slice(), ["a@one"]);
        assert_eq!(app.users, UsersState::Loading);
        deliver(&mut app, &["james", "olive"]);
        assert_eq!(app.selected_user(), Some("james"));
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.selected_user(), Some("olive"));
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.selected_user(), Some("james"));
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.users, UsersState::Idle);
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn other_windows_do_not_load_users() {
        let (mut app, loads, _) = users_app();
        app.on_key(key(KeyCode::Tab));
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.window.unwrap().id, "gateway");
        assert!(loads.borrow().is_empty());
        app.on_key(key(KeyCode::Char('n')));
        assert_eq!(
            app.mode,
            Mode::Window,
            "n only means something in the users window"
        );
    }

    #[test]
    fn errors_and_missing_hosts_show_in_the_window() {
        let (mut app, _, _) = users_app();
        open_users_window(&mut app);
        app.event_tx
            .send(UiEvent::Users(Err("no settings".into())))
            .unwrap();
        app.drain();
        assert_eq!(app.users, UsersState::Failed("no settings".into()));
        let (mut app, loads, _) = users_app();
        app.hosts.clear();
        open_users_window(&mut app);
        assert!(matches!(app.users, UsersState::Failed(_)));
        assert!(loads.borrow().is_empty());
    }

    #[test]
    fn r_reloads_the_users() {
        let (mut app, loads, _) = users_app();
        open_users_window(&mut app);
        app.on_key(key(KeyCode::Char('r')));
        assert_eq!(loads.borrow().len(), 2);
    }

    #[test]
    fn adding_a_user_asks_for_the_name_then_the_password_twice() {
        let (mut app, _, changes) = users_app();
        open_users_window(&mut app);
        app.on_key(key(KeyCode::Char('n')));
        assert_eq!(app.mode, Mode::UserName);
        type_text(&mut app, "Bad Name");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::UserName, "an invalid name is refused");
        for _ in 0..8 {
            app.on_key(key(KeyCode::Backspace));
        }
        type_text(&mut app, "olive");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::UserPassword);
        assert_eq!(app.pending_name(), Some("olive"));
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::UserPassword, "an empty password is refused");
        type_text(&mut app, "secret");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::UserRepeat);
        type_text(&mut app, "secret");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Window);
        assert_eq!(app.users, UsersState::Loading);
        assert_eq!(
            changes.borrow().as_slice(),
            [(
                "a@one".to_string(),
                Change::SetPassword {
                    name: "olive".into(),
                    password: "secret".into()
                }
            )]
        );
    }

    #[test]
    fn mismatched_passwords_change_nothing() {
        let (mut app, _, changes) = users_app();
        open_users_window(&mut app);
        app.on_key(key(KeyCode::Char('n')));
        type_text(&mut app, "olive");
        app.on_key(key(KeyCode::Enter));
        type_text(&mut app, "one");
        app.on_key(key(KeyCode::Enter));
        type_text(&mut app, "two");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Window);
        assert!(changes.borrow().is_empty());
        assert!(app.pending_name().is_none());
    }

    #[test]
    fn p_changes_the_selected_users_password() {
        let (mut app, _, changes) = users_app();
        open_users_window(&mut app);
        app.on_key(key(KeyCode::Char('p')));
        assert_eq!(
            app.mode,
            Mode::Window,
            "nothing is selected before the list loads"
        );
        deliver(&mut app, &["james"]);
        app.on_key(key(KeyCode::Char('p')));
        assert_eq!(app.pending_name(), Some("james"));
        type_text(&mut app, "new");
        app.on_key(key(KeyCode::Enter));
        type_text(&mut app, "new");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(
            changes.borrow()[0].1,
            Change::SetPassword {
                name: "james".into(),
                password: "new".into()
            }
        );
    }

    #[test]
    fn esc_leaves_a_user_prompt_for_the_window() {
        let (mut app, _, changes) = users_app();
        open_users_window(&mut app);
        app.on_key(key(KeyCode::Char('n')));
        type_text(&mut app, "olive");
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Window);
        assert!(app.window.is_some());
        assert!(app.input.is_empty() && changes.borrow().is_empty());
    }

    #[test]
    fn deleting_needs_a_yes() {
        let (mut app, _, changes) = users_app();
        open_users_window(&mut app);
        deliver(&mut app, &["james"]);
        app.on_key(key(KeyCode::Char('d')));
        assert_eq!(app.mode, Mode::UserDelete);
        app.on_key(key(KeyCode::Char('n')));
        assert_eq!(app.mode, Mode::Window);
        assert!(changes.borrow().is_empty());
        app.on_key(key(KeyCode::Char('d')));
        app.on_key(key(KeyCode::Char('y')));
        assert_eq!(
            changes.borrow().as_slice(),
            [(
                "a@one".to_string(),
                Change::Remove {
                    name: "james".into()
                }
            )]
        );
    }

    #[test]
    fn a_sudo_prompt_returns_to_the_open_window() {
        let (mut app, _, _) = users_app();
        open_users_window(&mut app);
        let (reply, answer) = mpsc::channel();
        app.password = Some(PasswordRequest {
            prompt: "sudo password".into(),
            reply,
        });
        app.mode = Mode::Password;
        type_text(&mut app, "pw");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(answer.recv().unwrap(), Some("pw".to_string()));
        assert_eq!(app.mode, Mode::Window);
    }

    #[test]
    fn default_users_hooks_report_they_are_unavailable() {
        let (_tx, rx) = mpsc::channel();
        let mut app = App::new(None, rx, Box::new(|_, _| {}));
        app.hosts = vec!["a@one".into()];
        open_users_window(&mut app);
        app.on_key(key(KeyCode::Char('n')));
        type_text(&mut app, "olive");
        app.on_key(key(KeyCode::Enter));
        type_text(&mut app, "x");
        app.on_key(key(KeyCode::Enter));
        type_text(&mut app, "x");
        app.on_key(key(KeyCode::Enter));
    }
}
