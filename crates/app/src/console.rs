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
    /// the browser's stream of the server's apply events.
    #[cfg(target_arch = "wasm32")]
    feed: Option<crate::gateway::stream::Stream>,
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
            #[cfg(target_arch = "wasm32")]
            feed: None,
        }
    }

    /// a console that hears nothing, for pictures that must not depend on what other work logs.
    pub fn detached() -> Self {
        let (_gone, incoming) = std::sync::mpsc::channel();
        Self {
            apply: ApplyProgress::default(),
            lines: Vec::new(),
            incoming,
            #[cfg(target_arch = "wasm32")]
            feed: None,
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
            feed: None,
        }
    }

    /// in the browser, follow the server's apply events while `on` (signed in to a gateway whose
    /// account may see them; the server refuses the stream otherwise) and
    /// feed them to the log as an apply on this machine would, so the progress dialog and the
    /// console show them. a lost connection is noted in the console, once each way.
    pub fn follow_server(&mut self, on: bool, wake: &Wake) {
        #[cfg(target_arch = "wasm32")]
        {
            use crate::gateway::stream::{Item, Stream, Update};
            use lanlord_model::apply::ApplyEvent;
            if !on {
                self.feed = None;
            } else if self.feed.is_none() {
                let wake = wake.clone();
                let down = std::cell::Cell::new(false);
                let handler = move |update: Update| {
                    match update {
                        Update::Item(Item::Apply(event)) => {
                            if let Ok(text) = serde_json::to_string(&event) {
                                log::emit(Kind::Event, text);
                            }
                            if let ApplyEvent::Log { text } = event {
                                log::emit(Kind::Out, text);
                            }
                        }
                        Update::Item(Item::Sample(_)) | Update::SignedOut => {}
                        Update::Refused(why) => log::emit(Kind::Info, why),
                        Update::Up => {
                            if down.replace(false) {
                                log::emit(Kind::Info, "The live feed from the server is back.");
                            }
                        }
                        Update::Down(_) => {
                            if !down.replace(true) {
                                log::emit(Kind::Info, "Lost the live feed from the server.");
                            }
                        }
                    }
                    wake.wake();
                };
                self.feed = Some(Stream::open(
                    lanlord_model::operations::topic::APPLY,
                    std::rc::Rc::new(handler),
                ));
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (on, wake);
    }

    /// move what arrived since the last frame into the list; true when there was anything.
    pub fn drain(&mut self) -> bool {
        let mut any = false;
        for line in self.incoming.try_iter() {
            any = true;
            self.apply.line(&line);
            // events are for the progress dialog, not for reading
            if line.kind != Kind::Event {
                self.lines.push(line);
            }
        }
        if self.lines.len() > KEEP {
            self.lines.drain(..self.lines.len() - KEEP);
        }
        any
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
    fn a_detached_console_hears_nothing() {
        let mut console = Console::detached();
        crate::log::emit(Kind::Err, "detached-test");
        console.drain();
        assert!(console.lines().is_empty());
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

    #[test]
    fn events_feed_the_progress_and_stay_out_of_the_lines() {
        let mut console = Console::new(Wake::none());
        let (tx, rx) = std::sync::mpsc::channel();
        console.incoming = rx;
        tx.send(Line {
            kind: Kind::Event,
            text: r#"{"event":"total","n":3}"#.into(),
        })
        .unwrap();
        tx.send(Line {
            kind: Kind::Info,
            text: "plain".into(),
        })
        .unwrap();
        console.drain();
        assert_eq!(console.apply.progress().total, 3);
        assert_eq!(console.lines().len(), 1);
        assert_eq!(console.lines()[0].text, "plain");
    }
}
