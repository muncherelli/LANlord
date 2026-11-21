//! what an apply reports while it runs: typed events read from the progress plugin the
//! playbooks ship, and a progress state that only ever moves forward. no i/o, so every front
//! end, the browser included, uses the same code.

use serde::{Deserialize, Serialize};

/// the first word of every line the progress plugin prints; the rest is one json event.
pub const MARKER: &str = "LANLORD_EVENT ";

/// how a task ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Ok,
    Skipped,
    Failed,
    Unreachable,
}

/// the totals ansible prints at the end of a run, added up over hosts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    pub ok: u32,
    pub changed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub unreachable: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ApplyEvent {
    /// an apply begins: a front end drops whatever an earlier one left.
    Started,
    /// `n` more tasks are going to run: a play's tasks, an included file's, a notified handler.
    Total {
        n: u32,
    },
    TaskStart {
        name: String,
    },
    /// one more item of the running looped task finished.
    Item {
        n: u32,
    },
    TaskDone {
        name: String,
        status: TaskStatus,
        #[serde(default)]
        changed: bool,
        /// why it failed, or what it said.
        #[serde(default)]
        msg: String,
    },
    Log {
        text: String,
    },
    /// which engine runs a module in this apply, and why when it is not the one asked for. older
    /// clients ignore it.
    Engine {
        module: String,
        engine: String,
        #[serde(default)]
        reason: String,
    },
    Finished {
        stats: Stats,
    },
}

/// the event in a line of server output, if it is one.
pub fn parse_line(line: &str) -> Option<ApplyEvent> {
    serde_json::from_str(line.trim_end().strip_prefix(MARKER)?).ok()
}

/// the line the progress plugin prints for `event`.
pub fn to_line(event: &ApplyEvent) -> String {
    format!(
        "{MARKER}{}",
        serde_json::to_string(event).unwrap_or_default()
    )
}

/// how a finished task shows in the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Ok,
    Changed,
    Skipped,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub mark: Mark,
    /// "(3 items)" for a looped task, or the failure message.
    pub detail: String,
}

/// where an apply stands. `percent` never goes down, even when more tasks turn up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub total: u32,
    pub done: u32,
    pub current: Option<String>,
    pub items: u32,
    pub rows: Vec<Row>,
    pub finished: Option<Stats>,
    percent: u8,
}

impl Progress {
    /// the bar, 0 to 100.
    pub fn percent(&self) -> u8 {
        self.percent
    }

    /// "Task 12 of 40: name" while running; the task being worked on counts.
    pub fn caption(&self) -> String {
        match (&self.current, self.finished) {
            (_, Some(stats)) if stats.failed + stats.unreachable > 0 => {
                format!("Failed after {} of {} tasks", self.done, self.total)
            }
            (_, Some(_)) => format!("Finished {} tasks", self.done),
            (Some(name), None) => {
                let step = self.done + 1;
                let sub = match self.items {
                    0 => String::new(),
                    n => format!(" ({n} items)"),
                };
                format!("Task {step} of {}: {name}{sub}", self.total.max(step))
            }
            (None, None) => "Starting...".into(),
        }
    }

    pub fn running(&self) -> bool {
        self.finished.is_none()
    }

    fn finish_task(&mut self, name: &str, status: TaskStatus, changed: bool, msg: &str) {
        // a skipped task is finished too; a looped one counts once, here
        self.done += 1;
        let mark = match status {
            TaskStatus::Skipped => Mark::Skipped,
            TaskStatus::Failed | TaskStatus::Unreachable => Mark::Failed,
            TaskStatus::Ok if changed => Mark::Changed,
            TaskStatus::Ok => Mark::Ok,
        };
        let detail = match (self.items, msg.is_empty()) {
            (_, false) => msg.to_string(),
            (0, true) => String::new(),
            (n, true) => format!("({n} items)"),
        };
        self.rows.push(Row {
            name: name.into(),
            mark,
            detail,
        });
        self.current = None;
        self.items = 0;
    }

