//! the remote commands window: type a command, run it on the gateway, see
//! its output here and in the console. nt4 machines are not reachable yet.

use std::sync::mpsc::Receiver;

use eframe::egui::{Frame, Key, Margin, RichText, ScrollArea, TextEdit, Ui};
use lanlord_app::log::{self, Kind, Line};

use crate::console;
use crate::controls;
use crate::theme::{self, BLACK};
use crate::widgets::Table;
use lanlord_app::services::Services;
use lanlord_app::tasks::Job;

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
        let idle = !services.busy(host) && self.running.is_none();
        let mut enter = false;
        controls::group(ui, "Run on the gateway", |ui| {
            enter = ui
                .horizontal(|ui| {
                    let width = ui.available_width() - 100.0;
                    let edit = controls::field(ui, |ui| {
                        ui.add(
                            TextEdit::singleline(&mut self.line)
                                .hint_text("command to run on the gateway")
                                .desired_width(width)
                                .frame(Frame::new())
                                .margin(Margin::ZERO),
                        )
                    });
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    let run = theme::action(ui, "Run", idle && !self.line.trim().is_empty());
                    run.clicked() || (enter && idle)
                })
                .inner;
            ui.add_space(4.0);
            self.history_table(ui);
        });
        if enter {
            self.run(services, host);
        }
        ui.add_space(4.0);
        controls::group(ui, "Output", |ui| self.output_pane(ui));
        ui.add_space(4.0);
        controls::group(
            ui,
            "NT4 machines (the protocol is still being researched)",
            |ui| {
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
            },
        );
        ui.add_space(6.0);
        controls::status_bar(
            ui,
            &format!(
                "Target: the gateway, {host}{}",
                if idle { "" } else { " - working..." }
            ),
            false,
        );
    }

    fn history_table(&mut self, ui: &mut Ui) {
        let rows: Vec<Vec<String>> = self.history.iter().map(|c| vec![c.clone()]).collect();
        let table = Table {
            id: "command-history",
            headings: &["Last commands"],
            widths: &[300.0],
            rows: &rows,
            selected: None,
            bad: &[],
            height: 56.0,
        };
        if let Some(i) = table.show(ui) {
            self.line.clone_from(&self.history[i]);
        }
    }

    fn output_pane(&self, ui: &mut Ui) {
        theme::sunken(ui, BLACK, |ui| {
            ui.set_width(ui.available_width());
            ScrollArea::vertical()
                .id_salt("command-output")
                .stick_to_bottom(true)
                .max_height(120.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height(100.0);
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
