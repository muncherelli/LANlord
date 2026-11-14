//! the tui's state and what each key does; no terminal code lives here.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use lanlord_core::log::{emit, Kind, Line};
use lanlord_core::ssh::Session;
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
}

/// a background task asking the user for a password; the answer, or `None`
/// when cancelled, goes back through `reply`.
pub struct PasswordRequest {
    pub prompt: String,
    pub reply: Sender<Option<String>>,
}

/// starts bootstrap on a host; it asks for passwords through the sender.
pub type Bootstrapper = Box<dyn FnMut(String, Sender<PasswordRequest>)>;

/// starts `command` on `host`; it reports back through the log.
pub type Runner = Box<dyn FnMut(String, Vec<String>)>;

pub struct App {
    pub hosts: Vec<String>,
    pub selected: usize,
    pub mode: Mode,
    pub input: String,
    pub console: VecDeque<Line>,
    pub quit: bool,
    /// the password a task is waiting for, while in `Mode::Password`.
    pub password: Option<PasswordRequest>,
    hosts_path: Option<PathBuf>,
    logs: Receiver<Line>,
    runner: Runner,
    bootstrapper: Bootstrapper,
    requests: Receiver<PasswordRequest>,
    request_tx: Sender<PasswordRequest>,
}

impl App {
    pub fn new(hosts_path: Option<PathBuf>, logs: Receiver<Line>, runner: Runner) -> Self {
        let hosts = hosts_path.as_deref().map(hosts::load).unwrap_or_default();
        let (request_tx, requests) = mpsc::channel();
        Self {
            hosts,
            selected: 0,
            mode: Mode::Normal,
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
            requests,
            request_tx,
        }
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
            Mode::Command | Mode::AddHost => self.input_key(key.code),
            Mode::Password => self.password_key(key.code),
        }
    }

    fn normal_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected + 1 < self.hosts.len() {
                    self.selected += 1;
                }
            }
            KeyCode::Char('a') => self.mode = Mode::AddHost,
            KeyCode::Char('b') => self.bootstrap(),
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

    fn input_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => {
                self.input.clear();
                self.mode = Mode::Normal;
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(c) => self.input.push(c),
            KeyCode::Enter => match self.mode {
                Mode::Command => self.run_command(),
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
        self.mode = Mode::Normal;
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
}
