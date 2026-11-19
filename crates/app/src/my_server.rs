//! the My Server window: what the signed-in server is (General) and the one dangerous thing you
//! can do to it (Advanced): take lanlord back off.

use lanlord_model::compat::{self, mark};
use lanlord_model::doctor::{Report, Status};
use lanlord_model::server::ServerFacts;

use crate::log::{emit, Kind};

use crate::remote::Remote;
use crate::services::Services;
use crate::view::{self, Button, Dialog, Node, Size};

pub const GENERAL: usize = 0;
pub const ADVANCED: usize = 1;
pub const COMPATIBILITY: usize = 2;

/// what a click or a keystroke in the window means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    Tab(usize),
    /// highlight a check on the Compatibility tab.
    Check(usize),
    /// read the server's report again.
    Recheck,
    AskPurge,
    /// the "also remove the administrator accounts" tick.
    ToggleAdmins,
    /// the confirmation field now holds this text.
    Typed(String),
    Cancel,
    Remove,
}

/// the confirmation box for a purge: what would go, who may type the name, the running job.
pub struct PurgeBox {
    host: String,
    preview: Remote<Vec<String>>,
    running: Remote<Vec<String>>,
    remove_admins: bool,
    /// what you typed to confirm; it must equal the host name.
    typed: String,
}

impl PurgeBox {
    pub fn host(&self) -> &str {
        &self.host
    }

    /// what a purge would remove, once the preview has come back.
    pub fn preview(&self) -> Option<&[String]> {
        self.preview.data.as_deref()
    }

    pub fn typed(&self) -> &str {
        &self.typed
    }

    pub fn remove_admins(&self) -> bool {
        self.remove_admins
    }

    fn new(host: &str, services: &Services) -> Self {
        let mut purge = Self {
            host: host.to_string(),
            preview: Remote::default(),
            running: Remote::default(),
            remove_admins: false,
            typed: String::new(),
        };
        purge.look(services);
        purge
    }

    /// list what a purge would remove with the current choices; nothing is changed.
    fn look(&mut self, services: &Services) {
        let (host, admins) = (self.host.clone(), self.remove_admins);
        let job = services.spawn(&self.host, move |gateway, ask| {
            gateway.purge_preview(&host, admins, ask)
        });
        if job.is_none() {
            return;
        }
        self.preview = Remote::default();
        self.preview.track(
            &self.host,
            job,
            "checking what would be removed...",
            "this is everything that would be removed",
            true,
        );
    }

    fn confirmed(&self, services: &Services) -> bool {
        self.typed == self.host
            && self.preview.data.is_some()
            && !self.preview.busy()
            && !self.running.busy()
            && !services.busy(&self.host)
    }

    fn remove(&mut self, services: &Services) {
        let (host, admins) = (self.host.clone(), self.remove_admins);
        let job = services.spawn(&self.host, move |gateway, ask| {
            gateway.purge(&host, admins, ask)
        });
        self.running.track(
            &self.host,
            job,
            "removing lanlord...",
            "lanlord was removed",
            false,
        );
    }
}

#[derive(Default)]
pub struct MyServer {
    tab: usize,
    facts: Remote<ServerFacts>,
    purge: Option<PurgeBox>,
    /// a purge finished on this host; the desktop logs off and forgets it.
    purged: Option<String>,
    /// the doctor's report of this server, copied from the shared cache on each refresh.
    report: Option<Report>,
    checking: bool,
    report_note: Option<(bool, String)>,
    /// the highlighted check on the Compatibility tab.
    picked: usize,
    /// the signed-in account is not an administrator: nothing here changes the server.
    read_only: bool,
}

impl MyServer {
    pub fn tab(&self) -> usize {
        self.tab
    }

    /// the doctor's report of the server shown, once it is read.
    pub fn report(&self) -> Option<&Report> {
        self.report.as_ref()
    }

    /// the server's facts and the job reading them.
    pub fn facts(&self) -> &Remote<ServerFacts> {
        &self.facts
    }

    /// the purge box, while it is open.
    pub fn purge(&self) -> Option<&PurgeBox> {
        self.purge.as_ref()
    }

    /// switch off what changes the server, for an account that is not an administrator.
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    /// can a purge be asked for? the account is an administrator, lanlord is on the server,
    /// nothing runs against it and no box is open already.
    fn can_purge(&self, idle: bool) -> bool {
        let installed = self.facts.data.as_ref().map(|f| f.lanlord_installed);
        !self.read_only && installed == Some(true) && idle && self.purge.is_none()
    }

