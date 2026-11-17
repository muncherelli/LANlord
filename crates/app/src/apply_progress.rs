//! the progress dialog of an apply: a bar, the task being worked on, how long it has taken, the
//! tasks done so far and the raw output under Details. it follows the events the apply sends
//! to the log (see `lanlord_model::apply`), so the gui, the tui and the web ui show the same.

use lanlord_model::apply::{self, ApplyEvent, Mark, Progress};

use crate::log::{Kind, Line};
use crate::view::{Button, Node};

/// how many raw lines the details keep.
const KEEP: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Msg {
    Close,
}

#[derive(Default)]
pub struct ApplyProgress {
    progress: Progress,
    /// an apply has reported in and the dialog has not been dismissed.
    active: bool,
    /// the dialog has not been opened for this run yet.
    fresh: bool,
    raw: Vec<(Kind, String)>,
    /// seconds on the front end's clock when the first event was seen, and now.
    started: Option<f64>,
    now: f64,
    done: bool,
}

impl ApplyProgress {
    /// take one line of the log in: events move the bar, and while an apply runs the other lines
    /// are kept for the details.
    pub fn line(&mut self, line: &Line) {
        if line.kind == Kind::Event {
            if let Ok(event) = serde_json::from_str::<ApplyEvent>(&line.text) {
                self.event(&event);
            }
        } else if self.active && self.progress.running() && !line.text.starts_with(apply::MARKER) {
            self.raw.push((line.kind, line.text.clone()));
            if self.raw.len() > KEEP {
                self.raw.remove(0);
            }
        }
    }

    pub fn event(&mut self, event: &ApplyEvent) {
        if !self.active
            || (self.progress.finished.is_some() && !matches!(event, ApplyEvent::Log { .. }))
        {
            // the first event of a new apply starts a clean dialog
            *self = Self {
                active: true,
                fresh: true,
                ..Self::default()
            };
        }
        self.progress.apply(event);
    }

    /// the front end's clock, in seconds, once a frame; the time on the dialog is measured on it.
    pub fn tick(&mut self, now: f64) {
        self.now = now;
        if self.active && self.started.is_none() {
            self.started = Some(now);
        }
    }

    /// true once per apply, when its dialog should open.
    pub fn take_started(&mut self) -> bool {
        std::mem::take(&mut self.fresh)
    }

    /// the dialog's own Close was pressed.
    pub fn take_done(&mut self) -> bool {
        std::mem::take(&mut self.done)
    }

    pub fn running(&self) -> bool {
        self.active && self.progress.running()
    }

    pub fn progress(&self) -> &Progress {
        &self.progress
    }

