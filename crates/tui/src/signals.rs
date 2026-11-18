//! leaving the terminal tidy when the process is told to stop (SIGTERM, SIGHUP).

use std::io::{self, Write};

use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{disable_raw_mode, LeaveAlternateScreen};

use crate::paint::mouse;

/// write what gives the terminal back to the shell: mouse reporting off, the alternate screen
/// left and the cursor shown.
fn leave_screen(out: &mut impl Write) -> io::Result<()> {
    mouse::disable(out)?;
    execute!(out, LeaveAlternateScreen, Show)
}

/// give the terminal back: leave the alternate screen, show the cursor, stop mouse reporting
/// and turn raw mode off. every step runs even if an earlier one fails.
pub(crate) fn restore_terminal(out: &mut impl Write) -> io::Result<()> {
    let screen = leave_screen(out);
    let raw = disable_raw_mode();
    screen.and(raw)
}

/// on SIGTERM or SIGHUP, restore the terminal and exit with the usual 128 plus the signal.
#[cfg(unix)]
pub(crate) fn restore_on_signal() -> io::Result<()> {
    use signal_hook::consts::{SIGHUP, SIGTERM};
    use signal_hook::iterator::Signals;

    let mut signals = Signals::new([SIGTERM, SIGHUP])?;
    std::thread::spawn(move || {
        if let Some(signal) = signals.forever().next() {
            let _ = restore_terminal(&mut io::stdout());
            std::process::exit(128 + signal);
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaving_the_screen_turns_everything_back_on_for_the_shell() {
        let mut out = Vec::new();
        leave_screen(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        // mouse reporting off, the alternate screen left, the cursor shown
        assert!(text.contains("\x1b[?1000l"), "{text:?}");
        assert!(text.contains("\x1b[?1049l"), "{text:?}");
        assert!(text.ends_with("\x1b[?25h"), "{text:?}");
    }

    #[test]
    fn restoring_writes_the_same_even_where_raw_mode_cannot_change() {
        let mut out = Vec::new();
        // raw mode may fail without a terminal; the screen sequences are written regardless
        let _ = restore_terminal(&mut out);
        assert!(String::from_utf8(out).unwrap().contains("\x1b[?1049l"));
    }

    #[cfg(unix)]
    #[test]
    fn the_signals_can_be_watched() {
        restore_on_signal().unwrap();
    }
}
