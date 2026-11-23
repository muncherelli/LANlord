//! the machines of the nt4 domain with something to run on one of them, and what it printed.
//! the plain panel is the computers tab of the nt4 domain window (a free-form command, and the general presets); the
//! hosting panel is the body of the hosting window (the iis 4 presets). every front end draws
//! the same nodes. a run is one operation that answers when the machine is done (its progress topic needs
//! the run id, which only the answer carries), so the tab shows a running line, then the result.

use lanlord_model::access::Grants;
use lanlord_model::nt4_exec::{self, Preset, PRESETS};
use lanlord_model::operations::{Operation, DEFAULT_TIMEOUT_SECS};
use web_time::Instant;

use crate::gateway::RunOutput;
use crate::remote::Remote;
use crate::services::Services;
use crate::tasks::Job;
use crate::view::{Button, Dialog, Node, Size, Table};

/// the most output lines kept for the page; a run is capped at 1 MiB per stream anyway.
const SHOWN_LINES: usize = 2000;

/// the width of every button, so a row of them lines up.
const BUTTON: f32 = 112.0;

/// may these grants run commands on machines?
pub fn may_run(grants: &Grants) -> bool {
    grants.allows(
        Operation::MachineRun {
            machine: String::new(),
            command: String::new(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
        .requires(),
    )
}

/// may these grants run the iis presets of the hosting component?
pub fn may_run_hosting(grants: &Grants) -> bool {
    grants.allows(
        Operation::MachinePreset {
            machine: String::new(),
            preset: HOSTING_PRESET.into(),
            args: Vec::new(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
        .requires(),
    )
}

/// any preset that `nt4_exec::is_hosting` counts, to ask what the hosting presets need.
const HOSTING_PRESET: &str = "w3svc-stop";

/// which of the two windows a panel is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// the computers tab: free-form commands.
    #[default]
    Plain,
    /// the hosting window: the iis presets.
    Hosting,
}

/// what a click in the page means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Msg {
    Pick(usize),
    Run,
    Iis,
    Reload,
}

/// what a click or a keystroke in the box means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoxMsg {
    Command(String),
    Timeout(String),
    /// the preset at this index of `PRESETS`.
    Preset(usize),
    /// argument `0` of the preset now holds this text.
    Arg(usize, String),
    Ok,
    Cancel,
}

/// which buttons work right now; the view greys out the rest and `update` ignores them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gates {
    pub run: bool,
    pub iis: bool,
    pub reload: bool,
}

/// the box that asks what to run: a command line or a preset, then (for a command, or a preset
/// that changes the machine) whether it is really meant.
struct RunBox {
    /// the host the box was opened on; it only ever runs there.
    host: String,
    machine: String,
    /// `Some` for a preset box: the index in `PRESETS`.
    preset: Option<usize>,
    /// the box offers the hosting presets (the hosting window) rather than the general ones.
    hosting: bool,
    command: String,
    args: Vec<String>,
    timeout: String,
    /// asking "are you sure?" about `shown`.
    confirming: bool,
    /// the user pressed OK once, so empty fields are an error now.
    tried: bool,
    error: Option<String>,
    focused: bool,
}

impl RunBox {
    fn free(host: &str, machine: &str) -> Self {
        Self {
            host: host.into(),
            machine: machine.into(),
            preset: None,
            hosting: false,
            command: String::new(),
            args: Vec::new(),
            timeout: DEFAULT_TIMEOUT_SECS.to_string(),
            confirming: false,
            tried: false,
            error: None,
            focused: false,
        }
    }

    fn presets(host: &str, machine: &str, hosting: bool) -> Self {
        let mut run = Self::free(host, machine);
        run.hosting = hosting;
        run.set_preset(first_preset(hosting));
        run
    }

    fn set_preset(&mut self, index: usize) {
        let index = index.min(PRESETS.len() - 1);
        // only this box's kind of preset is offered; a message for another one is not followed
        let index = if nt4_exec::is_hosting(PRESETS[index].id) == self.hosting {
            index
        } else {
            first_preset(self.hosting)
        };
        self.preset = Some(index);
        self.args = vec![String::new(); PRESETS[index].args.len()];
        self.error = None;
    }

