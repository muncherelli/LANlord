//! the remote commands window: type a command, run it on the gateway, see its output here and
//! in the console. nt4 machines are not reachable yet.

use std::sync::mpsc::Receiver;

use crate::log::{self, Kind, Line};

use crate::services::Services;
use crate::tasks::Job;
use crate::view::{Button, Node, Size, Table};

const HISTORY: usize = 10;
const KEEP: usize = 500;

struct Running {
    job: Job<i32>,
    lines: Receiver<Line>,
}

/// what a click or a keystroke in the panel means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    /// the command line now reads this.
    Edit(String),
    Run,
    /// Enter on the command line with this text on it: take it and run it.
    Submit(String),
    /// put history entry `0` back on the command line.
    Recall(usize),
}

#[derive(Default)]
pub struct CommandPanel {
    pub line: String,
    history: Vec<String>,
    output: Vec<Line>,
    running: Option<Running>,
}

/// remember `command` as the newest history entry, once.
fn remember(history: &mut Vec<String>, command: &str) {
    history.retain(|c| c != command);
    history.insert(0, command.to_string());
    history.truncate(HISTORY);
}

impl CommandPanel {
    pub fn history(&self) -> &[String] {
        &self.history
    }

    pub fn output(&self) -> &[Line] {
        &self.output
    }

    pub fn running(&self) -> bool {
        self.running.is_some()
    }

    fn push(&mut self, kind: Kind, text: impl Into<String>) {
        self.output.push(Line {
            kind,
            text: text.into(),
        });
        if self.output.len() > KEEP {
            self.output.drain(..self.output.len() - KEEP);
        }
    }

    /// run the command line on `host` through `sh -c`, so quotes and pipes work.
    pub fn run(&mut self, services: &Services, host: &str) {
        let command = self.line.trim().to_string();
        if command.is_empty() || self.running.is_some() {
            return;
        }
        let lines = log::subscribe();
        let (target, shell) = (host.to_string(), command.clone());
        let job = services.spawn(host, move |gateway, _ask| {
            gateway.run(&target, &["sh".into(), "-c".into(), shell])
        });
        match job {
            Some(job) => {
                remember(&mut self.history, &command);
                self.output.clear();
                self.push(Kind::Info, format!("$ {command}"));
                self.running = Some(Running { job, lines });
            }
            None => self.push(Kind::Err, "the gateway is busy"),
        }
    }

    /// collect output and pick up the exit code.
    pub fn poll(&mut self) {
        let Some(running) = self.running.take() else {
            return;
        };
        let result = running.job.poll();
        let lines: Vec<Line> = running.lines.try_iter().collect();
        for line in lines {
            self.push(line.kind, line.text);
        }
        match result {
            None => self.running = Some(running),
            Some(Ok(0)) => self.push(Kind::Info, "exit 0"),
            Some(Ok(code)) => self.push(Kind::Err, format!("exit {code}")),
            Some(Err(error)) => self.push(Kind::Err, error),
        }
    }

    /// the panel for `host`; `machines` are the nt4 machines it cannot reach yet.
    pub fn view(
        &self,
        services: &Services,
        host: Option<&str>,
        machines: &[String],
    ) -> Vec<Node<Msg>> {
        let Some(host) = host else {
            return vec![Node::Text(
                "No host yet. Add one in the Hosts window first.".into(),
            )];
        };
        let idle = !services.busy(host) && self.running.is_none();
        let can_run = idle && !self.line.trim().is_empty();
        vec![
            Node::Group {
                title: "Run on the gateway".into(),
                body: vec![
                    Node::Row(vec![
                        Node::Input {
                            value: self.line.clone(),
                            size: Size::FillLess(100.0),
                            focus: false,
                            hint: "command to run on the gateway".into(),
                            edit: Box::new(Msg::Edit),
                            enter: idle
                                .then(|| Box::new(Msg::Submit) as Box<dyn Fn(String) -> Msg>),
                        },
                        Node::Button(Button::new("Run", can_run, Msg::Run)),
                    ]),
                    Node::Gap(4.0),
                    Node::Table(Table {
                        id: "command-history",
                        headings: vec!["Last commands"],
                        widths: vec![300.0],
                        rows: self.history.iter().map(|c| vec![c.clone()]).collect(),
                        selected: None,
                        bad: Vec::new(),
                        height: 56.0,
                        select: Msg::Recall,
                    }),
                ],
            },
            Node::Gap(4.0),
            Node::Group {
                title: "Output".into(),
                body: vec![Node::Log {
                    id: "command-output",
                    lines: self
                        .output
                        .iter()
                        .map(|l| (l.kind, l.text.clone()))
                        .collect(),
                    height: Some(120.0),
                }],
            },
            Node::Gap(4.0),
            Node::Group {
                title: "NT4 machines (the protocol is still being researched)".into(),
                body: vec![Node::Disabled(vec![Node::Table(Table {
                    id: "command-machines",
                    headings: vec!["Machines (not available yet)"],
                    widths: vec![200.0],
                    rows: machines.iter().map(|m| vec![m.clone()]).collect(),
                    selected: None,
                    bad: Vec::new(),
                    height: 40.0,
                    select: Msg::Recall,
                })])],
            },
            Node::Gap(6.0),
            Node::Status {
                text: format!(
                    "Target: the gateway, {host}{}",
                    if idle { "" } else { " - working..." }
                ),
                error: false,
            },
        ]
    }

    /// act on a click or keystroke for `host`.
    pub fn update(&mut self, msg: Msg, services: &Services, host: &str) {
        match msg {
            Msg::Edit(text) => self.line = text,
            Msg::Run => self.run(services, host),
            Msg::Submit(text) => {
                self.line = text;
                self.run(services, host);
            }
            Msg::Recall(i) => {
                if let Some(command) = self.history.get(i) {
                    self.line.clone_from(command);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_keeps_the_newest_first_without_repeats() {
        let mut history = Vec::new();
        for i in 0..12 {
            remember(&mut history, &format!("cmd{i}"));
        }
        remember(&mut history, "cmd5");
        assert_eq!(history.len(), HISTORY);
        assert_eq!(history[0], "cmd5");
        assert_eq!(history.iter().filter(|c| *c == "cmd5").count(), 1);
    }
}
