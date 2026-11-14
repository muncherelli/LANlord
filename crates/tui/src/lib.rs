//! the terminal ui: a host list, a command box and a console.

mod app;
mod hosts;
mod ui;

use std::time::Duration;

use anyhow::{anyhow, Result};
use lanlord_core::apply::apply;
use lanlord_core::bootstrap::{bootstrap, AnsibleMode, Options};
use lanlord_core::crypto;
use lanlord_core::exec::{exec_detached, Target};
use lanlord_core::log::{emit, subscribe, Kind};
use lanlord_core::users::{self, Change};
use ratatui::backend::Backend;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::Terminal;

use app::{App, PasswordRequest, UiEvent};
use std::sync::mpsc::{self, Sender};

/// draw and handle input until the app asks to quit.
fn run_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    mut next_event: impl FnMut() -> Option<Event>,
) -> Result<()> {
    while !app.quit {
        app.drain();
        terminal
            .draw(|frame| ui::draw(frame, app))
            .map_err(|err| anyhow!(err.to_string()))?;
        if let Some(Event::Key(key)) = next_event() {
            if key.kind == KeyEventKind::Press {
                app.on_key(key);
            }
        }
    }
    Ok(())
}

/// run the command on its own thread and report the result through the log.
fn spawn_runner(host: String, command: Vec<String>) {
    std::thread::spawn(
        move || match exec_detached(&Target::Gateway { host }, &command) {
            Ok(code) => emit(Kind::Info, format!("exit {code}")),
            Err(err) => emit(Kind::Err, err.to_string()),
        },
    );
}

/// a password prompt that asks through the ui.
fn asker(requests: Sender<PasswordRequest>) -> impl FnMut(&str) -> Result<String> {
    move |what| {
        let (reply, answer) = mpsc::channel();
        requests
            .send(PasswordRequest {
                prompt: what.to_string(),
                reply,
            })
            .map_err(|_| anyhow!("the ui is gone"))?;
        answer
            .recv()
            .ok()
            .flatten()
            .ok_or_else(|| anyhow!("cancelled"))
    }
}

/// run bootstrap on its own thread; passwords are asked for through the ui.
fn spawn_bootstrap(host: String, requests: Sender<PasswordRequest>) {
    std::thread::spawn(move || {
        let options = Options {
            host,
            ansible: AnsibleMode::detect(),
            passwordless_sudo: false,
        };
        if let Err(err) = bootstrap(&options, &mut asker(requests)) {
            emit(Kind::Err, err.to_string());
        }
    });
}

/// run apply on its own thread, with your settings when you have a key.
fn spawn_apply(host: String, modules: Vec<String>, requests: Sender<PasswordRequest>) {
    std::thread::spawn(move || {
        let result = crypto::load_default()
            .and_then(|identity| apply(&host, identity.as_ref(), &modules, &mut asker(requests)));
        if let Err(err) = result {
            emit(Kind::Err, err.to_string());
        }
    });
}

/// the key every users task needs, or why there is none.
fn key() -> Result<crypto::Identity> {
    crypto::load_default()?.ok_or_else(|| anyhow!("no key yet; run lanlord keygen first"))
}

/// fetch the logins on its own thread and send them to the ui.
fn spawn_users_load(host: String, requests: Sender<PasswordRequest>, events: Sender<UiEvent>) {
    std::thread::spawn(move || {
        let result =
            key().and_then(|identity| users::list_on(&host, &identity, &mut asker(requests)));
        if let Err(err) = &result {
            emit(Kind::Err, err.to_string());
        }
        let _ = events.send(UiEvent::Users(result.map_err(|e| e.to_string())));
    });
}

/// change a login on its own thread and send the resulting logins to the ui.
fn spawn_users_change(
    host: String,
    change: Change,
    requests: Sender<PasswordRequest>,
    events: Sender<UiEvent>,
) {
    std::thread::spawn(move || {
        let result = key().and_then(|id| users::change(&host, &id, &change, &mut asker(requests)));
        if let Err(err) = &result {
            emit(Kind::Err, err.to_string());
        }
        let _ = events.send(UiEvent::Users(result.map_err(|e| e.to_string())));
    });
}

/// open the tui and block until it is closed.
pub fn run() -> Result<()> {
    let logs = subscribe();
    let mut app = App::new(hosts::config_path(), logs, Box::new(spawn_runner))
        .with_bootstrapper(Box::new(spawn_bootstrap))
        .with_applier(Box::new(spawn_apply))
        .with_users(Box::new(spawn_users_load), Box::new(spawn_users_change));
    let mut terminal = ratatui::init();
    let result = run_loop(&mut terminal, &mut app, || {
        event::poll(Duration::from_millis(50))
            .ok()
            .filter(|ready| *ready)
            .and_then(|_| event::read().ok())
    });
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc;

    #[test]
    fn loop_runs_a_scripted_session_and_quits() {
        let ran = Rc::new(RefCell::new(Vec::new()));
        let seen = ran.clone();
        let (_tx, rx) = mpsc::channel();
        let mut app = App::new(
            None,
            rx,
            Box::new(move |host, command| seen.borrow_mut().push((host, command))),
        );
        app.hosts = vec!["me@gw".into()];

        let keys = [
            KeyCode::Enter,
            KeyCode::Char('l'),
            KeyCode::Char('s'),
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::Char('q'),
        ];
        let mut script = keys
            .into_iter()
            .map(|code| Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
        let mut terminal = Terminal::new(TestBackend::new(60, 12)).unwrap();
        run_loop(&mut terminal, &mut app, || script.next()).unwrap();

        assert!(app.quit);
        assert_eq!(
            ran.borrow().as_slice(),
            [("me@gw".to_string(), vec!["ls".to_string()])]
        );
    }

    #[test]
    fn loop_ignores_non_key_events() {
        let (_tx, rx) = mpsc::channel();
        let mut app = App::new(None, rx, Box::new(|_, _| {}));
        let mut events = vec![
            Event::FocusGained,
            Event::Key(KeyEvent::from(KeyCode::Char('q'))),
        ]
        .into_iter();
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        run_loop(&mut terminal, &mut app, || events.next()).unwrap();
        assert!(app.quit);
    }
}
