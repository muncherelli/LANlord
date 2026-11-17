//! the Console window: what lanlord prints, collected from `lanlord_core::log`.

use std::sync::mpsc::{self, Receiver};

use crate::log::{self, Line};

use crate::view::Node;
use crate::wake::Wake;

/// how many lines to keep.
const KEEP: usize = 2000;

pub struct Console {
    lines: Vec<Line>,
    incoming: Receiver<Line>,
}

impl Console {
    /// subscribe to the log; `wake` is called whenever a line arrives.
    pub fn new(wake: Wake) -> Self {
        let (forward, incoming) = mpsc::channel();
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
            lines: Vec::new(),
            incoming,
        }
    }

    /// move what arrived since the last frame into the list.
    pub fn drain(&mut self) {
        self.lines.extend(self.incoming.try_iter());
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

    use crate::log::Kind;

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
