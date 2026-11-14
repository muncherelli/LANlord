//! one place for everything lanlord prints. the cli prints lines itself; the
//! tui and gui subscribe and show them in their console.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// stdout of a remote command.
    Out,
    /// stderr of a remote command.
    Err,
    /// progress worth showing every time.
    Info,
    /// what lanlord is doing, shown with --verbose or in a console.
    Debug,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub kind: Kind,
    pub text: String,
}

static SUBSCRIBERS: Mutex<Vec<Sender<Line>>> = Mutex::new(Vec::new());
static VERBOSE: AtomicBool = AtomicBool::new(false);

/// show debug lines on the terminal when nothing is subscribed.
pub fn set_verbose(on: bool) {
    VERBOSE.store(on, Ordering::Relaxed);
}

/// receive every line from now on; consoles get debug lines too.
pub fn subscribe() -> Receiver<Line> {
    let (tx, rx) = mpsc::channel();
    SUBSCRIBERS.lock().unwrap().push(tx);
    rx
}

/// send a line to the subscribers, or print it when there are none.
pub fn emit(kind: Kind, text: impl Into<String>) {
    let line = Line {
        kind,
        text: text.into(),
    };
    let mut subscribers = SUBSCRIBERS.lock().unwrap();
    // drop subscribers whose receiver is gone
    subscribers.retain(|tx| tx.send(line.clone()).is_ok());
    if subscribers.is_empty() {
        print_line(&line);
    }
}

fn print_line(line: &Line) {
    match line.kind {
        Kind::Out => println!("{}", line.text),
        Kind::Err | Kind::Info => eprintln!("{}", line.text),
        Kind::Debug if VERBOSE.load(Ordering::Relaxed) => eprintln!("debug: {}", line.text),
        Kind::Debug => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscribers_receive_every_kind() {
        let rx = subscribe();
        emit(Kind::Debug, "log-test-debug");
        emit(Kind::Out, "log-test-out");
        let seen: Vec<Line> = rx
            .try_iter()
            .filter(|l| l.text.starts_with("log-test-"))
            .collect();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].kind, Kind::Debug);
        assert_eq!(seen[1].kind, Kind::Out);
    }

    #[test]
    fn dropped_subscribers_are_forgotten() {
        let rx = subscribe();
        drop(rx);
        // must not panic or block
        emit(Kind::Info, "log-test-after-drop");
    }
}