    fn the_preset(&self) -> Option<&'static Preset> {
        self.preset.and_then(|i| PRESETS.get(i))
    }

    /// the timeout in seconds, or why it is not one.
    fn secs(&self) -> Result<u32, String> {
        self.timeout
            .trim()
            .parse::<u32>()
            .map_err(|_| "the timeout is a whole number of seconds, 1 to 900".to_string())
    }

    /// the problem with argument `i`, once it has text or OK was pressed.
    fn arg_problem(&self, i: usize) -> Option<String> {
        let arg = self.args.get(i)?;
        if arg.is_empty() && !self.tried {
            return None;
        }
        let name = self.the_preset()?.args.get(i)?;
        nt4_exec::check_arg(arg)
            .err()
            .map(|reason| format!("{name} {reason}"))
    }

    /// the operation this box asks for, checked by the same rules the server applies.
    fn operation(&self) -> Result<Operation, String> {
        let timeout_secs = self.secs()?;
        let machine = self.machine.clone();
        let operation = match self.the_preset() {
            Some(preset) => Operation::MachinePreset {
                machine,
                preset: preset.id.into(),
                args: self.args.clone(),
                timeout_secs,
            },
            None => Operation::MachineRun {
                machine,
                command: self.command.trim().to_string(),
                timeout_secs,
            },
        };
        operation
            .validate()
            .map(|()| operation)
            .map_err(|invalid| format!("{} {}", invalid.field, invalid.reason))
    }

    /// what the machine will run, as the confirmation shows it.
    fn shown(&self) -> String {
        match self.the_preset() {
            Some(preset) => nt4_exec::render(preset.id, &self.args)
                .unwrap_or_else(|reason| format!("cannot show the command: {reason}")),
            None => self.command.trim().to_string(),
        }
    }

    fn needs_confirmation(&self) -> bool {
        nt4_exec::needs_confirmation(self.the_preset().map(|p| p.id))
    }

    fn view(&mut self) -> Dialog<BoxMsg> {
        let mut body = Vec::new();
        let focus = !self.focused;
        self.focused = true;
        let (id, title) = match (self.preset, self.confirming) {
            (_, true) => ("computer-confirm", "Run on machine"),
            (Some(_), false) if self.hosting => ("computer-iis", "IIS presets"),
            (Some(_), false) => ("computer-presets", "Presets"),
            (None, false) => ("computer-run", "Run command"),
        };
        if self.confirming {
            body.push(Node::Alert(format!(
                "Run this on {}? It runs as a domain administrator.",
                self.machine
            )));
            body.push(Node::Code(self.shown()));
        } else {
            body.push(Node::Text(format!("Machine: {}", self.machine)));
            match self.the_preset() {
                Some(preset) => self.preset_fields(preset, &mut body),
                None => {
                    body.push(Node::Text("Command".into()));
                    body.push(Node::Input {
                        value: self.command.clone(),
                        size: Size::FillLess(10.0),
                        focus,
                        hint: "for example ver".into(),
                        edit: Box::new(BoxMsg::Command),
                        enter: None,
                    });
                }
            }
            body.push(Node::Text("Timeout in seconds (1 to 900)".into()));
            body.push(Node::Input {
                value: self.timeout.clone(),
                size: Size::Points(70.0),
                focus: false,
                hint: String::new(),
                edit: Box::new(BoxMsg::Timeout),
                enter: None,
            });
        }
        if let Some(error) = &self.error {
            body.push(Node::Warning(error.clone()));
        }
        body.push(Node::Gap(8.0));
        let ok = if self.confirming { "Run" } else { "OK" };
        body.push(Node::Buttons(vec![
            Button::new("Cancel", true, BoxMsg::Cancel).width(80.0),
            Button::new(ok, true, BoxMsg::Ok).width(80.0),
        ]));
        Dialog {
            id,
            title: title.into(),
            body,
            escape: Some(BoxMsg::Cancel),
        }
    }

    fn preset_fields(&self, preset: &Preset, body: &mut Vec<Node<BoxMsg>>) {
        body.push(Node::Text("Preset".into()));
        body.push(Node::Choice {
            id: "computer-preset",
            current: preset.label.into(),
            width: 330.0,
            options: PRESETS
                .iter()
                .enumerate()
                .filter(|(_, p)| nt4_exec::is_hosting(p.id) == self.hosting)
                .map(|(i, p)| (i, p.label.to_string()))
                .collect(),
            pick: BoxMsg::Preset,
        });
        body.push(Node::Text(
            if preset.mutating {
                "This changes the machine, so it asks first."
            } else {
                "This only reads."
            }
            .into(),
        ));
        for (i, name) in preset.args.iter().enumerate() {
            body.push(Node::Text(format!("Argument: {name}")));
            body.push(Node::Input {
                value: self.args[i].clone(),
                size: Size::FillLess(10.0),
                focus: false,
                hint: String::new(),
                edit: Box::new(move |text| BoxMsg::Arg(i, text)),
                enter: None,
            });
            if let Some(problem) = self.arg_problem(i) {
                body.push(Node::FieldError(problem));
            }
        }
    }
}

/// the index in `PRESETS` of the first hosting preset, or of the first general one.
fn first_preset(hosting: bool) -> usize {
    PRESETS
        .iter()
        .position(|p| nt4_exec::is_hosting(p.id) == hosting)
        .unwrap_or(0)
}

/// a run in progress.
struct Running {
    job: Job<RunOutput>,
    started: Instant,
    /// the host the run was started on.
    host: String,
    machine: String,
    what: String,
}

/// what the last run did.
struct LastRun {
    machine: String,
    what: String,
    /// "exit code 0, 1.2 s", "timed out ..." or the error.
    summary: String,
    error: bool,
    lines: Vec<String>,
}

#[derive(Default)]
pub struct ComputersPanel {
    mode: Mode,
    /// the hosting window only: is the iis part of the component on?
    iis_on: bool,
    remote: Remote<Vec<String>>,
    selected: Option<usize>,
    /// the host `selected` and `last` belong to.
    shown_host: Option<String>,
    can_run: bool,
    ask: Option<RunBox>,
    running: Option<Running>,
    last: Option<LastRun>,
}

fn load(
    gateway: &dyn crate::gateway::Gateway,
    host: &str,
    ask: crate::ask::AskPassword,
) -> anyhow::Result<Vec<String>> {
    let mut names = gateway.machines(host, ask)?;
    names.sort();
    Ok(names)
}

