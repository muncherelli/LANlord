//! noticing that the process was told to stop (SIGTERM, SIGHUP), so the loop can end and give
//! the terminal back the way a quit does.

use std::io;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// escapes that turn off mouse reporting and the alt screen and show the cursor again.
#[cfg(unix)]
const RESET: &[u8] = b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1049l\x1b[?25h";

/// the terminal settings of stdin as they are now, if it is a tty.
#[cfg(unix)]
fn cooked_termios() -> Option<libc::termios> {
    // safety: tcgetattr only writes into the zeroed struct we hand it
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        (libc::tcgetattr(0, &mut t) == 0).then_some(t)
    }
}

/// give the terminal back and exit without unwinding; async-signal-safe calls only.
#[cfg(unix)]
fn hard_exit(signal: i32, saved: Option<&libc::termios>) -> ! {
    // safety: write, tcsetattr and _exit are async-signal-safe
    unsafe {
        libc::write(1, RESET.as_ptr().cast(), RESET.len());
        if let Some(t) = saved {
            libc::tcsetattr(0, libc::TCSANOW, t);
        }
        libc::_exit(128 + signal)
    }
}

/// the stop signals that have arrived, kept in flags the signal handlers set.
#[derive(Debug, Default)]
pub(crate) struct Stop {
    flags: Vec<(i32, Arc<AtomicBool>)>,
}

impl Stop {
    /// start listening for SIGTERM and SIGHUP; nothing is listened for off unix.
    pub(crate) fn listen() -> io::Result<Stop> {
        #[cfg(unix)]
        {
            use signal_hook::consts::{SIGHUP, SIGTERM};
            // the cooked terminal settings, taken before the terminal is switched to raw mode;
            // none when stdin is not a tty, and then nothing is restored on a hard exit
            let saved = cooked_termios();
            let mut flags = Vec::new();
            for signal in [SIGTERM, SIGHUP] {
                let flag = Arc::new(AtomicBool::new(false));
                let seen = flag.clone();
                // order matters: this is registered before the flag handler, so the first
                // signal still sees the flag unset and only sets it (graceful stop), while a
                // second one finds it set and ends a wedged shutdown loop. that exit skips
                // the normal terminal restore, so it first does it by hand with async-signal-
                // safe calls only (write, tcsetattr, _exit): mouse capture, alt screen and
                // cursor escapes, then the saved termios, then 128 + signal.
                // safety: the closure only calls async-signal-safe functions
                unsafe {
                    signal_hook::low_level::register(signal, move || {
                        if seen.load(std::sync::atomic::Ordering::SeqCst) {
                            hard_exit(signal, saved.as_ref());
                        }
                    })?;
                }
                signal_hook::flag::register(signal, flag.clone())?;
                flags.push((signal, flag));
            }
            Ok(Stop { flags })
        }
        #[cfg(not(unix))]
        Ok(Stop::default())
    }

    /// the signal that asked for a stop, if one did.
    pub(crate) fn requested(&self) -> Option<i32> {
        use std::sync::atomic::Ordering;
        self.flags
            .iter()
            .find(|(_, flag)| flag.load(Ordering::Relaxed))
            .map(|(signal, _)| *signal)
    }

    /// let the signal that stopped us take its usual course, now the terminal is back.
    pub(crate) fn finish(&self) {
        #[cfg(unix)]
        if let Some(signal) = self.requested() {
            let _ = signal_hook::low_level::emulate_default_handler(signal);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use signal_hook::consts::{SIGHUP, SIGTERM};

    // one test on purpose: handlers and their flags are process-wide, so raising SIGHUP in a
    // second test could flip the first. only one raise per process, as a second signal now
    // exits the process through the conditional shutdown (and SIGTERM is never raised here).
    #[test]
    fn a_stop_signal_sets_its_flag() {
        let stop = Stop::listen().unwrap();
        assert_eq!(stop.requested(), None);
        signal_hook::low_level::raise(SIGHUP).unwrap();
        // the handler runs on the raising thread before raise returns
        assert_eq!(stop.requested(), Some(SIGHUP));
        assert_ne!(stop.requested(), Some(SIGTERM));
    }

    #[test]
    fn a_flag_set_by_hand_is_found() {
        let stop = Stop::default();
        assert_eq!(stop.requested(), None);
        let flag = Arc::new(AtomicBool::new(true));
        let stop = Stop {
            flags: vec![(SIGTERM, flag)],
        };
        assert_eq!(stop.requested(), Some(SIGTERM));
    }
}