    /// the host whose purge just finished, once.
    pub fn take_purged(&mut self) -> Option<String> {
        self.purged.take()
    }

    /// is the purge box open (or running)?
    pub fn purging(&self) -> bool {
        self.purge.is_some()
    }

    /// open the box that asks before a purge.
    pub fn ask_to_purge(&mut self, services: &Services, host: &str) {
        if self.purge.is_none() {
            self.purge = Some(PurgeBox::new(host, services));
        }
    }

    /// pick up finished reads and start one when the window shows another server.
    pub fn refresh(&mut self, services: &Services, host: &str) {
        self.report = services.compat().report(host);
        self.checking = services.compat().busy();
        self.report_note = services.compat().note();
        self.facts.poll();
        if self.facts.wants_load(host, false) {
            let target = host.to_string();
            let job = services.spawn(host, move |gateway, ask| gateway.server_facts(&target, ask));
            self.facts
                .track(host, job, "reading the server...", "up to date", true);
        }
    }

    /// pick up the purge box's jobs; a finished purge closes it and is reported once.
    pub fn refresh_purge(&mut self) {
        let Some(purge) = self.purge.as_mut() else {
            return;
        };
        purge.preview.poll();
        if purge.running.poll() {
            for line in purge.running.data.take().unwrap_or_default() {
                emit(Kind::Info, line);
            }
            emit(
                Kind::Info,
                format!("lanlord was removed from {}", purge.host),
            );
            self.purged = self.purge.take().map(|p| p.host);
        }
    }

    /// the window's contents; `idle` when nothing runs against the server.
    pub fn view(&self, idle: bool) -> Vec<Node<Msg>> {
        let page = match self.tab {
            GENERAL => self.general(),
            COMPATIBILITY => self.compatibility(idle),
            _ => self.advanced(idle),
        };
        vec![
            Node::Tabs {
                titles: view::titles(&["General", "Advanced", "Compatibility"]),
                current: self.tab,
                pick: Msg::Tab,
                body: page,
            },
            Node::Gap(6.0),
            if self.tab == COMPATIBILITY {
                let (error, text) = self.report_note.clone().unwrap_or_default();
                Node::Status { text, error }
            } else {
                view::status(&self.facts)
            },
        ]
    }

    /// the Compatibility page: what the server is, each check with its mark and what needs it,
    /// and for the highlighted one the exact fix.
    fn compatibility(&self, idle: bool) -> Vec<Node<Msg>> {
        let recheck = Node::Button(Button::new(
            "Re-check",
            idle && !self.checking,
            Msg::Recheck,
        ));
        let Some(report) = &self.report else {
            let text = if self.checking {
                "Checking what this server can do..."
            } else {
                "This server has not been checked yet."
            };
            return vec![
                Node::Gap(4.0),
                Node::Text(text.into()),
                Node::Gap(4.0),
                recheck,
            ];
        };
        let what = report.environment.describe();
        let article = if what.starts_with(['a', 'e', 'i', 'o', 'u']) {
            "an"
        } else {
            "a"
        };
        let picked = self.picked.min(report.checks.len().saturating_sub(1));
        let rows = report
            .checks
            .iter()
            .map(|c| {
                vec![
                    mark(c.status).to_string(),
                    c.title.clone(),
                    compat::explain(&c.id),
                ]
            })
            .collect();
        let mut page = vec![
            Node::Text(format!("This server is {article} {what}.")),
            Node::Gap(4.0),
            Node::Table(view::Table {
                id: "compat-checks",
                headings: vec!["", "Check", "Needed for"],
                widths: vec![36.0, 150.0],
                rows,
                selected: Some(picked),
                bad: report
                    .checks
                    .iter()
                    .map(|c| c.status == Status::Fail)
                    .collect(),
                height: 150.0,
                select: Msg::Check,
            }),
            Node::Gap(4.0),
        ];
        if let Some(check) = report.checks.get(picked) {
            let mut body = vec![Node::Text(check.detail.clone())];
            if let Some(fix) = &check.fix {
                body.push(Node::Gap(4.0));
                body.push(Node::Text("To fix it (you can copy this):".into()));
                body.push(Node::Lines {
                    id: "compat-fix",
                    lines: fix.lines().map(str::to_string).collect(),
                    height: 90.0,
                });
                if report.environment.is_lxc() {
                    body.push(Node::Gap(4.0));
                    body.push(Node::Text(
                        "These changes are made on the Proxmox host, and the host must restart \
                         this container before they take effect. LANlord cannot do it from \
                         inside."
                            .into(),
                    ));
                }
            }
            page.push(Node::Group {
                title: check.title.clone(),
                body,
            });
            page.push(Node::Gap(4.0));
        }
        page.push(recheck);
        page
    }