/// the lines of a run's output for the page, stdout then stderr.
fn output_lines(output: &RunOutput) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    lines.extend(output.stdout.lines().map(str::to_string));
    if !output.stderr.trim().is_empty() {
        lines.push("[stderr]".into());
        lines.extend(output.stderr.lines().map(str::to_string));
    }
    if lines.len() > SHOWN_LINES {
        let more = lines.len() - SHOWN_LINES;
        lines.truncate(SHOWN_LINES);
        lines.push(format!("[{more} more lines not shown]"));
    }
    if output.truncated {
        lines.push("[the output was cut at 1 MiB]".into());
    }
    lines
}

/// "1.2 s", or "3 min 5 s" for a long run.
fn took(secs: f32) -> String {
    if secs < 60.0 {
        format!("{secs:.1} s")
    } else {
        let whole = secs as u32;
        format!("{} min {} s", whole / 60, whole % 60)
    }
}

impl ComputersPanel {
    pub fn poll(&mut self) {
        if self.remote.poll() {
            let count = self.remote.data.as_ref().map_or(0, Vec::len);
            self.selected = self.selected.filter(|i| *i < count);
            // another host's machines are not this host's: nothing carries over. only `view`
            // says which host is on screen, so this clears and never sets `shown_host`
            if self.remote.source() != self.shown_host.as_deref() {
                self.selected = None;
                self.last = None;
            }
        }
        let Some(running) = self.running.take() else {
            return;
        };
        match running.job.poll() {
            None => self.running = Some(running),
            Some(result) => {
                // a run started on another host than the one shown is not shown under it
                let shown = self.shown_host.as_deref() == Some(running.host.as_str());
                if shown && self.remote.belongs_to(&running.host) {
                    let secs = running.started.elapsed().as_secs_f32();
                    self.last = Some(finish(running.machine, running.what, result, secs));
                }
            }
        }
    }

    /// may the person run commands? set from their grants; the server decides anyway.
    pub fn set_can_run(&mut self, can: bool) {
        self.can_run = can;
    }

    /// make this the panel of the hosting window.
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    /// hosting window: is the iis part of the component on? the server decides anyway.
    pub fn set_iis_on(&mut self, on: bool) {
        self.iis_on = on;
    }

    pub fn running(&self) -> bool {
        self.running.is_some()
    }

    fn names(&self) -> &[String] {
        self.remote.data.as_deref().unwrap_or(&[])
    }

    fn current(&self) -> Option<&String> {
        self.names().get(self.selected?)
    }

