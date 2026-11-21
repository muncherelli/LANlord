//! the progress dialog of an apply: a bar, the task being worked on, how long it has taken, the
//! tasks done so far and the raw output under Details. it follows the events the apply sends
//! to the log (see `lanlord_model::apply`), so the gui, the tui and the web ui show the same.

use lanlord_model::apply::{self, Mark, Progress};
pub use lanlord_model::apply::{ApplyEvent, Stats};

use crate::log::{Kind, Line};
use crate::view::{Button, Node};

/// how many raw lines the details keep.
const KEEP: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Msg {
    Close,
    /// open or close the details under the task list.
    Details,
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
    /// the details are open. kept here, not in the front end, so a click always lands, and kept
    /// from one apply to the next.
    details: bool,
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
        if matches!(event, ApplyEvent::Started)
            || !self.active
            || (self.progress.finished.is_some() && !matches!(event, ApplyEvent::Log { .. }))
        {
            // the first event of a new apply starts a clean dialog
            *self = Self {
                active: true,
                fresh: true,
                details: self.details,
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
            Msg::Details => self.details = !self.details,
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
        let tasks: Vec<(Kind, String)> = p
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
        let details = self.details.then(|| {
            // the raw output of the run, or the whole task list when the run printed none
            let lines = if self.raw.is_empty() {
                tasks.clone()
            } else {
                self.raw.clone()
            };
            if lines.is_empty() {
                vec![(Kind::Debug, "No output yet.".to_string())]
            } else {
                lines
            }
        });
        let mut view = vec![
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
            Node::Button(Button::new(
                if self.details {
                    "\u{25bc} Details"
                } else {
                    "\u{25ba} Details"
                },
                true,
                Msg::Details,
            )),
            Node::Buttons(vec![Button::new("Close", !self.running(), Msg::Close)]),
            Node::Status {
                text: match (p.finished, failed) {
                    (None, _) => "Applying settings...".into(),
                    (Some(_), true) => "The apply failed; see the failed task above.".into(),
                    (Some(_), false) => "The apply finished.".into(),
                },
                error: failed,
            },
        ];
        if let Some(details) = details {
            view.insert(
                5,
                Node::Log {
                    id: "apply-raw",
                    lines: details,
                    height: Some(120.0),
                },
            );
        }
        view
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanlord_model::apply::TaskStatus;

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
    fn a_new_apply_drops_what_a_cut_short_one_left() {
        let mut p = ApplyProgress::default();
        event(&mut p, ApplyEvent::Total { n: 5 });
        event(&mut p, ApplyEvent::TaskStart { name: "a".into() });
        assert!(p.running());
        // no finish ever came; the next apply still starts clean
        event(&mut p, ApplyEvent::Started);
        event(&mut p, ApplyEvent::Total { n: 2 });
        assert!(p.running());
        assert_eq!(p.progress.total, 2);
        assert!(p.progress.current.is_none());
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

    fn done(name: &str, status: TaskStatus, msg: &str) -> ApplyEvent {
        ApplyEvent::TaskDone {
            name: name.into(),
            status,
            changed: false,
            msg: msg.into(),
        }
    }

    fn task_lines(p: &ApplyProgress) -> Vec<(Kind, String)> {
        match &p.view()[3] {
            Node::Log { lines, .. } => lines.clone(),
            _ => panic!("the task list is a log"),
        }
    }

    #[test]
    fn every_kind_of_task_has_its_own_mark_and_colour() {
        let mut p = ApplyProgress::default();
        event(&mut p, ApplyEvent::Total { n: 4 });
        event(&mut p, done("a", TaskStatus::Ok, ""));
        event(&mut p, done("b", TaskStatus::Skipped, ""));
        event(&mut p, done("c", TaskStatus::Failed, "no space"));
        event(&mut p, done("d", TaskStatus::Unreachable, ""));
        let kinds: Vec<Kind> = task_lines(&p).iter().map(|l| l.0).collect();
        assert_eq!(kinds, [Kind::Out, Kind::Debug, Kind::Err, Kind::Err]);
        assert_eq!(task_lines(&p)[2].1, "FAILED  c no space");
    }

    #[test]
    fn a_failed_run_says_so_in_the_status_bar_and_can_be_closed() {
        let mut p = ApplyProgress::default();
        event(&mut p, ApplyEvent::Total { n: 2 });
        event(&mut p, done("a", TaskStatus::Failed, "boom"));
        event(
            &mut p,
            ApplyEvent::Finished {
                stats: Stats {
                    failed: 1,
                    ..Stats::default()
                },
            },
        );
        let view = p.view();
        assert!(matches!(
            view.last(),
            Some(Node::Status { error: true, text }) if text.contains("failed")
        ));
        let Some(Node::Buttons(buttons)) = view.iter().find(|n| matches!(n, Node::Buttons(_)))
        else {
            panic!("a close button")
        };
        assert!(buttons[0].enabled);
        assert!(!p.take_done());
        p.update(Msg::Close);
        assert!(p.take_done());
        assert!(!p.take_done());
    }

    #[test]
    fn a_clean_run_says_it_finished_and_a_running_one_says_it_works() {
        let mut p = ApplyProgress::default();
        event(&mut p, ApplyEvent::Total { n: 1 });
        assert!(matches!(
            p.view().last(),
            Some(Node::Status { error: false, text }) if text.contains("Applying")
        ));
        event(
            &mut p,
            ApplyEvent::Finished {
                stats: Stats::default(),
            },
        );
        assert!(matches!(
            p.view().last(),
            Some(Node::Status { error: false, text }) if text.contains("finished")
        ));
    }

    #[test]
    fn the_elapsed_time_counts_minutes_and_hours() {
        let mut p = ApplyProgress::default();
        p.tick(1.0);
        assert_eq!(p.elapsed(), "0:00", "no run, no time");
        event(&mut p, ApplyEvent::Total { n: 1 });
        p.tick(100.0);
        p.tick(100.0 + 3725.0);
        assert_eq!(p.elapsed(), "1:02:05");
        p.tick(50.0);
        assert_eq!(
            p.elapsed(),
            "0:00",
            "a clock that went back does not underflow"
        );
    }

    #[test]
    fn lines_that_are_not_events_or_are_malformed_change_nothing() {
        let mut p = ApplyProgress::default();
        p.line(&Line {
            kind: Kind::Event,
            text: "{not json".into(),
        });
        assert!(!p.running());
        assert!(!p.take_started());
        assert_eq!(p.progress().total, 0);
    }

    #[test]
    fn the_details_keep_only_the_newest_lines() {
        let mut p = ApplyProgress::default();
        event(&mut p, ApplyEvent::Total { n: 1 });
        for n in 0..KEEP + 3 {
            p.line(&Line {
                kind: Kind::Out,
                text: format!("line {n}"),
            });
        }
        assert_eq!(p.raw.len(), KEEP);
        assert_eq!(p.raw[0].1, "line 3");
    }

    #[test]
    fn a_log_event_after_the_end_does_not_start_a_new_run() {
        let mut p = ApplyProgress::default();
        event(&mut p, ApplyEvent::Total { n: 1 });
        event(
            &mut p,
            ApplyEvent::Finished {
                stats: Stats::default(),
            },
        );
        event(
            &mut p,
            ApplyEvent::Log {
                text: "late".into(),
            },
        );
        assert!(!p.running());
        assert_eq!(p.progress().percent(), 100);
    }

    fn details(p: &ApplyProgress) -> Option<Vec<(Kind, String)>> {
        p.view().iter().find_map(|n| match n {
            Node::Log {
                id: "apply-raw",
                lines,
                ..
            } => Some(lines.clone()),
            _ => None,
        })
    }

    fn details_label(p: &ApplyProgress) -> String {
        p.view()
            .iter()
            .find_map(|n| match n {
                Node::Button(b) if b.label.ends_with("Details") => Some(b.label.clone()),
                _ => None,
            })
            .expect("a details button")
    }

    #[test]
    fn the_details_button_opens_and_closes_them_and_the_arrow_follows() {
        let mut p = ApplyProgress::default();
        event(&mut p, ApplyEvent::Total { n: 1 });
        assert!(details(&p).is_none());
        assert_eq!(details_label(&p), "\u{25ba} Details");
        p.update(Msg::Details);
        assert!(details(&p).is_some());
        assert_eq!(details_label(&p), "\u{25bc} Details");
        // a new apply keeps what was chosen
        event(&mut p, ApplyEvent::Started);
        assert!(details(&p).is_some());
        p.update(Msg::Details);
        assert!(details(&p).is_none());
    }

    #[test]
    fn the_details_show_the_raw_output_or_else_every_task_with_its_status() {
        let mut p = ApplyProgress::default();
        p.update(Msg::Details);
        event(&mut p, ApplyEvent::Total { n: 3 });
        event(&mut p, done("a", TaskStatus::Ok, ""));
        event(&mut p, done("b", TaskStatus::Skipped, ""));
        event(&mut p, done("c", TaskStatus::Failed, "no space"));
        let lines = details(&p).unwrap();
        let text: Vec<&str> = lines.iter().map(|l| l.1.as_str()).collect();
        assert_eq!(text, ["ok      a", "skipped b", "FAILED  c no space"]);
        p.line(&Line {
            kind: Kind::Out,
            text: "plain output".into(),
        });
        assert_eq!(
            details(&p).unwrap(),
            [(Kind::Out, "plain output".to_string())]
        );
    }
}