    pub fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Close => self.done = true,
        }
    }

    /// "1:05", or "1:02:05" past an hour.
    fn elapsed(&self) -> String {
        let secs = self.started.map_or(0.0, |s| (self.now - s).max(0.0)) as u64;
        match secs {
            s if s >= 3600 => format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60),
            s => format!("{}:{:02}", s / 60, s % 60),
        }
    }

    pub fn view(&self) -> Vec<Node<Msg>> {
        let p = &self.progress;
        let failed = p.finished.is_some_and(|s| s.failed + s.unreachable > 0);
        let tasks = p
            .rows
            .iter()
            .map(|row| {
                let (kind, tag) = match row.mark {
                    Mark::Ok => (Kind::Out, "ok     "),
                    Mark::Changed => (Kind::Info, "changed"),
                    Mark::Skipped => (Kind::Debug, "skipped"),
                    Mark::Failed => (Kind::Err, "FAILED "),
                };
                let tail = if row.detail.is_empty() {
                    String::new()
                } else {
                    format!(" {}", row.detail)
                };
                (kind, format!("{tag} {}{tail}", row.name))
            })
            .collect();
        vec![
            Node::Text(p.caption()),
            Node::Bar {
                fraction: f32::from(p.percent()) / 100.0,
                text: format!("{}%", p.percent()),
            },
            Node::Text(format!("Elapsed: {}", self.elapsed())),
            Node::Log {
                id: "apply-tasks",
                lines: tasks,
                height: None,
            },
            Node::Fold {
                id: "apply-details",
                title: "Details".into(),
                body: vec![Node::Log {
                    id: "apply-raw",
                    lines: self.raw.clone(),
                    height: Some(120.0),
                }],
            },
            Node::Buttons(vec![Button::new("Close", !self.running(), Msg::Close)]),
            Node::Status {
                text: match (p.finished, failed) {
                    (None, _) => "Applying settings...".into(),
                    (Some(_), true) => "The apply failed; see the failed task above.".into(),
                    (Some(_), false) => "The apply finished.".into(),
                },
                error: failed,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanlord_model::apply::{Stats, TaskStatus};

    fn event(p: &mut ApplyProgress, e: ApplyEvent) {
        p.line(&Line {
            kind: Kind::Event,
            text: serde_json::to_string(&e).unwrap(),
        });
    }

    #[test]
    fn the_first_event_opens_the_dialog_once() {
        let mut p = ApplyProgress::default();
        assert!(!p.take_started());
        event(&mut p, ApplyEvent::Total { n: 2 });
        event(&mut p, ApplyEvent::TaskStart { name: "a".into() });
        assert!(p.take_started());
        assert!(!p.take_started());
        assert!(p.running());
    }

    #[test]
    fn the_dialog_shows_the_caption_the_bar_and_the_time() {
        let mut p = ApplyProgress::default();
        event(&mut p, ApplyEvent::Total { n: 4 });
        p.tick(10.0);
        event(&mut p, ApplyEvent::TaskStart { name: "a".into() });
        event(
            &mut p,
            ApplyEvent::TaskDone {
                name: "a".into(),
                status: TaskStatus::Ok,
                changed: true,
                msg: String::new(),
            },
        );
        event(&mut p, ApplyEvent::TaskStart { name: "b".into() });
        p.tick(75.0);
        let view = p.view();
        assert!(matches!(&view[0], Node::Text(t) if t == "Task 2 of 4: b"));
        assert!(matches!(&view[1], Node::Bar { fraction, .. } if (*fraction - 0.25).abs() < 1e-6));
        assert!(matches!(&view[2], Node::Text(t) if t == "Elapsed: 1:05"));
        let Node::Log { lines, .. } = &view[3] else {
            panic!("the task list is a log")
        };
        assert_eq!(lines, &[(Kind::Info, "changed a".to_string())]);
        let Some(Node::Buttons(buttons)) = view.iter().find(|n| matches!(n, Node::Buttons(_)))
        else {
            panic!("a close button")
        };
        assert!(!buttons[0].enabled, "no closing while it runs");
    }

    #[test]
    fn a_finished_run_can_be_closed_and_a_new_one_starts_clean() {
        let mut p = ApplyProgress::default();
        event(&mut p, ApplyEvent::Total { n: 1 });
        event(
            &mut p,
            ApplyEvent::Finished {
                stats: Stats::default(),
            },
        );
        assert!(!p.running());
        assert_eq!(p.progress().percent(), 100);
        event(&mut p, ApplyEvent::Total { n: 5 });
        assert_eq!((p.progress().total, p.progress().percent()), (5, 0));
        assert!(p.running());
    }

    #[test]
    fn raw_output_is_kept_for_the_details_but_not_the_event_lines() {
        let mut p = ApplyProgress::default();
        p.line(&Line {
            kind: Kind::Debug,
            text: "before".into(),
        });
        event(&mut p, ApplyEvent::Total { n: 1 });
        p.line(&Line {
            kind: Kind::Debug,
            text: "plain output".into(),
        });
        p.line(&Line {
            kind: Kind::Debug,
            text: format!("{}{{}}", apply::MARKER),
        });
        assert_eq!(p.raw, [(Kind::Debug, "plain output".to_string())]);
    }
}
