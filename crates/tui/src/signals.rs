//! noticing that the process was told to stop (SIGTERM, SIGHUP), so the loop can end and give
//! the terminal back the way a quit does.

use std::io;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

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
            let mut flags = Vec::new();
            for signal in [SIGTERM, SIGHUP] {
                let flag = Arc::new(AtomicBool::new(false));
                // order matters: the conditional shutdown is registered first, so the first
                // signal still sees the flag unset and only sets it (graceful stop), while a
                // second signal finds it set and exits at once with 128 + signal, which ends
                // a wedged shutdown loop.
                signal_hook::flag::register_conditional_shutdown(
                    signal,
                    128 + signal,
                    flag.clone(),
                )?;
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