    fn reload(&mut self, services: &Services, host: &str) {
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| load(gateway, &target, ask));
        self.remote
            .track(host, job, "loading...", "loaded the machines", true);
    }

    pub fn gates(&self, services: &Services, host: &str) -> Gates {
        let idle = !services.busy(host) && !self.remote.busy() && self.running.is_none();
        let picked = self.current().is_some() && self.remote.belongs_to(host);
        let may = idle && self.ask.is_none() && self.can_run && picked;
        Gates {
            run: may && self.mode == Mode::Plain,
            iis: may && (self.mode == Mode::Plain || self.iis_on),
            reload: idle,
        }
    }

    pub fn view(&mut self, services: &Services, host: Option<&str>) -> Vec<Node<Msg>> {
        // another host than the one shown: nothing carries over, even before its list arrives
        if host != self.shown_host.as_deref() {
            self.selected = None;
            self.last = None;
            // a box opened on the old host would show its machine under the new one
            self.ask = None;
            self.shown_host = host.map(str::to_string);
        }
        let stale = host.filter(|h| self.remote.wants_load(h, false));
        if let Some(host) = stale {
            self.reload(services, host);
        }
        self.preview(services, host)
    }

    /// the page as it is, without asking the server for anything.
    pub fn preview(&self, services: &Services, host: Option<&str>) -> Vec<Node<Msg>> {
        let Some(host) = host else {
            return crate::view::no_host();
        };
        let gates = self.gates(services, host);
        let rows = self.names().iter().map(|n| vec![n.clone()]).collect();
        let hosting = self.mode == Mode::Hosting;
        let mut buttons = Vec::new();
        if hosting {
            if self.iis_on {
                buttons.push(Button::new("IIS presets...", gates.iis, Msg::Iis).width(BUTTON));
            }
        } else {
            buttons.push(Button::new("Run command...", gates.run, Msg::Run).width(BUTTON));
            buttons.push(Button::new("Presets...", gates.iis, Msg::Iis).width(BUTTON));
        }
        buttons.push(Button::new("Refresh", gates.reload, Msg::Reload).width(BUTTON));
        let mut nodes = vec![
            Node::Text(
                if hosting {
                    "IIS presets run on a domain machine as a domain administrator."
                } else {
                    "Commands run on a domain machine as a domain administrator."
                }
                .into(),
            ),
            Node::Gap(4.0),
            Node::Group {
                title: "Machines".into(),
                body: vec![
                    Node::Table(Table {
                        id: "computers-table",
                        headings: vec!["Machine"],
                        widths: vec![],
                        rows,
                        selected: self.selected,
                        bad: Vec::new(),
                        height: 90.0,
                        select: Msg::Pick,
                        steady: true,
                    }),
                    Node::Gap(4.0),
                    Node::Toolbar(buttons),
                ],
            },
        ];
        if hosting && !self.iis_on {
            nodes.push(Node::Text(
                "Turn on the IIS part of the Hosting component to use the presets.".into(),
            ));
        } else if !self.can_run {
            nodes.push(Node::Text(
                if hosting {
                    "Running IIS presets needs manage rights on Hosting."
                } else {
                    "Running commands needs manage rights on Remote commands."
                }
                .into(),
            ));
        }
        nodes.push(Node::Gap(4.0));
        nodes.push(self.output_group());
        nodes.push(Node::Gap(4.0));
        nodes.push(match &self.running {
            Some(run) => Node::Status {
                text: format!("running on {}: {} ...", run.machine, run.what),
                error: false,
            },
            None => match (&self.remote.note, &self.last) {
                (Some((true, _)), _) | (_, None) => crate::view::status(&self.remote),
                (_, Some(last)) => Node::Status {
                    text: format!("{}: {}", last.machine, last.summary),
                    error: last.error,
                },
            },
        });
        nodes
    }

    fn output_group(&self) -> Node<Msg> {
        let (title, lines) = match (&self.running, &self.last) {
            (Some(run), _) => (
                format!("Output of {} on {}", run.what, run.machine),
                vec!["running...".to_string()],
            ),
            (None, Some(last)) => {
                let mut lines = vec![format!("[{}]", last.summary)];
                lines.extend(last.lines.iter().cloned());
                (
                    format!("Output of {} on {}", last.what, last.machine),
                    lines,
                )
            }
            (None, None) => ("Output".to_string(), Vec::new()),
        };
        Node::Group {
            title,
            body: vec![Node::Lines {
                id: "computers-output",
                lines,
                height: 120.0,
                enabled: true,
            }],
        }
    }

    pub fn update(&mut self, msg: Msg, services: &Services, host: &str) {
        // the same rules as the buttons: a front end may send any message
        let gates = self.gates(services, host);
        let allowed = match msg {
            // rows of another host's list are not this host's machines
            Msg::Pick(_) => self.remote.belongs_to(host),
            Msg::Run => gates.run,
            Msg::Iis => gates.iis,
            Msg::Reload => gates.reload,
        };
        if !allowed {
            return;
        }
        let machine = self.current().cloned();
        match (msg, machine) {
            (Msg::Pick(i), _) => self.selected = Some(i),
            (Msg::Run, Some(machine)) => self.ask = Some(RunBox::free(host, &machine)),
            (Msg::Iis, Some(machine)) => {
                let hosting = self.mode == Mode::Hosting;
                self.ask = Some(RunBox::presets(host, &machine, hosting));
            }
            (Msg::Reload, _) => self.reload(services, host),
            (Msg::Run | Msg::Iis, None) => {}
        }
    }

    /// closes the box when it was opened on another host than `host`: `view` does this only
    /// while the tab is open, and the host can change from another tab of the window.
    pub fn close_stale_box(&mut self, host: Option<&str>) {
        if self
            .ask
            .as_ref()
            .is_some_and(|run| Some(run.host.as_str()) != host)
        {
            self.ask = None;
        }
    }

    /// the box that asks what to run, while one is open.
    pub fn dialog(&mut self) -> Option<Dialog<BoxMsg>> {
        self.ask.as_mut().map(RunBox::view)
    }

    pub fn update_dialog(&mut self, msg: BoxMsg, services: &Services, host: Option<&str>) {
        let Some(mut run) = self.ask.take() else {
            return;
        };
        match msg {
            BoxMsg::Command(text) => run.command = text,
            BoxMsg::Timeout(text) => run.timeout = text,
            BoxMsg::Preset(i) => run.set_preset(i),
            BoxMsg::Arg(i, text) => {
                if let Some(arg) = run.args.get_mut(i) {
                    *arg = text;
                }
            }
            BoxMsg::Cancel if run.confirming => run.confirming = false,
            BoxMsg::Cancel => return,
            BoxMsg::Ok => {
                run.tried = true;
                match run.operation() {
                    Err(error) => run.error = Some(error),
                    Ok(operation) => {
                        run.error = None;
                        if run.needs_confirmation() && !run.confirming {
                            run.confirming = true;
                        } else {
                            let started = match host {
                                Some(host) if host != run.host => Err(format!(
                                    "this box was opened on {}, not {host}; cancel and try again",
                                    run.host
                                )),
                                Some(host) => self.start(services, host, &run, operation),
                                None => Err("there is no host to run on".to_string()),
                            };
                            match started {
                                Ok(()) => return,
                                Err(error) => run.error = Some(error),
                            }
                        }
                    }
                }
            }
        }
        self.ask = Some(run);
    }

    /// start the run; the error is for the box, which stays open with what was typed.
    fn start(
        &mut self,
        services: &Services,
        host: &str,
        run: &RunBox,
        operation: Operation,
    ) -> Result<(), String> {
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| {
            gateway.machine_run(&target, operation.clone(), ask)
        });
        match job {
            Some(job) => {
                self.running = Some(Running {
                    job,
                    started: Instant::now(),
                    host: host.to_string(),
                    machine: run.machine.clone(),
                    what: run.shown(),
                });
                Ok(())
            }
            None => Err("the gateway is busy, try again in a moment".to_string()),
        }
    }

    /// the open box, for tests that fill it in.
    pub fn box_open(&self) -> bool {
        self.ask.is_some()
    }

    pub fn names_shown(&self) -> Vec<String> {
        self.names().to_vec()
    }

    pub fn select(&mut self, name: &str) {
        self.selected = self.names().iter().position(|n| n == name);
    }

    pub fn last_summary(&self) -> Option<&str> {
        self.last.as_ref().map(|l| l.summary.as_str())
    }

    pub fn last_lines(&self) -> &[String] {
        self.last.as_ref().map_or(&[], |l| l.lines.as_slice())
    }

    pub fn belongs_to(&self, host: &str) -> bool {
        self.remote.belongs_to(host)
    }
}

