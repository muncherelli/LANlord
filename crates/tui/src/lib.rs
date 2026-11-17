//! the terminal ui: the same desktop, windows, menus and boxes as the gui, drawn in character
//! cells from the shared app layer, with the mouse.

pub mod paint;
mod shell;
mod view;

use std::io::{stdout, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use lanlord_app::gateway::{Gateway, Ssh};
use lanlord_app::hostlist::HostList;
use lanlord_app::login::SystemAuth;
use lanlord_core::hosts;
use ratatui::backend::Backend;
use ratatui::crossterm::event::{self, Event};
use ratatui::crossterm::{execute, terminal::SetTitle};
use ratatui::Terminal;

pub use shell::Shell;

/// draw and handle input until the user quits; `title` is told whenever the app's title changes.
fn run_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    shell: &mut Shell,
    mut next_event: impl FnMut() -> Option<Event>,
    mut title: impl FnMut(&str),
) -> Result<()> {
    let mut shown = String::new();
    while !shell.quit {
        shell.poll();
        terminal
            .draw(|frame| {
                let area = frame.area();
                shell.render(area, frame.buffer_mut());
            })
            .map_err(|err| anyhow!(err.to_string()))?;
        let now = shell.title();
        if now != shown {
            title(&now);
            shown = now;
        }
        if let Some(event) = next_event() {
            shell.on_event(event, Instant::now());
        }
    }
    Ok(())
}

/// open the tui and block until it is closed.
pub fn run() -> Result<()> {
    let gateway: Arc<dyn Gateway> = Arc::new(Ssh);
    let mut shell = Shell::new(
        gateway,
        HostList::new(hosts::config_path()),
        paint::palette::Palette::detect(),
    );
    // the logon box comes first, like the gui; LANLORD_SCREEN=desktop skips it
    if std::env::var("LANLORD_SCREEN").as_deref() != Ok("desktop") {
        shell.require_login(Arc::new(SystemAuth::new()));
    }
    if std::env::var("LANLORD_FAKE_SERVER").as_deref() == Ok("1") {
        shell.use_demo_server();
    }
    let mut terminal = ratatui::init();
    // mouse reporting stays on for the length of the loop and goes off when the guard drops
    let capture = paint::mouse::Capture::start()?;
    let result = run_loop(
        &mut terminal,
        &mut shell,
        || {
            event::poll(Duration::from_millis(100))
                .ok()
                .filter(|ready| *ready)
                .and_then(|_| event::read().ok())
        },
        |title| {
            let mut out = stdout();
            let _ = execute!(out, SetTitle(title));
            let _ = out.flush();
        },
    );
    drop(capture);
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanlord_app::gateway::Fake;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn the_loop_draws_names_the_app_and_quits_on_ctrl_q() {
        let mut shell = Shell::new(
            Arc::new(Fake::new()),
            HostList::new(None),
            paint::palette::Palette::default(),
        );
        let mut events = vec![Event::Key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::CONTROL,
        ))]
        .into_iter();
        let mut titles = Vec::new();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        run_loop(
            &mut terminal,
            &mut shell,
            || events.next(),
            |t| titles.push(t.to_string()),
        )
        .unwrap();
        assert!(shell.quit);
        assert_eq!(
            titles,
            [format!("LANlord Client v{}", env!("CARGO_PKG_VERSION"))]
        );
    }
}
