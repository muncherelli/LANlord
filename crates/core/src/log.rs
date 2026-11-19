//! one place for everything lanlord prints. the cli prints lines itself; the
//! tui and gui subscribe and show them in their console.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;

pub use lanlord_model::log::*;

static SUBSCRIBERS: Mutex<Vec<Sender<Line>>> = Mutex::new(Vec::new());
static VERBOSE: AtomicBool = AtomicBool::new(false);
static QUIET_TASKS: AtomicBool = AtomicBool::new(false);
static QUIET_HELD: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// show debug lines on the terminal when nothing is subscribed.
pub fn set_verbose(on: bool) {
    VERBOSE.store(on, Ordering::Relaxed);
}

/// keep what an apply prints (task output and the playbook's own messages) off the terminal when
/// nothing is subscribed. nothing is lost: the lines are held, and `take_held` hands them over
/// so a failure can show them.
pub fn set_quiet_tasks(on: bool) {
    QUIET_TASKS.store(on, Ordering::Relaxed);
}

/// the lines held back while quiet, oldest first; empties the hold.
pub fn take_held() -> Vec<String> {
    std::mem::take(&mut *QUIET_HELD.lock().unwrap())
}

/// the most lines the hold keeps.
const HELD_MAX: usize = 400;

/// whether a quiet terminal keeps `line` back, and the hold it goes to.
fn hold(quiet: bool, line: &Line, held: &Mutex<Vec<String>>) -> bool {
    let hidden = quiet
        && match line.kind {
            Kind::Out => true,
            Kind::Info => is_task_progress(&line.text),
            _ => false,
        };
    if hidden {
        let mut held = held.lock().unwrap();
        if held.len() >= HELD_MAX {
            held.remove(0);
        }
        held.push(line.text.clone());
    }
    hidden
}

/// a line that only reports a task that went well.
fn is_task_progress(text: &str) -> bool {
    ["ok: ", "changed: ", "skipped: ", "applying "]
        .iter()
        .any(|prefix| text.starts_with(prefix))
        || text == "apply finished"
}

/// whether debug lines are asked for (`--verbose`).
pub fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
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
    if hold(QUIET_TASKS.load(Ordering::Relaxed), line, &QUIET_HELD) {
        return;
    }
    match line.kind {
        Kind::Out => println!("{}", line.text),
        Kind::Err | Kind::Info => eprintln!("{}", line.text),
        Kind::Debug if VERBOSE.load(Ordering::Relaxed) => eprintln!("debug: {}", line.text),
        Kind::Debug | Kind::Event => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscribers_receive_every_kind() {
        let rx = subscribe();
        emit(Kind::Debug, "log-recv-debug");
        emit(Kind::Out, "log-recv-out");
        let seen: Vec<Line> = rx
            .try_iter()
            .filter(|l| l.text.starts_with("log-recv-"))
            .collect();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].kind, Kind::Debug);
        assert_eq!(seen[1].kind, Kind::Out);
    }

    #[test]
    fn a_quiet_terminal_holds_task_output_for_a_failure_to_show() {
        let held = Mutex::new(Vec::new());
        let out = |text: &str| Line {
            kind: Kind::Out,
            text: text.into(),
        };
        assert!(hold(true, &out("fatal: [gw]: FAILED! => boom"), &held));
        assert!(hold(
            true,
            &Line {
                kind: Kind::Info,
                text: "ok: [gw]".into()
            },
            &held
        ));
        assert!(!hold(
            true,
            &Line {
                kind: Kind::Info,
                text: "lanlord is installed".into()
            },
            &held
        ));
        assert!(!hold(false, &out("loud"), &held));
        assert_eq!(
            *held.lock().unwrap(),
            ["fatal: [gw]: FAILED! => boom", "ok: [gw]"]
        );
    }

    #[test]
    fn dropped_subscribers_are_forgotten() {
        let rx = subscribe();
        drop(rx);
        // must not panic or block
        emit(Kind::Info, "log-drop-after-drop");
    }
}
