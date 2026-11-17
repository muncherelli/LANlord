//! the Console window: what lanlord prints, collected from `lanlord_core::log`.

use std::sync::mpsc::Receiver;

use crate::apply_progress::ApplyProgress;
use crate::log::{self, Kind, Line};

use crate::view::Node;
use crate::wake::Wake;

/// how many lines to keep.
const KEEP: usize = 2000;

pub struct Console {
    /// the progress of the apply that is running, fed from the same log.
    pub apply: ApplyProgress,
    lines: Vec<Line>,
    incoming: Receiver<Line>,
}

impl Console {
    /// subscribe to the log; `wake` is called whenever a line arrives.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(wake: Wake) -> Self {
        let (forward, incoming) = std::sync::mpsc::channel();
        let from_log = log::subscribe();
        std::thread::spawn(move || {
            for line in from_log {
                if forward.send(line).is_err() {
                    break;
                }
                wake.wake();
            }
        });
        Self {
            apply: ApplyProgress::default(),
            lines: Vec::new(),
            incoming,
        }
    }

    /// in the browser there are no threads to forward lines with, so the frame reads the log
    /// directly in [`Console::drain`]; new lines show at the next repaint.
    #[cfg(target_arch = "wasm32")]
    pub fn new(_wake: Wake) -> Self {
        Self {
            apply: ApplyProgress::default(),
            lines: Vec::new(),
            incoming: log::subscribe(),
        }
    }

    /// move what arrived since the last frame into the list.
    pub fn drain(&mut self) {
        for line in self.incoming.try_iter() {
            self.apply.line(&line);
            // events are for the progress dialog, not for reading
            if line.kind != Kind::Event {
                self.lines.push(line);
            }
        }
        if self.lines.len() > KEEP {
            self.lines.drain(..self.lines.len() - KEEP);
        }
    }

    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// the window: every kept line, newest at the bottom.
    pub fn view<M>(&self) -> Vec<Node<M>> {
        vec![Node::Log {
            id: "console",
            lines: self
                .lines
                .iter()
                .map(|l| (l.kind, l.text.clone()))
                .collect(),
            height: None,
        }]
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn lines_arrive_in_order_and_old_ones_fall_off() {
        let mut console = Console::new(Wake::none());
        for n in 0..KEEP + 5 {
            log::emit(Kind::Info, format!("console-test-{n}"));
        }
        let start = Instant::now();
        loop {
            console.drain();
            let done = format!("console-test-{}", KEEP + 4);
            let arrived = console.lines().iter().any(|l| l.text == done);
            if arrived || start.elapsed() > Duration::from_secs(5) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(console.lines().len(), KEEP);
        // other tests log at the same time, so only look at this test's lines
        let last = console
            .lines()
            .iter()
            .rev()
            .find(|l| l.text.starts_with("console-test-"))
            .unwrap();
        assert_eq!(last.text, format!("console-test-{}", KEEP + 4));
    }

    #[test]
    fn the_window_is_one_log_of_every_line() {
        let mut console = Console::new(Wake::none());
        // filled by hand: emitting would land in the other test's console too
        console.lines.push(Line {
            kind: Kind::Err,
            text: "broken".into(),
        });
        let view = console.view::<()>();
        let [Node::Log { lines, .. }] = view.as_slice() else {
            panic!("the console is one log");
        };
        assert_eq!(lines, &[(Kind::Err, "broken".to_string())]);
    }
}