    /// take one event in. the total only grows and so does the percentage.
    pub fn apply(&mut self, event: &ApplyEvent) {
        match event {
            ApplyEvent::Total { n } => self.total = self.total.saturating_add(*n),
            ApplyEvent::TaskStart { name } => {
                self.current = Some(name.clone());
                self.items = 0;
            }
            ApplyEvent::Item { n } => self.items = self.items.max(*n),
            ApplyEvent::TaskDone {
                name,
                status,
                changed,
                msg,
            } => self.finish_task(name, *status, *changed, msg),
            ApplyEvent::Started | ApplyEvent::Log { .. } | ApplyEvent::Engine { .. } => {}
            ApplyEvent::Finished { stats } => {
                self.finished = Some(*stats);
                self.current = None;
            }
        }
        // tasks nobody counted (a skipped tag, a dynamic include) raise the total
        self.total = self.total.max(self.done);
        let now = match (self.finished, self.total) {
            (Some(stats), _) if stats.failed + stats.unreachable == 0 => 100,
            (_, 0) => 0,
            // never show 100 until the run says it is over
            _ => (u64::from(self.done) * 100 / u64::from(self.total)).min(99) as u8,
        };
        self.percent = self.percent.max(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(status: TaskStatus, changed: bool) -> ApplyEvent {
        ApplyEvent::TaskDone {
            name: "t".into(),
            status,
            changed,
            msg: String::new(),
        }
    }

    fn start(name: &str) -> ApplyEvent {
        ApplyEvent::TaskStart { name: name.into() }
    }

    fn run(events: &[ApplyEvent]) -> Progress {
        let mut p = Progress::default();
        for e in events {
            p.apply(e);
        }
        p
    }

    #[test]
    fn percent_is_finished_over_total() {
        let p = run(&[
            ApplyEvent::Total { n: 4 },
            start("a"),
            done(TaskStatus::Ok, false),
        ]);
        assert_eq!((p.done, p.total, p.percent()), (1, 4, 25));
        assert_eq!(p.caption(), "Starting...");
    }

    #[test]
    fn a_skipped_task_counts_as_finished() {
        let p = run(&[
            ApplyEvent::Total { n: 2 },
            start("a"),
            done(TaskStatus::Skipped, false),
        ]);
        assert_eq!(p.percent(), 50);
        assert_eq!(p.rows[0].mark, Mark::Skipped);
    }

    #[test]
    fn a_looped_task_counts_once_with_its_items_as_sub_text() {
        let p = run(&[
            ApplyEvent::Total { n: 2 },
            start("loop"),
            ApplyEvent::Item { n: 1 },
            ApplyEvent::Item { n: 2 },
            ApplyEvent::Item { n: 3 },
        ]);
        assert_eq!(p.caption(), "Task 1 of 2: loop (3 items)");
        assert_eq!(p.done, 0);
        let p = run(&[
            ApplyEvent::Total { n: 2 },
            start("loop"),
            ApplyEvent::Item { n: 3 },
            done(TaskStatus::Ok, true),
        ]);
        assert_eq!((p.done, p.percent()), (1, 50));
        assert_eq!(p.rows[0].mark, Mark::Changed);
        assert_eq!(p.rows[0].detail, "(3 items)");
    }

    #[test]
    fn more_tasks_never_move_the_bar_back() {
        let mut p = run(&[
            ApplyEvent::Total { n: 2 },
            start("a"),
            done(TaskStatus::Ok, false),
        ]);
        assert_eq!(p.percent(), 50);
        // an include adds eight tasks: 1 of 10 is less than 50, so the bar holds
        p.apply(&ApplyEvent::Total { n: 8 });
        assert_eq!((p.total, p.percent()), (10, 50));
        for _ in 0..4 {
            p.apply(&done(TaskStatus::Ok, false));
            assert_eq!(p.percent(), 50);
        }
        p.apply(&done(TaskStatus::Ok, false));
        assert_eq!(p.percent(), 60);
    }

    #[test]
    fn it_never_goes_backwards_whatever_comes() {
        let mut p = Progress::default();
        let mut last = 0;
        let mut events = vec![ApplyEvent::Total { n: 3 }];
        for i in 0..12 {
            events.push(start("x"));
            events.push(done(TaskStatus::Ok, false));
            if i % 3 == 0 {
                events.push(ApplyEvent::Total { n: 5 });
            }
        }
        for e in &events {
            p.apply(e);
            assert!(p.percent() >= last);
            last = p.percent();
        }
    }

    #[test]
    fn uncounted_tasks_raise_the_total_instead_of_overflowing() {
        let p = run(&[
            ApplyEvent::Total { n: 1 },
            done(TaskStatus::Ok, false),
            done(TaskStatus::Ok, false),
        ]);
        assert_eq!(p.total, 2);
        assert_eq!(p.percent(), 99, "100 waits for the end of the run");
    }

    #[test]
    fn finishing_fills_the_bar_unless_something_failed() {
        let ok = run(&[
            ApplyEvent::Total { n: 5 },
            done(TaskStatus::Ok, false),
            ApplyEvent::Finished {
                stats: Stats::default(),
            },
        ]);
        assert_eq!(ok.percent(), 100);
        assert!(!ok.running());
        let bad = run(&[
            ApplyEvent::Total { n: 5 },
            done(TaskStatus::Failed, false),
            ApplyEvent::Finished {
                stats: Stats {
                    failed: 1,
                    ..Stats::default()
                },
            },
        ]);
        assert_eq!(bad.percent(), 20);
        assert_eq!(bad.caption(), "Failed after 1 of 5 tasks");
        assert_eq!(bad.rows[0].mark, Mark::Failed);
    }

    #[test]
    fn the_caption_names_the_running_task() {
        let p = run(&[
            ApplyEvent::Total { n: 40 },
            start("a"),
            done(TaskStatus::Ok, false),
            start("install packages"),
        ]);
        assert_eq!(p.caption(), "Task 2 of 40: install packages");
    }

    #[test]
    fn lines_round_trip_and_other_output_is_not_an_event() {
        let event = ApplyEvent::TaskDone {
            name: "n".into(),
            status: TaskStatus::Unreachable,
            changed: false,
            msg: "x".into(),
        };
        assert_eq!(parse_line(&to_line(&event)), Some(event));
        assert_eq!(parse_line("TASK [x] ****"), None);
        assert_eq!(parse_line("LANLORD_EVENT {not json"), None);
    }

    /// what the plugin printed for a small play, as a fixture.
    #[test]
    fn the_plugin_fixture_parses_and_drives_the_bar() {
        let fixture = include_str!("../tests/progress_fixture.txt");
        let events: Vec<ApplyEvent> = fixture.lines().filter_map(parse_line).collect();
        assert_eq!(events.len(), 14, "every marked line is an event");
        let p = run(&events);
        assert_eq!(p.total, 4);
        assert_eq!(p.done, 4);
        assert_eq!(p.percent(), 100);
        let marks: Vec<Mark> = p.rows.iter().map(|r| r.mark).collect();
        assert_eq!(
            marks,
            [Mark::Changed, Mark::Skipped, Mark::Changed, Mark::Ok]
        );
        assert_eq!(p.rows[2].detail, "(3 items)");
    }

    #[test]
    fn items_only_grow_and_a_failure_message_beats_the_item_count() {
        let mut p = run(&[
            ApplyEvent::Total { n: 1 },
            start("loop"),
            ApplyEvent::Item { n: 5 },
            ApplyEvent::Item { n: 2 },
        ]);
        assert_eq!(p.items, 5);
        p.apply(&ApplyEvent::TaskDone {
            name: "loop".into(),
            status: TaskStatus::Failed,
            changed: true,
            msg: "disk full".into(),
        });
        assert_eq!(p.rows[0].detail, "disk full");
        assert_eq!(p.rows[0].mark, Mark::Failed);
        assert_eq!(p.items, 0);
    }

    #[test]
    fn an_unreachable_host_is_a_failure_too() {
        let p = run(&[
            ApplyEvent::Total { n: 2 },
            done(TaskStatus::Unreachable, false),
            ApplyEvent::Finished {
                stats: Stats {
                    unreachable: 1,
                    ..Stats::default()
                },
            },
        ]);
        assert_eq!(p.rows[0].mark, Mark::Failed);
        assert_eq!(p.percent(), 50);
        assert!(p.caption().starts_with("Failed after"));
    }

    #[test]
    fn a_run_with_no_total_stays_at_zero_until_it_ends() {
        let mut p = run(&[start("a")]);
        assert_eq!(p.percent(), 0);
        assert_eq!(p.caption(), "Task 1 of 1: a");
        p.apply(&ApplyEvent::Log { text: "x".into() });
        assert_eq!(p.percent(), 0);
        p.apply(&ApplyEvent::Finished {
            stats: Stats::default(),
        });
        assert_eq!(p.percent(), 100);
        assert_eq!(p.caption(), "Finished 0 tasks");
    }

    #[test]
    fn json_with_missing_optional_fields_still_parses() {
        let line = format!(
            "{MARKER}{}",
            r#"{"event":"task_done","name":"n","status":"ok"}"#
        );
        assert_eq!(
            parse_line(&format!("{line}\r\n")),
            Some(ApplyEvent::TaskDone {
                name: "n".into(),
                status: TaskStatus::Ok,
                changed: false,
                msg: String::new(),
            })
        );
        assert_eq!(
            parse_line(&format!("{MARKER}{}", r#"{"event":"nope"}"#)),
            None
        );
    }
}