    fn general(&self) -> Vec<Node<Msg>> {
        let Some(facts) = &self.facts.data else {
            return vec![Node::Text("Reading the server...".into())];
        };
        let lanlord = match (&facts.lanlord_installed, &facts.lanlord_version) {
            (false, _) => "Not installed".to_string(),
            (true, Some(version)) => format!("Installed, version {version}"),
            (true, None) => "Installed".to_string(),
        };
        let processor = if facts.cpu_model.is_empty() {
            "Unknown".to_string()
        } else {
            facts.cpu_model.clone()
        };
        let rows = [
            ("Server", facts.hostname.clone()),
            ("Signed in as", facts.user.clone()),
            ("System", facts.os.clone()),
            ("Kernel", facts.kernel.clone()),
            ("Up for", facts.uptime()),
            ("Processor", processor),
        ]
        .into_iter()
        .chain(facts.cpu_rows())
        .chain([("Memory", facts.memory()), ("LANlord", lanlord)]);
        let mut computer: Vec<Node<Msg>> = rows
            .map(|(label, value)| Node::field(label, value))
            .collect();
        if let Some(applied) = facts.last_applied_local() {
            computer.push(Node::field("Last applied", applied));
        }
        // the environment check (container, nesting, keyctl) joins these once core::env lands
        vec![Node::Group {
            title: "Computer".into(),
            body: computer,
        }]
    }

    fn advanced(&self, idle: bool) -> Vec<Node<Msg>> {
        let installed = self.facts.data.as_ref().map(|f| f.lanlord_installed);
        let ready = self.can_purge(idle);
        let mut body = vec![
            Node::Gap(4.0),
            Node::Text(
                "Removes the containers, files, service account and sudo rules that \
                 LANlord put on this server. Your own login stays."
                    .into(),
            ),
            Node::Gap(6.0),
            Node::Button(Button::new(
                "Remove LANlord from this server...",
                ready,
                Msg::AskPurge,
            )),
        ];
        if installed == Some(false) {
            body.push(Node::Gap(4.0));
            body.push(Node::Text(
                "LANlord is not installed on this server.".into(),
            ));
        }
        vec![Node::Danger(vec![Node::Group {
            title: "Danger zone".into(),
            body,
        }])]
    }

    /// the confirmation box, while a purge is being asked for or run.
    pub fn dialog(&self, services: &Services) -> Option<Dialog<Msg>> {
        let purge = self.purge.as_ref()?;
        let is_root = self.facts.data.as_ref().is_some_and(|f| f.is_root);
        let working = purge.running.busy();
        let mut body = vec![
            Node::Heading(format!("Remove LANlord from {}?", purge.host)),
            Node::Text("This cannot be undone. It would remove:".into()),
            Node::Gap(4.0),
            Node::Lines {
                id: "purge-lines",
                lines: purge.preview.data.clone().unwrap_or_default(),
                height: 150.0,
            },
            Node::Gap(4.0),
            Node::Check {
                label: "Also remove the administrator accounts".into(),
                checked: purge.remove_admins,
                enabled: is_root && !working && !purge.preview.busy(),
                toggle: Msg::ToggleAdmins,
            },
        ];
        if !is_root {
            body.push(Node::Text(
                "(Removing the accounts needs a root login.)".into(),
            ));
        }
        body.extend([
            Node::Gap(4.0),
            Node::Text(format!("Type {} to confirm:", purge.host)),
            Node::Input {
                value: purge.typed.clone(),
                size: Size::Points(240.0),
                hint: String::new(),
                focus: false,
                edit: Box::new(Msg::Typed),
                enter: None,
            },
            Node::Gap(6.0),
            Node::Buttons(vec![
                Button::new("Cancel", !working, Msg::Cancel),
                Button::new("Remove", purge.confirmed(services), Msg::Remove),
            ]),
        ]);
        let note = purge
            .running
            .note
            .clone()
            .or_else(|| purge.preview.note.clone());
        if let Some((error, text)) = note {
            body.push(Node::Status { text, error });
        }
        Some(Dialog {
            id: "purge-box",
            title: "Remove LANlord".into(),
            body,
            escape: None,
        })
    }

