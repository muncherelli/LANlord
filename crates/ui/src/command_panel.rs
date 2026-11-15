//! the remote commands window: type a command, run it on the gateway, see
//! its output here and in the console. nt4 machines are not reachable yet.

use std::sync::mpsc::Receiver;

use eframe::egui::{Key, RichText, ScrollArea, TextEdit, Ui};
use lanlord_core::log::{self, Kind, Line};

use crate::console;
use crate::services::Services;
use crate::tasks::Job;
use crate::theme::{self, BLACK};
use crate::widgets::Table;

const HISTORY: usize = 10;
const KEEP: usize = 500;

struct Running {
    job: Job<i32>,
    lines: Receiver<Line>,
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
    #[cfg(test)]
    pub fn history(&self) -> &[String] {
        &self.history
    }

    #[cfg(test)]
    pub fn output(&self) -> &[Line] {
        &self.output
    }

    #[cfg(test)]
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

    pub fn show(
        &mut self,
        ui: &mut Ui,
        services: &Services,
        host: Option<&str>,
        machines: &[String],
    ) {
        let Some(host) = host else {
            ui.label("No host yet. Add one in the Hosts window first.");
            return;
        };
        ui.label(format!("Target: the gateway, {host}"));
        ui.label(RichText::new("NT4 machines: protocol still being researched").italics());
        let rows: Vec<Vec<String>> = machines.iter().map(|m| vec![m.clone()]).collect();
        let table = Table {
            id: "command-machines",
            headings: &["Machines (not available yet)"],
            widths: &[200.0],
            rows: &rows,
            selected: None,
            bad: &[],
            height: 40.0,
        };
        ui.add_enabled_ui(false, |ui| table.show(ui));
        let idle = !services.busy(host) && self.running.is_none();
        let enter = ui
            .horizontal(|ui| {
                let edit = ui.add(
                    TextEdit::singleline(&mut self.line)
                        .hint_text("command to run on the gateway")
                        .desired_width(ui.available_width() - 90.0),
                );
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let run = theme::action(ui, "Run", idle && !self.line.trim().is_empty());
                run.clicked() || (enter && idle)
            })
            .inner;
        if enter {
            self.run(services, host);
        }
        self.history_table(ui);
        self.output_pane(ui);
    }

    fn history_table(&mut self, ui: &mut Ui) {
        ui.label("History:");
        let rows: Vec<Vec<String>> = self.history.iter().map(|c| vec![c.clone()]).collect();
        let table = Table {
            id: "command-history",
            headings: &["Last commands"],
            widths: &[300.0],
            rows: &rows,
            selected: None,
            bad: &[],
            height: 60.0,
        };
        if let Some(i) = table.show(ui) {
            self.line.clone_from(&self.history[i]);
        }
    }

    fn output_pane(&self, ui: &mut Ui) {
        ui.label("Output:");
        theme::sunken(ui, BLACK, |ui| {
            ui.set_width(ui.available_width());
            ScrollArea::vertical()
                .id_salt("command-output")
                .stick_to_bottom(true)
                .max_height(ui.available_height().max(60.0) - 6.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height(60.0);
                    for line in &self.output {
                        let text = RichText::new(&line.text)
                            .monospace()
                            .color(console::colour(line.kind));
                        ui.label(text);
                    }
                });
        });
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
