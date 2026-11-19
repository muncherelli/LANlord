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

/// write the reset escapes to `out` and put the saved settings back on `input`; async-signal-
/// safe calls only.
#[cfg(unix)]
fn restore_terminal(out: i32, input: i32, saved: Option<&libc::termios>) {
    // safety: write and tcsetattr are async-signal-safe
    unsafe {
        libc::write(out, RESET.as_ptr().cast(), RESET.len());
        if let Some(t) = saved {
            libc::tcsetattr(input, libc::TCSANOW, t);
        }
    }
}

/// give the terminal back and exit without unwinding.
#[cfg(unix)]
fn hard_exit(signal: i32, saved: Option<&libc::termios>) -> ! {
    restore_terminal(1, 0, saved);
    // safety: _exit is async-signal-safe
    unsafe { libc::_exit(128 + signal) }
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
    // exits the process at once (SIGTERM is never raised here).
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

    #[test]
    fn restore_writes_the_reset_escapes() {
        let mut fds = [0; 2];
        // safety: plain fd calls on a fresh pipe
        unsafe {
            assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
            restore_terminal(fds[1], -1, None);
            let mut buf = [0u8; 64];
            let n = libc::read(fds[0], buf.as_mut_ptr().cast(), buf.len());
            assert_eq!(&buf[..n as usize], RESET);
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }

    #[test]
    fn restore_puts_the_saved_settings_back() {
        let (mut master, mut slave) = (0, 0);
        let mut pipe = [0; 2];
        // safety: fd calls on a fresh pty and pipe; the termios structs are zeroed first
        unsafe {
            let none = std::ptr::null_mut();
            assert_eq!(
                libc::openpty(&mut master, &mut slave, none, none.cast(), none.cast()),
                0
            );
            assert_eq!(libc::pipe(pipe.as_mut_ptr()), 0);
            let mut saved: libc::termios = std::mem::zeroed();
            assert_eq!(libc::tcgetattr(slave, &mut saved), 0);
            let mut raw = saved;
            raw.c_lflag &= !libc::ECHO;
            assert_eq!(libc::tcsetattr(slave, libc::TCSANOW, &raw), 0);
            restore_terminal(pipe[1], slave, Some(&saved));
            let mut now: libc::termios = std::mem::zeroed();
            assert_eq!(libc::tcgetattr(slave, &mut now), 0);
            assert_ne!(now.c_lflag & libc::ECHO, 0);
            for fd in [master, slave, pipe[0], pipe[1]] {
                libc::close(fd);
            }
        }
    }
}