    /// act on a message. the rules the window shows are enforced here too, whatever the front
    /// end sends: no purge without the typed host name, no cancelling a purge that runs.
    pub fn update(&mut self, msg: Msg, services: &Services, host: &str) {
        match msg {
            Msg::Tab(tab) => {
                self.tab = tab;
                if tab == COMPATIBILITY && self.report.is_none() && !self.checking {
                    services.compat().recheck(services, host);
                }
            }
            Msg::Check(row) => self.picked = row,
            Msg::Recheck => services.compat().recheck(services, host),
            Msg::AskPurge => {
                if self.can_purge(!services.busy(host)) {
                    self.ask_to_purge(services, host);
                }
            }
            Msg::Cancel => {
                if !self.purge.as_ref().is_some_and(|p| p.running.busy()) {
                    self.purge = None;
                }
            }
            Msg::ToggleAdmins => {
                let is_root = self.facts.data.as_ref().is_some_and(|f| f.is_root);
                if let Some(purge) = self.purge.as_mut() {
                    if is_root && !purge.running.busy() && !purge.preview.busy() {
                        purge.remove_admins = !purge.remove_admins;
                        purge.look(services);
                    }
                }
            }
            Msg::Typed(text) => {
                if let Some(purge) = self.purge.as_mut() {
                    purge.typed = text;
                }
            }
            Msg::Remove => {
                if self.read_only {
                    return;
                }
                if let Some(purge) = self.purge.as_mut() {
                    if purge.confirmed(services) {
                        purge.remove(services);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use std::time::{Duration, Instant};

    use super::*;
    use crate::gateway::Fake;
    use crate::wake::Wake;

    /// the group titles and the label and value of every field on the General page.
    fn general_lines(window: &MyServer) -> (Vec<String>, Vec<(String, String)>) {
        let (mut titles, mut fields) = (Vec::new(), Vec::new());
        for node in window.general() {
            if let Node::Group { title, body } = node {
                titles.push(title);
                for node in body {
                    if let Node::Field { label, value } = node {
                        fields.push((label, value));
                    }
                }
            }
        }
        (titles, fields)
    }

    #[test]
    fn the_general_page_shows_the_processor_and_what_a_container_allows() {
        let mut window = MyServer::default();
        window.facts.data = Some(lanlord_model::server::demo());
        let (titles, fields) = general_lines(&window);
        let value = |label: &str| {
            fields
                .iter()
                .find(|(l, _)| l == label)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        // the interfaces moved to the Networks window
        assert_eq!(titles, ["Computer"]);
        assert_eq!(
            value("Processor"),
            "Intel(R) Core(TM) i7-4790K CPU @ 4.00GHz"
        );
        assert_eq!((value("Cores"), value("Threads")), ("2".into(), "4".into()));
        assert!(fields.iter().all(|(l, _)| l != "Processors"));
        assert_eq!(value("Memory"), "4096 MB, 3328 MB free");
        assert!(
            value("Last applied").ends_with('M'),
            "local time, not the raw text"
        );
        let facts = window.facts.data.as_mut().unwrap();
        (facts.cpus, facts.cpus_limited) = (2, true);
        (
            facts.mem_total_mb,
            facts.mem_available_mb,
            facts.mem_limited,
        ) = (1024, 512, true);
        facts.cpu_model.clear();
        let (_, fields) = general_lines(&window);
        let value = |label: &str| fields.iter().find(|(l, _)| l == label).unwrap().1.clone();
        assert_eq!(value("Threads"), "2 (limited by container)");
        assert!(fields
            .iter()
            .all(|(l, _)| l != "Cores" && l != "Processors"));
        assert_eq!(
            value("Memory"),
            "1024 MB, 512 MB free (limited by container)"
        );
        assert_eq!(value("Processor"), "Unknown");
    }

    fn until(window: &mut MyServer, done: impl Fn(&MyServer) -> bool) {
        let start = Instant::now();
        while !done(window) {
            window.refresh_purge();
            assert!(start.elapsed() < Duration::from_secs(5), "timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// a window whose purge box is open with its preview back.
    fn asked(fake: &Arc<Fake>, services: &Services) -> MyServer {
        let mut window = MyServer::default();
        window.refresh(services, "me@gw");
        let start = Instant::now();
        while !window.facts.belongs_to("me@gw") {
            window.refresh(services, "me@gw");
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(2));
        }
        window.update(Msg::AskPurge, services, "me@gw");
        until(&mut window, |w| {
            w.purge().is_some_and(|p| p.preview().is_some())
        });
        assert!(!services.busy("me@gw"));
        assert_eq!(fake.calls().len(), 2, "the facts and the preview");
        window
    }

    #[test]
    fn an_account_that_is_not_an_admin_cannot_ask_for_a_purge() {
        let fake = Arc::new(Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
        let mut window = MyServer::default();
        window.refresh(&services, "me@gw");
        let start = Instant::now();
        while !window.facts.belongs_to("me@gw") {
            window.refresh(&services, "me@gw");
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(2));
        }
        window.set_read_only(true);
        window.update(Msg::AskPurge, &services, "me@gw");
        assert!(window.purge().is_none());
        window.set_read_only(false);
        window.update(Msg::AskPurge, &services, "me@gw");
        assert!(window.purge().is_some());
    }

    #[test]
    fn an_account_that_is_not_an_admin_cannot_remove_lanlord() {
        let fake = Arc::new(Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
        let mut window = asked(&fake, &services);
        window.update(Msg::Typed("me@gw".into()), &services, "me@gw");
        // the box was up when the account lost its rights
        window.set_read_only(true);
        window.update(Msg::Remove, &services, "me@gw");
        assert!(!services.busy("me@gw"));
        assert!(!fake.calls().iter().any(|c| c.starts_with("purge me@gw")));
    }

    #[test]
    fn a_wrong_host_name_purges_nothing() {
        let fake = Arc::new(Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
        let mut window = asked(&fake, &services);
        window.update(Msg::Typed("me@gx".into()), &services, "me@gw");
        window.update(Msg::Remove, &services, "me@gw");
        assert!(!services.busy("me@gw"));
        assert!(!fake.calls().iter().any(|c| c.starts_with("purge me@gw")));
    }

    #[test]
    fn the_admins_toggle_waits_for_the_preview() {
        let fake = Arc::new(Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
        let mut window = asked(&fake, &services);
        window.update(Msg::ToggleAdmins, &services, "me@gw");
        assert!(window.purge().is_some_and(|p| p.remove_admins()));
        // the new preview is still running, so a second toggle is ignored
        window.update(Msg::ToggleAdmins, &services, "me@gw");
        assert!(window.purge().is_some_and(|p| p.remove_admins()));
    }

    #[test]
    fn a_running_purge_cannot_be_cancelled_and_still_reports() {
        let fake = Arc::new(Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
        let mut window = asked(&fake, &services);
        window.update(Msg::Typed("me@gw".into()), &services, "me@gw");
        window.update(Msg::Remove, &services, "me@gw");
        window.update(Msg::Cancel, &services, "me@gw");
        assert!(window.purging(), "the box stays while it runs");
        until(&mut window, |w| !w.purging());
        assert_eq!(window.take_purged().as_deref(), Some("me@gw"));
        assert!(fake
            .calls()
            .contains(&"purge me@gw admins=false".to_string()));
    }

    #[test]
    fn the_tabs_switch_pages_and_the_purge_box_follows_its_messages() {
        let fake = Arc::new(Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
        let tab_of = |window: &MyServer| match &window.view(true)[0] {
            Node::Tabs { current, .. } => *current,
            _ => panic!("the tabs come first"),
        };
        let mut window = MyServer::default();
        assert_eq!(tab_of(&window), GENERAL);
        // without the facts there is nothing to purge yet
        window.update(Msg::AskPurge, &services, "me@gw");
        assert!(!window.purging());
        window.update(Msg::Tab(1), &services, "me@gw");
        assert_eq!(tab_of(&window), 1);
        let mut window = asked(&fake, &services);
        window.update(Msg::Typed("me@gw".into()), &services, "me@gw");
        assert_eq!(window.purge().unwrap().typed(), "me@gw");
        let dialog = window.dialog(&services).expect("the box is open");
        assert_eq!(dialog.title, "Remove LANlord");
        window.update(Msg::Cancel, &services, "me@gw");
        assert!(!window.purging());
    }

    const UNPRIVILEGED: &str = "virt_container=lxc\nvirt_vm=none\nuid_map=0 100000 65536\npve_marker=1\nnesting=no\nkeyctl=yes\ntun=missing\nlink_bridge=ok\n";

    /// the page of the open tab.
    fn page(nodes: Vec<Node<Msg>>) -> Vec<Node<Msg>> {
        match nodes.into_iter().next() {
            Some(Node::Tabs { body, .. }) => body,
            _ => panic!("no tabs"),
        }
    }

    fn compat_window(probe: &str) -> (MyServer, Services, Arc<Fake>) {
        let fake = Arc::new(Fake::default());
        *fake.probe.lock().unwrap() = probe.into();
        let services = Services::new(fake.clone(), Wake::none());
        let mut window = MyServer::default();
        window.update(Msg::Tab(COMPATIBILITY), &services, "me@gw");
        let start = Instant::now();
        while window.report().is_none() {
            window.refresh(&services, "me@gw");
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(2));
        }
        (window, services, fake)
    }

    #[test]
    fn the_compatibility_tab_marks_each_check_and_shows_the_fix_of_the_picked_one() {
        let (mut window, services, _) = compat_window(UNPRIVILEGED);
        let body = page(window.view(true));
        assert!(
            matches!(&body[0], Node::Text(t) if t == "This server is an unprivileged lxc container on proxmox.")
        );
        let Some(Node::Table(table)) = body.iter().find(|n| matches!(n, Node::Table(_))) else {
            panic!("no table");
        };
        let row = |title: &str| {
            let at = table.rows.iter().position(|r| r[1] == title).unwrap();
            (table.rows[at].clone(), table.bad[at])
        };
        let (nesting, bad) = row("container nesting");
        assert_eq!((nesting[0].as_str(), bad), (mark(Status::Fail), true));
        assert!(nesting[2].contains("NT4") && nesting[2].contains("Network services"));
        assert_eq!(row("container keyctl").0[0], mark(Status::Ok));
        let (tun, bad) = row("tun/tap device");
        assert_eq!((tun[0].as_str(), bad), (mark(Status::Warn), false));
        assert!(tun[2].contains("VDE"));
        // a failing check shows the pct command in the copyable box
        let nesting_at = table
            .rows
            .iter()
            .position(|r| r[1] == "container nesting")
            .unwrap();
        window.update(Msg::Check(nesting_at), &services, "me@gw");
        let body = page(window.view(true));
        let fix = body
            .iter()
            .find_map(|n| match n {
                Node::Group { body, .. } => body.iter().find_map(|n| match n {
                    Node::Lines { lines, .. } => Some(lines.join("\n")),
                    _ => None,
                }),
                _ => None,
            })
            .expect("the fix box");
        assert!(fix.contains("pct set <CTID> --features nesting=1,keyctl=1"));
        // the tun fix gives the two config lines too
        let tun_at = table
            .rows
            .iter()
            .position(|r| r[1] == "tun/tap device")
            .unwrap();
        window.update(Msg::Check(tun_at), &services, "me@gw");
        let body = page(window.view(true));
        let text = body
            .iter()
            .filter_map(|n| match n {
                Node::Group { body, .. } => Some(
                    body.iter()
                        .filter_map(|n| match n {
                            Node::Lines { lines, .. } => Some(lines.join("\n")),
                            Node::Text(t) => Some(t.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("lxc.cgroup2.devices.allow: c 10:200 rwm"));
        assert!(text.contains("host must restart"));
    }

    #[test]
    fn recheck_reads_the_server_again() {
        let (mut window, services, fake) = compat_window(UNPRIVILEGED);
        *fake.probe.lock().unwrap() = "virt_container=none\nvirt_vm=none\n".into();
        window.update(Msg::Recheck, &services, "me@gw");
        let start = Instant::now();
        while !window.report().is_some_and(|r| !r.environment.is_lxc()) {
            window.refresh(&services, "me@gw");
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            fake.calls()
                .iter()
                .filter(|c| c.starts_with("doctor"))
                .count(),
            2
        );
    }
}
