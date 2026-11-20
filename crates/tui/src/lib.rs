//! the terminal ui: the same desktop, windows, menus and boxes as the gui, drawn in character
//! cells from the shared app layer, with the mouse.

mod layout;
pub mod paint;
mod shell;
mod signals;
mod view;

use std::io::{stdout, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use lanlord_app::gateway::{Gateway, Ssh};
use lanlord_app::hostlist::HostList;
use lanlord_app::login::SystemAuth;
use lanlord_app::updates::Notice;
use lanlord_core::hosts;
use lanlord_core::prefs;
use lanlord_core::update;
use lanlord_core::updater::RealClock;
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
    stopped: impl Fn() -> bool,
) -> Result<()> {
    let mut shown = String::new();
    while !shell.quit && !stopped() {
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
    shell.set_prefs(lanlord_core::prefs::path());
    watch_updates(&mut shell);
    // a rebuilt build comes back to the server it was on and tries the agent again
    if let Ok(host) = std::env::var("LANLORD_LAST_HOST") {
        if std::env::var("LANLORD_RELAUNCHED").as_deref() == Ok("1") {
            shell.resume(&host);
        }
    }
    // a stop from outside (SIGTERM, SIGHUP) ends the loop like a quit, so the terminal is given
    // back by the same path; listen before the terminal is taken, so a failure leaves it alone
    let stop = signals::Stop::listen()?;
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
        || stop.requested().is_some(),
    );
    drop(capture);
    ratatui::restore();
    // the terminal is back, so the signal can end the process as it would have
    stop.finish();
    result?;
    // a rebuild ended this build so that the new one can take over this terminal
    match shell.take_restart() {
        Some((exe, host)) => {
            let args: Vec<_> = std::env::args_os().skip(1).collect();
            update::restart_in_terminal(&exe, &args, host.as_deref())
        }
        None => Ok(()),
    }
}

/// check for updates in the background as the gui does, and rebuild from the checkout when this
/// is a development build.
fn watch_updates(shell: &mut Shell) {
    let prefs = prefs::path().map(|p| prefs::load(&p)).unwrap_or_default();
    let notice = Notice::new(
        Arc::new(update::System),
        prefs::path(),
        prefs.check_updates,
        Arc::new(RealClock::new()),
        Arc::new(|| {}),
    );
    shell.watch_updates(notice, update::dev_repo(), std::env::current_exe().ok());
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanlord_app::gateway::Fake;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn the_loop_ends_at_once_when_a_stop_was_asked_for() {
        let mut shell = Shell::new(
            Arc::new(Fake::new()),
            HostList::new(None),
            paint::palette::Palette::default(),
        );
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        run_loop(&mut terminal, &mut shell, || None, |_| {}, || true).unwrap();
        assert!(!shell.quit, "it stopped without anyone quitting");
    }

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
            || false,
        )
        .unwrap();
        assert!(shell.quit);
        assert_eq!(
            titles,
            [format!(
                "LANlord Client v{}",
                lanlord_model::version::display()
            )]
        );
    }
}