fn finish(machine: String, what: String, result: Result<RunOutput, String>, secs: f32) -> LastRun {
    match result {
        Err(error) => LastRun {
            machine,
            what,
            summary: error,
            error: true,
            lines: Vec::new(),
        },
        Ok(output) => {
            let summary = match (output.timed_out, output.exit_code) {
                (true, _) => format!("timed out after {}", took(secs)),
                (false, Some(code)) => format!("exit code {code}, {}", took(secs)),
                (false, None) => format!("no exit code, {}", took(secs)),
            };
            LastRun {
                machine,
                what,
                error: output.timed_out || output.exit_code != Some(0),
                summary,
                lines: output_lines(&output),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::gateway::Fake;
    use crate::wake::Wake;

    fn panel() -> (ComputersPanel, Services, Arc<Fake>) {
        panel_in(Mode::Plain)
    }

    fn hosting() -> (ComputersPanel, Services, Arc<Fake>) {
        let (mut panel, services, fake) = panel_in(Mode::Hosting);
        panel.set_iis_on(true);
        (panel, services, fake)
    }

    fn panel_in(mode: Mode) -> (ComputersPanel, Services, Arc<Fake>) {
        let fake = Arc::new(Fake::new());
        *fake.machines.lock().unwrap() = vec!["NT4DEV02".into(), "NT4DEV01".into()];
        let services = Services::new(fake.clone(), Wake::none());
        let mut panel = ComputersPanel::default();
        panel.set_mode(mode);
        panel.set_can_run(true);
        panel.shown_host = Some("a@b".into());
        panel.reload(&services, "a@b");
        wait(&mut panel);
        (panel, services, fake)
    }

    fn wait(panel: &mut ComputersPanel) {
        let start = std::time::Instant::now();
        while panel.remote.busy() || panel.running.is_some() {
            panel.poll();
            assert!(start.elapsed() < Duration::from_secs(5), "never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn output(stdout: &str) -> RunOutput {
        RunOutput {
            exit_code: Some(0),
            stdout: stdout.into(),
            stderr: String::new(),
            timed_out: false,
            truncated: false,
        }
    }

    #[test]
    fn the_machines_are_listed_sorted_and_picking_enables_the_buttons() {
        let (mut panel, services, _) = panel();
        assert_eq!(panel.names_shown(), ["NT4DEV01", "NT4DEV02"]);
        let off = panel.gates(&services, "a@b");
        assert!(!off.run && !off.iis && off.reload);
        panel.update(Msg::Pick(1), &services, "a@b");
        let on = panel.gates(&services, "a@b");
        assert!(on.run && on.iis);
        // another host's list is not this host's machine
        assert!(!panel.gates(&services, "c@d").run);
    }

    #[test]
    fn without_manage_rights_nothing_can_run() {
        let (mut panel, services, _) = panel();
        panel.select("NT4DEV01");
        panel.set_can_run(false);
        let gates = panel.gates(&services, "a@b");
        assert!(!gates.run && !gates.iis && gates.reload);
        panel.update(Msg::Run, &services, "a@b");
        panel.update(Msg::Iis, &services, "a@b");
        assert!(!panel.box_open());
    }

    #[test]
    fn rights_follow_the_commands_area() {
        use lanlord_model::access::{Area, Level};
        assert!(may_run(&Grants::all(Level::Manage)));
        let mut view_only = Grants::all(Level::Manage);
        view_only.0.insert(Area::Commands, Level::View);
        assert!(!may_run(&view_only));
    }

    #[test]
    fn a_command_is_checked_then_confirmed_then_run() {
        let (mut panel, services, fake) = panel();
        panel.select("NT4DEV01");
        panel.update(Msg::Run, &services, "a@b");
        // an empty command and a bad timeout are refused in the box
        panel.update_dialog(BoxMsg::Ok, &services, Some("a@b"));
        assert!(panel.ask.as_ref().unwrap().error.is_some());
        panel.update_dialog(BoxMsg::Command("ver".into()), &services, Some("a@b"));
        panel.update_dialog(BoxMsg::Timeout("901".into()), &services, Some("a@b"));
        panel.update_dialog(BoxMsg::Ok, &services, Some("a@b"));
        assert!(!panel.ask.as_ref().unwrap().confirming);
        panel.update_dialog(BoxMsg::Timeout("abc".into()), &services, Some("a@b"));
        panel.update_dialog(BoxMsg::Ok, &services, Some("a@b"));
        assert!(!panel.ask.as_ref().unwrap().confirming);
        panel.update_dialog(BoxMsg::Timeout("30".into()), &services, Some("a@b"));
        panel.update_dialog(BoxMsg::Ok, &services, Some("a@b"));
        assert!(panel.ask.as_ref().unwrap().confirming);
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.contains("machine-run")));
        // cancelling the question goes back to the command, cancelling that closes the box
        panel.update_dialog(BoxMsg::Cancel, &services, Some("a@b"));
        assert!(!panel.ask.as_ref().unwrap().confirming);
        panel.update_dialog(BoxMsg::Ok, &services, Some("a@b"));
        panel.update_dialog(BoxMsg::Ok, &services, Some("a@b"));
        assert!(!panel.box_open() && panel.running());
        wait(&mut panel);
        assert!(fake
            .calls
            .lock()
            .unwrap()
            .contains(&"machine-run a@b NT4DEV01 ver".to_string()));
        assert!(panel.last_summary().unwrap().starts_with("exit code 0"));
        assert_eq!(panel.last_lines(), ["ok"]);
    }

    #[test]
    fn a_free_command_that_exits_the_shell_is_refused_like_the_server_does() {
        let (mut panel, services, _) = panel();
        panel.select("NT4DEV01");
        panel.update(Msg::Run, &services, "a@b");
        panel.update_dialog(BoxMsg::Command("exit 3".into()), &services, Some("a@b"));
        panel.update_dialog(BoxMsg::Ok, &services, Some("a@b"));
        assert!(panel
            .ask
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("exit"));
    }

    #[test]
    fn a_read_only_preset_runs_at_once_and_a_mutating_one_asks() {
        let (mut panel, services, fake) = hosting();
        panel.select("NT4DEV01");
        panel.update(Msg::Iis, &services, "a@b");
        let index = |id: &str| PRESETS.iter().position(|p| p.id == id).unwrap();
        panel.update_dialog(BoxMsg::Preset(index("w3svc-stop")), &services, Some("a@b"));
        panel.update_dialog(BoxMsg::Ok, &services, Some("a@b"));
        assert!(panel.ask.as_ref().unwrap().confirming);
        assert_eq!(panel.ask.as_ref().unwrap().shown(), "net stop w3svc");
        panel.update_dialog(BoxMsg::Cancel, &services, Some("a@b"));
        panel.update_dialog(
            BoxMsg::Preset(index("services-status")),
            &services,
            Some("a@b"),
        );
        panel.update_dialog(BoxMsg::Ok, &services, Some("a@b"));
        assert!(!panel.box_open() && panel.running());
        wait(&mut panel);
        assert!(fake
            .calls
            .lock()
            .unwrap()
            .contains(&"machine-preset a@b NT4DEV01 services-status".to_string()));
    }

    #[test]
    fn preset_arguments_are_checked_inline() {
        let mut run = RunBox::presets("a@b", "NT4DEV01", true);
        let start = PRESETS.iter().position(|p| p.id == "startweb").unwrap();
        run.set_preset(start);
        assert_eq!(run.args.len(), 1);
        // nothing is said about an empty field until OK was pressed
        assert!(run.arg_problem(0).is_none());
        run.args[0] = "1;x".into();
        assert!(run.arg_problem(0).unwrap().starts_with("site "));
        run.args[0] = "1".into();
        assert!(run.arg_problem(0).is_none());
        run.args[0].clear();
        run.tried = true;
        assert!(run.arg_problem(0).is_some());
        assert!(run.operation().is_err());
        run.args[0] = "1".into();
        assert!(run.operation().is_ok());
        run.timeout = "0".into();
        assert!(run.operation().is_err());
        run.timeout = "900".into();
        assert!(run.operation().is_ok());
    }

    #[test]
    fn a_run_is_summed_up_with_its_exit_code_and_timeout() {
        let done = finish("M".into(), "ver".into(), Ok(output("a\r\nb\r\n")), 1.25);
        assert_eq!(
            (done.summary.as_str(), done.error),
            ("exit code 0, 1.2 s", false)
        );
        assert_eq!(done.lines, ["a", "b"]);
        let mut failed = output("");
        failed.exit_code = Some(3);
        failed.stderr = "no\r\n".into();
        let done = finish("M".into(), "x".into(), Ok(failed), 0.1);
        assert!(done.error && done.summary.starts_with("exit code 3"));
        assert_eq!(done.lines, ["[stderr]", "no"]);
        let mut late = output("part");
        late.exit_code = None;
        late.timed_out = true;
        late.truncated = true;
        let done = finish("M".into(), "x".into(), Ok(late), 125.0);
        assert_eq!(done.summary, "timed out after 2 min 5 s");
        assert_eq!(done.lines.last().unwrap(), "[the output was cut at 1 MiB]");
        let done = finish("M".into(), "x".into(), Err("boom".into()), 1.0);
        assert!(done.error && done.summary == "boom");
    }

    #[test]
    fn very_long_output_is_cut_for_the_page() {
        let text = "x\n".repeat(SHOWN_LINES + 5);
        let lines = output_lines(&output(&text));
        assert_eq!(lines.len(), SHOWN_LINES + 1);
        assert_eq!(lines.last().unwrap(), "[5 more lines not shown]");
    }

    #[test]
    fn ok_with_no_host_or_a_busy_gateway_keeps_the_box_open_with_the_error() {
        let (mut panel, services, _) = panel();
        panel.select("NT4DEV01");
        panel.update(Msg::Run, &services, "a@b");
        panel.update_dialog(BoxMsg::Command("ver".into()), &services, Some("a@b"));
        // a free command asks first, then runs
        panel.update_dialog(BoxMsg::Ok, &services, None);
        panel.update_dialog(BoxMsg::Ok, &services, None);
        let open = panel.ask.as_ref().unwrap();
        assert!(open.error.as_ref().unwrap().contains("no host"));
        assert_eq!(open.command, "ver");
        // another job holds the gateway
        let hold = services.spawn("a@b", |_, _| {
            std::thread::sleep(Duration::from_millis(300));
            Ok(())
        });
        assert!(hold.is_some());
        panel.update_dialog(BoxMsg::Ok, &services, Some("a@b"));
        panel.update_dialog(BoxMsg::Ok, &services, Some("a@b"));
        let open = panel.ask.as_ref().unwrap();
        assert!(open.error.as_ref().unwrap().contains("busy"));
        assert_eq!(open.command, "ver");
        assert!(!panel.running());
    }

    #[test]
    fn another_hosts_machines_clear_the_pick_and_the_last_run() {
        let (mut panel, services, _) = panel();
        panel.select("NT4DEV01");
        panel.update(Msg::Run, &services, "a@b");
        panel.update_dialog(BoxMsg::Command("ver".into()), &services, Some("a@b"));
        panel.update_dialog(BoxMsg::Ok, &services, Some("a@b"));
        panel.update_dialog(BoxMsg::Ok, &services, Some("a@b"));
        wait(&mut panel);
        assert!(panel.last_summary().is_some());
        // the same host again keeps both
        panel.reload(&services, "a@b");
        wait(&mut panel);
        assert!(panel.current().is_some() && panel.last_summary().is_some());
        panel.view(&services, Some("c@d"));
        wait(&mut panel);
        assert!(panel.current().is_none() && panel.last_summary().is_none());
    }

    #[test]
    fn a_box_opened_on_one_host_refuses_ok_on_another() {
        let (mut panel, services, fake) = panel();
        let (mut hosted, _, _) = hosting();
        panel.select("NT4DEV01");
        hosted.select("NT4DEV01");
        for (panel, open) in [(&mut panel, Msg::Run), (&mut hosted, Msg::Iis)] {
            panel.update(open, &services, "a@b");
            if panel.ask.as_ref().unwrap().preset.is_none() {
                panel.update_dialog(BoxMsg::Command("ver".into()), &services, Some("a@b"));
                panel.update_dialog(BoxMsg::Ok, &services, Some("c@d"));
            }
            panel.update_dialog(BoxMsg::Ok, &services, Some("c@d"));
            panel.update_dialog(BoxMsg::Ok, &services, Some("c@d"));
            let open = panel.ask.as_ref().unwrap();
            assert!(open.error.as_ref().unwrap().contains("a@b"));
            assert!(!panel.running());
            panel.update_dialog(BoxMsg::Cancel, &services, Some("a@b"));
            panel.update_dialog(BoxMsg::Cancel, &services, Some("a@b"));
            assert!(!panel.box_open());
        }
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.contains("machine-") && c.contains("c@d")));
    }

    #[test]
    fn switching_hosts_clears_the_pick_and_the_last_run_before_any_list_loads() {
        let (mut panel, services, _) = panel();
        panel.select("NT4DEV01");
        panel.update(Msg::Run, &services, "a@b");
        panel.update_dialog(BoxMsg::Command("ver".into()), &services, Some("a@b"));
        panel.update_dialog(BoxMsg::Ok, &services, Some("a@b"));
        panel.update_dialog(BoxMsg::Ok, &services, Some("a@b"));
        wait(&mut panel);
        // the same host keeps both across frames
        panel.view(&services, Some("a@b"));
        assert!(panel.current().is_some() && panel.last_summary().is_some());
        // the other host's first frame: cleared at once, its list is still loading
        panel.view(&services, Some("c@d"));
        assert!(panel.current().is_none() && panel.last_summary().is_none());
        assert_eq!(panel.shown_host.as_deref(), Some("c@d"));
        // a stale row cannot be picked while the other host's list is not here
        panel.update(Msg::Pick(0), &services, "c@d");
        assert!(panel.current().is_none());
    }

    #[test]
    fn a_run_that_ends_after_the_host_changed_is_not_shown() {
        let (mut panel, services, _) = panel();
        panel.select("NT4DEV01");
        panel.update(Msg::Run, &services, "a@b");
        panel.update_dialog(BoxMsg::Command("ver".into()), &services, Some("a@b"));
        panel.update_dialog(BoxMsg::Ok, &services, Some("a@b"));
        panel.update_dialog(BoxMsg::Ok, &services, Some("a@b"));
        assert!(panel.running());
        panel.view(&services, Some("c@d"));
        wait(&mut panel);
        assert!(panel.last_summary().is_none());
    }

    #[test]
    fn a_box_is_closed_when_the_host_changes_without_a_view() {
        let (mut panel, services, _) = panel();
        panel.select("NT4DEV01");
        panel.update(Msg::Run, &services, "a@b");
        panel.close_stale_box(Some("a@b"));
        assert!(panel.box_open());
        panel.close_stale_box(Some("c@d"));
        assert!(!panel.box_open());
    }

    #[test]
    fn a_run_from_the_old_host_never_shows_under_the_new_one() {
        let (mut panel, services, _) = panel();
        panel.select("NT4DEV01");
        panel.update(Msg::Run, &services, "a@b");
        panel.update_dialog(BoxMsg::Command("ver".into()), &services, Some("a@b"));
        panel.update_dialog(BoxMsg::Ok, &services, Some("a@b"));
        panel.update_dialog(BoxMsg::Ok, &services, Some("a@b"));
        assert!(panel.running());
        // no host is on screen while the old run is still going: this starts no load, so the
        // list still belongs to the run's host and only the `shown_host` guard keeps it out
        panel.view(&services, None);
        let start = std::time::Instant::now();
        while panel.running() {
            panel.poll();
            assert!(start.elapsed() < Duration::from_secs(5), "never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(panel.last_summary().is_none());
        assert!(panel.remote.belongs_to("a@b"));
        assert!(panel.shown_host.is_none());
    }

    #[test]
    fn a_host_change_closes_a_box_opened_on_the_old_host() {
        let (mut panel, services, _) = panel();
        panel.select("NT4DEV01");
        panel.update(Msg::Run, &services, "a@b");
        assert!(panel.box_open());
        panel.view(&services, Some("a@b"));
        assert!(panel.box_open());
        panel.view(&services, Some("c@d"));
        assert!(!panel.box_open());
    }

    #[test]
    fn the_machines_table_keeps_its_height_so_the_output_takes_the_room() {
        let (panel, services, _) = panel();
        let nodes = panel.preview(&services, Some("a@b"));
        let steady = nodes.iter().any(|n| match n {
            Node::Group { body, .. } => body
                .iter()
                .any(|b| matches!(b, Node::Table(table) if table.steady)),
            _ => false,
        });
        assert!(steady);
    }

    #[test]
    fn the_page_shows_equal_width_buttons_and_the_output() {
        let (mut panel, services, _) = panel();
        panel.select("NT4DEV01");
        let nodes = panel.preview(&services, Some("a@b"));
        let widths: Vec<Option<f32>> = nodes
            .iter()
            .filter_map(|n| match n {
                Node::Group { body, .. } => body.iter().find_map(|b| match b {
                    Node::Toolbar(buttons) => Some(buttons.iter().map(|b| b.width).collect()),
                    _ => None,
                }),
                _ => None,
            })
            .next()
            .unwrap();
        assert_eq!(widths, [Some(BUTTON); 3]);
        assert!(nodes
            .iter()
            .any(|n| matches!(n, Node::Group { title, .. } if title == "Output")));
    }

    #[test]
    fn the_two_panels_offer_their_own_buttons_and_only_hosting_presets() {
        let (mut plain, services, _) = panel();
        plain.select("NT4DEV01");
        plain.update(Msg::Iis, &services, "a@b");
        let dialog = plain.ask.as_mut().unwrap().view();
        let offered = dialog.body.iter().find_map(|n| match n {
            Node::Choice { options, .. } => Some(options.clone()),
            _ => None,
        });
        // the computers tab offers the general presets, and none of the iis ones
        let offered = offered.unwrap();
        assert!(offered.iter().any(|(i, _)| PRESETS[*i].id == "ver"));
        assert!(offered
            .iter()
            .all(|(i, _)| !nt4_exec::is_hosting(PRESETS[*i].id)));
        plain.update_dialog(BoxMsg::Cancel, &services, Some("a@b"));
        let (mut hosted, services, _) = hosting();
        hosted.select("NT4DEV01");
        hosted.update(Msg::Run, &services, "a@b");
        assert!(!hosted.box_open());
        hosted.update(Msg::Iis, &services, "a@b");
        let run = hosted.ask.as_mut().unwrap();
        let dialog = run.view();
        let offered = dialog.body.iter().find_map(|n| match n {
            Node::Choice { options, .. } => Some(options.clone()),
            _ => None,
        });
        let offered = offered.unwrap();
        assert!(!offered.is_empty());
        assert!(offered
            .iter()
            .all(|(i, _)| nt4_exec::is_hosting(PRESETS[*i].id)));
        assert!(!offered.iter().any(|(i, _)| PRESETS[*i].id == "ver"));
        // a general preset sent by a front end is not followed
        hosted.update_dialog(
            BoxMsg::Preset(PRESETS.iter().position(|p| p.id == "ver").unwrap()),
            &services,
            Some("a@b"),
        );
        let shown = hosted.ask.as_ref().unwrap().the_preset().unwrap().id;
        assert!(nt4_exec::is_hosting(shown));
    }

    #[test]
    fn hosting_without_its_iis_part_has_no_presets_and_says_why() {
        let (mut hosted, services, _) = hosting();
        hosted.set_iis_on(false);
        hosted.select("NT4DEV01");
        assert!(!hosted.gates(&services, "a@b").iis);
        hosted.update(Msg::Iis, &services, "a@b");
        assert!(!hosted.box_open());
        let nodes = hosted.preview(&services, Some("a@b"));
        let has = |text: &str| {
            nodes
                .iter()
                .any(|n| matches!(n, Node::Text(t) if t.contains(text)))
        };
        assert!(has("Turn on the IIS part"));
        let toolbar = nodes.iter().find_map(|n| match n {
            Node::Group { body, .. } => body.iter().find_map(|b| match b {
                Node::Toolbar(buttons) => Some(buttons.len()),
                _ => None,
            }),
            _ => None,
        });
        assert_eq!(toolbar, Some(1));
    }

    #[test]
    fn hosting_rights_follow_the_hosting_area() {
        use lanlord_model::access::{Area, Level};
        assert!(may_run_hosting(&Grants::all(Level::Manage)));
        let mut commands_only = Grants::all(Level::View);
        commands_only.0.insert(Area::Commands, Level::Manage);
        assert!(!may_run_hosting(&commands_only));
        assert!(may_run(&commands_only));
    }
}
