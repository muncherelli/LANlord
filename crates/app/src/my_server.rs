//! the My Server window: what the signed-in server is (General), the one dangerous thing you can
//! do to it (Advanced): take lanlord back off, and how people sign in to it over ssh (SSH).

use lanlord_model::compat::{self, mark};
use lanlord_model::doctor::{Report, Status};
use lanlord_model::lan_networks::{self, LanNetwork};
use lanlord_model::operations::Operation;
use lanlord_model::rootssh::{RootSsh, RootSshInfo, DROPIN};
use lanlord_model::server::ServerFacts;
use lanlord_model::sshauth::SshAuthInfo;
use lanlord_model::validate;

use crate::log::{emit, Kind};

use crate::remote::Remote;
use crate::services::Services;
use crate::view::{self, Button, Dialog, Node, Size};

pub const GENERAL: usize = 0;
pub const ADVANCED: usize = 1;
pub const SSH: usize = 2;
pub const COMPATIBILITY: usize = 3;

/// what a click or a keystroke in the window means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    Tab(usize),
    /// highlight a check on the Compatibility tab.
    Check(usize),
    /// read the server's report again.
    Recheck,
    AskPurge,
    /// pick a level of root's ssh access, at this index of `RootSsh::ALL`; nothing is sent yet.
    PickRootSsh(usize),
    /// set root's ssh access to the picked level.
    ApplyRootSsh,
    /// the ssh password sign-in tick was clicked: turn it the other way.
    TogglePasswords,
    /// go ahead and turn passwords off although some account has no key.
    ConfirmPasswordsOff,
    /// pick the administrator account whose keys are listed, at this index.
    PickKeyUser(usize),
    /// pick a key of the listed account.
    PickKey(usize),
    /// open the box that takes a new public key.
    AskAddKey,
    /// the new key field now holds this text.
    KeyText(String),
    /// add the typed key to the picked account.
    AddKey,
    /// remove the picked key from the picked account.
    RemoveKey,
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

/// a box of the SSH tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SshDialog {
    /// turning passwords off would lock these accounts out: confirm or cancel.
    PasswordsOff(Vec<String>),
    /// type a public key for this account.
    AddKey(String),
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
    /// how root may sign in over ssh, as the server says now.
    ssh: Remote<RootSshInfo>,
    /// the level picked in the window and not applied yet.
    ssh_pick: Option<RootSsh>,
    /// the lanlord networks and what each allows over ssh, for the notice on the Advanced page.
    lan: Remote<Vec<LanNetwork>>,
    /// ssh password sign-in and the administrators' keys, as the server says now.
    auth: Remote<SshAuthInfo>,
    /// the administrator whose keys are listed, an index into the accounts.
    key_user: usize,
    /// the key picked in that list.
    key_pick: Option<usize>,
    /// the text of the key being added.
    key_text: String,
    /// the box open on the SSH tab, and the host it was opened for.
    ssh_dialog: Option<(String, SshDialog)>,
}

/// the height of the list of checks, and of the box holding the fix of the picked one.
const COMPAT_ROWS: f32 = 150.0;
const COMPAT_FIX: f32 = 54.0;

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

    /// how root's ssh access stands, once it is read.
    pub fn root_ssh(&self) -> &Remote<RootSshInfo> {
        &self.ssh
    }

    /// the level picked and not yet applied.
    pub fn root_ssh_pick(&self) -> Option<RootSsh> {
        self.ssh_pick
    }

    /// ssh sign-in as the server says it, once read.
    pub fn ssh_auth(&self) -> &Remote<SshAuthInfo> {
        &self.auth
    }

    /// the box open on the SSH tab.
    pub fn ssh_dialog(&self) -> Option<&SshDialog> {
        self.ssh_dialog.as_ref().map(|(_, d)| d)
    }

    /// the host the open box (purge or ssh) is about.
    pub fn dialog_host(&self) -> String {
        match (&self.purge, &self.ssh_dialog) {
            (Some(purge), _) => purge.host.clone(),
            (None, Some((host, _))) => host.clone(),
            _ => String::new(),
        }
    }

    /// the account whose keys are listed, if the server has any administrator.
    fn key_account(&self) -> Option<&lanlord_model::sshauth::AccountKeys> {
        let accounts = &self.auth.data.as_ref()?.accounts;
        accounts.get(self.key_user).or_else(|| accounts.first())
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
        self.ssh.poll();
        self.lan.poll();
        if self.auth.poll() {
            self.key_pick = None;
        }
        if self.lan.wants_load(host, false) {
            let target = host.to_string();
            let job = services.spawn(host, move |gateway, ask| {
                // no settings, or no key to read them, is no networks
                Ok(gateway
                    .load_settings(&target, ask)
                    .ok()
                    .and_then(|settings| lan_networks::read(&settings).ok())
                    .map(|networks| networks.list)
                    .unwrap_or_default())
            });
            self.lan
                .track(host, job, "reading the networks...", "up to date", true);
        }
        if self.ssh.wants_load(host, false) {
            let target = host.to_string();
            let job = services.spawn(host, move |gateway, _| gateway.root_ssh(&target));
            self.ssh.track(
                host,
                job,
                "reading root's ssh access...",
                "up to date",
                true,
            );
        }
        if self.facts.wants_load(host, false) {
            let target = host.to_string();
            let job = services.spawn(host, move |gateway, ask| gateway.server_facts(&target, ask));
            self.facts
                .track(host, job, "reading the server...", "up to date", true);
        }
        if !self.read_only && self.auth.wants_load(host, false) {
            let target = host.to_string();
            let job = services.spawn(host, move |gateway, ask| gateway.ssh_auth(&target, ask));
            self.auth
                .track(host, job, "reading ssh sign-in...", "up to date", true);
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
        let mut pages = vec![
            (GENERAL, self.general()),
            (COMPATIBILITY, self.compatibility(idle)),
            (SSH, self.ssh_page(idle)),
            (ADVANCED, self.advanced(idle)),
        ];
        let at = pages
            .iter()
            .position(|(tab, _)| *tab == self.tab)
            .unwrap_or(pages.len() - 1);
        let (_, page) = pages.remove(at);
        vec![
            Node::Tabs {
                key: view::pages_fingerprint(self.tab, &page, &pages),
                titles: view::titles(&["General", "Advanced", "SSH", "Compatibility"]),
                current: self.tab,
                pick: Msg::Tab,
                body: page,
                others: pages,
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
        // the page has the same parts, of the same sizes, with or without the report and whatever
        // is picked, so the window is the same size on every tab
        let Some(report) = &self.report else {
            let text = if self.checking {
                "Checking what this server can do..."
            } else {
                "This server has not been checked yet."
            };
            return Self::compat_page(text.into(), None, "Details", Vec::new(), None, recheck);
        };
        let what = report.environment.describe();
        let article = if what.starts_with(['a', 'e', 'i', 'o', 'u']) {
            "an"
        } else {
            "a"
        };
        let picked = self.picked.min(report.checks.len().saturating_sub(1));
        let table = Some(Node::Table(view::Table {
            id: "compat-checks",
            headings: vec!["", "Check", "Needed for"],
            widths: vec![36.0, 150.0],
            rows: report
                .checks
                .iter()
                .map(|c| {
                    vec![
                        mark(c.status).to_string(),
                        c.title.clone(),
                        compat::explain(&c.id),
                    ]
                })
                .collect(),
            selected: Some(picked),
            bad: report
                .checks
                .iter()
                .map(|c| c.status == Status::Fail)
                .collect(),
            height: COMPAT_ROWS,
            select: Msg::Check,
        }));
        let (title, mut detail, fix) = match report.checks.get(picked) {
            Some(c) => (
                c.title.as_str(),
                vec![Node::Text(c.detail.clone())],
                c.fix.as_deref(),
            ),
            None => ("Details", Vec::new(), None),
        };
        if fix.is_some() && report.environment.is_lxc() {
            detail.push(Node::Text(
                "These changes are made on the Proxmox host, and the host must restart this \
                 container before they take effect. LANlord cannot do it from inside."
                    .into(),
            ));
        }
        Self::compat_page(
            format!("This server is {article} {what}."),
            table,
            title,
            detail,
            fix,
            recheck,
        )
    }

    /// the Compatibility page: what the server is, the list of checks, the details of the picked
    /// one in a box of 3 rows, and its fix in a box of its own.
    fn compat_page(
        intro: String,
        table: Option<Node<Msg>>,
        title: &str,
        detail: Vec<Node<Msg>>,
        fix: Option<&str>,
        recheck: Node<Msg>,
    ) -> Vec<Node<Msg>> {
        let table = table.unwrap_or_else(|| {
            Node::Table(view::Table {
                id: "compat-checks",
                headings: vec!["", "Check", "Needed for"],
                widths: vec![36.0, 150.0],
                rows: Vec::new(),
                selected: None,
                bad: Vec::new(),
                height: COMPAT_ROWS,
                select: Msg::Check,
            })
        });
        vec![
            Node::Text(intro),
            Node::Gap(4.0),
            table,
            Node::Gap(4.0),
            Node::Details {
                title: title.into(),
                lines: 3,
                body: detail,
            },
            Node::Gap(4.0),
            Node::Text(if fix.is_some() {
                "To fix it (you can copy this):".into()
            } else {
                " ".into()
            }),
            Node::Lines {
                id: "compat-fix",
                lines: fix
                    .map(|f| f.lines().map(str::to_string).collect())
                    .unwrap_or_default(),
                height: COMPAT_FIX,
                enabled: fix.is_some(),
            },
            Node::Gap(4.0),
            recheck,
        ]
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

    /// the SSH page: whether passwords are taken, the administrators' keys, root's access and
    /// what the firewall does to ssh.
    fn ssh_page(&self, idle: bool) -> Vec<Node<Msg>> {
        let mut page = vec![
            self.passwords_group(idle),
            Node::Gap(6.0),
            self.keys_group(idle),
            Node::Gap(6.0),
            self.root_ssh_group(idle),
            Node::Gap(6.0),
        ];
        let now = crate::networks_window::unix_now();
        if let Some(notice) = self
            .lan
            .data
            .as_deref()
            .and_then(|list| ssh_networks_notice(list, now))
        {
            page.push(notice);
        }
        page
    }

    /// whether ssh takes passwords: one tick, and the accounts that have no key to fall back on.
    fn passwords_group(&self, idle: bool) -> Node<Msg> {
        let info = self.auth.data.as_ref();
        let on = info.and_then(|i| i.passwords.or(i.chosen)).unwrap_or(true);
        let usable =
            !self.read_only && idle && !self.auth.busy() && info.is_some_and(|i| i.dropins);
        let mut body = vec![
            Node::Gap(4.0),
            Node::Text(
                "Whether people may sign in to this server over ssh with a password. Keys \
                 are safer; turn passwords off once every administrator has a key. Until \
                 you choose, LANlord leaves the ssh daemon as it is."
                    .into(),
            ),
            Node::Gap(4.0),
            Node::field(
                "Now",
                info.map_or("Reading...", |i| match (i.chosen, i.passwords) {
                    (Some(true), _) => "Passwords on (set by LANlord)",
                    (Some(false), _) => "Passwords off (set by LANlord)",
                    (None, Some(true)) => "Passwords on (not managed by LANlord)",
                    (None, Some(false)) => "Passwords off (not managed by LANlord)",
                    (None, None) => "Not managed by LANlord",
                }),
            ),
            Node::Check {
                label: "Allow signing in to ssh with a password".into(),
                checked: on,
                enabled: usable,
                toggle: Msg::TogglePasswords,
            },
        ];
        if self.read_only {
            body.push(Node::Text("Only an administrator can change this.".into()));
        }
        if info.is_some_and(|i| !i.dropins) {
            body.push(Node::Alert(format!(
                "This server's sshd does not read {}, so password sign-in cannot be set here.",
                DROPIN_DIR
            )));
        }
        if let Some((error, text)) = self.auth.note.clone() {
            body.push(Node::Gap(4.0));
            body.push(Node::Status { text, error });
        }
        Node::Group {
            title: "Password sign-in".into(),
            body,
        }
    }

    /// the public keys of the administrator accounts, with Add and Remove.
    fn keys_group(&self, idle: bool) -> Node<Msg> {
        let account = self.key_account();
        let can = !self.read_only && idle && !self.auth.busy() && account.is_some();
        let names: Vec<(usize, String)> = self
            .auth
            .data
            .as_ref()
            .map(|i| {
                i.accounts
                    .iter()
                    .map(|a| a.user.clone())
                    .enumerate()
                    .collect()
            })
            .unwrap_or_default();
        let shown = account.map_or("no administrators".to_string(), |a| a.user.clone());
        let keys: Vec<String> = account
            .map(|a| a.keys.iter().map(|k| key_label(k)).collect())
            .unwrap_or_default();
        let body = vec![
            Node::Gap(4.0),
            Node::Text(
                "The ssh public keys that let an administrator sign in without a password. \
                 Keys LANlord adds are marked in the file; the others are listed too."
                    .into(),
            ),
            Node::Gap(4.0),
            Node::Labelled {
                label: "Account:".into(),
                body: vec![Node::Choice {
                    id: "key-user",
                    current: shown,
                    width: 220.0,
                    options: names,
                    pick: Msg::PickKeyUser,
                }],
            },
            Node::List {
                id: "admin-keys",
                items: keys,
                selected: self.key_pick,
                min_height: 70.0,
                empty: "No ssh key for this account.".into(),
                select: Msg::PickKey,
            },
            Node::Toolbar(vec![
                Button::new("Add key...", can, Msg::AskAddKey),
                Button::new("Remove key", can && self.key_pick.is_some(), Msg::RemoveKey),
            ]),
        ];
        Node::Group {
            title: "Administrator ssh keys".into(),
            body,
        }
    }

    /// whether root may sign in to the server over ssh: the setting, its three levels, and what
    /// each one means. an account that is not an administrator sees it and cannot change it.
    fn root_ssh_group(&self, idle: bool) -> Node<Msg> {
        let info = self.ssh.data.as_ref();
        let chosen = info.and_then(|i| i.chosen);
        let shown = self.ssh_pick.or(chosen);
        let readable = info.is_some_and(|i| i.dropins);
        let mut body = vec![
            Node::Gap(4.0),
            Node::Text(
                "Whether root may sign in to this server over ssh. Until you choose, \
                 LANlord leaves the ssh daemon as it is."
                    .into(),
            ),
            Node::Gap(4.0),
            Node::field(
                "Now",
                chosen.map_or("Not managed by LANlord", RootSsh::label),
            ),
            Node::Gap(4.0),
            Node::Labelled {
                label: "Root may sign in:".into(),
                body: vec![Node::Choice {
                    id: "root-ssh",
                    current: shown.map_or("choose a level", RootSsh::label).into(),
                    width: 220.0,
                    options: RootSsh::ALL
                        .iter()
                        .enumerate()
                        .map(|(i, level)| (i, level.label().to_string()))
                        .collect(),
                    pick: Msg::PickRootSsh,
                }],
            },
        ];
        if self.read_only {
            // only an administrator changes it: the choice gives way to a line saying so
            body.pop();
            body.push(Node::Text("Only an administrator can change this.".into()));
        }
        if let Some(level) = shown {
            body.push(Node::Gap(4.0));
            body.push(Node::Text(level.explain().to_string()));
        }
        if info.is_some() && !readable {
            body.push(Node::Gap(4.0));
            body.push(Node::Alert(format!(
                "This server's sshd does not read {}, so root's ssh access cannot be set here.",
                DROPIN.rsplit_once('/').map_or(DROPIN, |(dir, _)| dir)
            )));
        }
        let ready = !self.read_only
            && idle
            && readable
            && !self.ssh.busy()
            && self.ssh_pick.is_some_and(|p| Some(p) != chosen);
        body.push(Node::Gap(6.0));
        body.push(Node::Button(Button::new("Apply", ready, Msg::ApplyRootSsh)));
        if let Some((error, text)) = self.ssh.note.clone() {
            body.push(Node::Gap(4.0));
            body.push(Node::Status { text, error });
        }
        Node::Group {
            title: "Root ssh access".into(),
            body,
        }
    }

    /// the box over the window: a purge being asked for or run, or one of the SSH tab's.
    pub fn dialog(&self, services: &Services) -> Option<Dialog<Msg>> {
        if self.purge.is_none() {
            return self.ssh_box();
        }
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
                enabled: true,
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

    /// the SSH tab's boxes: the lock-out warning and the new key.
    fn ssh_box(&self) -> Option<Dialog<Msg>> {
        let (_, dialog) = self.ssh_dialog.as_ref()?;
        Some(match dialog {
            SshDialog::PasswordsOff(without) => Dialog {
                id: "ssh-passwords-box",
                title: "Turn off ssh passwords".into(),
                body: vec![
                    Node::Heading("Some accounts have no ssh key".into()),
                    Node::Text(format!(
                        "With passwords off, {} could no longer sign in over ssh.",
                        without.join(", ")
                    )),
                    Node::Text(
                        "Add a key for each first, or go on only if you can still reach this \
                         server another way."
                            .into(),
                    ),
                    Node::Gap(6.0),
                    Node::Buttons(vec![
                        Button::new("Cancel", true, Msg::Cancel),
                        Button::new("Turn off anyway", true, Msg::ConfirmPasswordsOff),
                    ]),
                ],
                escape: Some(Msg::Cancel),
            },
            SshDialog::AddKey(user) => Dialog {
                id: "ssh-key-box",
                title: "Add ssh key".into(),
                body: vec![
                    Node::Heading(format!("Add a key for {user}")),
                    Node::Text(
                        "Paste one public key, such as the line of an id_ed25519.pub file:".into(),
                    ),
                    Node::Input {
                        value: self.key_text.clone(),
                        size: Size::Points(360.0),
                        hint: "ssh-ed25519 AAAA... name".into(),
                        focus: true,
                        edit: Box::new(Msg::KeyText),
                        enter: None,
                    },
                    Node::Gap(6.0),
                    Node::Buttons(vec![
                        Button::new("Cancel", true, Msg::Cancel),
                        Button::new("Add", self.key_text_ok(user), Msg::AddKey),
                    ]),
                ],
                escape: Some(Msg::Cancel),
            },
        })
    }

    /// is the typed key one the server would take?
    fn key_text_ok(&self, user: &str) -> bool {
        validate::operation(&Operation::AddAdminKey {
            user: user.to_string(),
            key: self.key_text.trim().to_string(),
        })
        .is_ok()
    }

    /// set the password switch and read the state back.
    fn set_passwords(&mut self, services: &Services, host: &str, passwords: bool, confirmed: bool) {
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| {
            gateway.set_password_login(&target, passwords, confirmed, ask)?;
            gateway.ssh_auth(&target, ask)
        });
        self.auth.track(
            host,
            job,
            "setting ssh password sign-in...",
            if passwords {
                "ssh passwords are on"
            } else {
                "ssh passwords are off"
            },
            false,
        );
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
            Msg::TogglePasswords => {
                let Some(info) = self.auth.data.as_ref() else {
                    return;
                };
                if self.read_only || !info.dropins || self.auth.busy() {
                    return;
                }
                let want = !info.passwords.or(info.chosen).unwrap_or(true);
                match info.refusal().filter(|_| !want) {
                    // would lock somebody out: ask first; the server refuses on its own too
                    Some(_) => {
                        self.ssh_dialog = Some((
                            host.to_string(),
                            SshDialog::PasswordsOff(info.without_key()),
                        ));
                    }
                    None => self.set_passwords(services, host, want, false),
                }
            }
            Msg::ConfirmPasswordsOff => {
                if let Some((_, SshDialog::PasswordsOff(_))) = self.ssh_dialog.take() {
                    if !self.read_only && !self.auth.busy() {
                        self.set_passwords(services, host, false, true);
                    }
                }
            }
            Msg::PickKeyUser(i) => {
                self.key_user = i;
                self.key_pick = None;
            }
            Msg::PickKey(i) => {
                if self.key_account().is_some_and(|a| i < a.keys.len()) {
                    self.key_pick = Some(i);
                }
            }
            Msg::AskAddKey => {
                if let (false, Some(account)) = (self.read_only, self.key_account()) {
                    let user = account.user.clone();
                    self.key_text.clear();
                    self.ssh_dialog = Some((host.to_string(), SshDialog::AddKey(user)));
                }
            }
            Msg::KeyText(text) => self.key_text = text,
            Msg::AddKey => {
                let Some((_, SshDialog::AddKey(user))) = self.ssh_dialog.clone() else {
                    return;
                };
                if self.read_only || self.auth.busy() || !self.key_text_ok(&user) {
                    return;
                }
                let (target, key) = (host.to_string(), self.key_text.trim().to_string());
                let job = services.spawn(host, move |gateway, ask| {
                    gateway.add_admin_key(&target, &user, &key, ask)?;
                    gateway.ssh_auth(&target, ask)
                });
                if self
                    .auth
                    .track(host, job, "adding the key...", "the key was added", false)
                {
                    self.ssh_dialog = None;
                    self.key_text.clear();
                }
            }
            Msg::RemoveKey => {
                let (Some(account), Some(pick)) = (self.key_account(), self.key_pick) else {
                    return;
                };
                let Some(key) = account.keys.get(pick).cloned() else {
                    return;
                };
                let user = account.user.clone();
                if self.read_only || self.auth.busy() {
                    return;
                }
                let target = host.to_string();
                let job = services.spawn(host, move |gateway, ask| {
                    gateway.remove_admin_key(&target, &user, &key, ask)?;
                    gateway.ssh_auth(&target, ask)
                });
                self.auth.track(
                    host,
                    job,
                    "removing the key...",
                    "the key was removed",
                    false,
                );
            }
            Msg::PickRootSsh(i) => {
                if !self.read_only {
                    self.ssh_pick = RootSsh::ALL.get(i).copied();
                }
            }
            Msg::ApplyRootSsh => {
                let Some(level) = self.ssh_pick else {
                    return;
                };
                let readable = self.ssh.data.as_ref().is_some_and(|i| i.dropins);
                if self.read_only || !readable || self.ssh.busy() {
                    return;
                }
                let target = host.to_string();
                let job = services.spawn(host, move |gateway, ask| {
                    gateway.set_root_ssh(&target, level, ask)?;
                    gateway.root_ssh(&target)
                });
                if self.ssh.track(
                    host,
                    job,
                    "setting root's ssh access...",
                    "root's ssh access is set",
                    false,
                ) {
                    self.ssh_pick = None;
                }
            }
            Msg::Cancel => {
                self.ssh_dialog = None;
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

/// the folder sshd reads its drop-ins from, for the messages.
const DROPIN_DIR: &str = "/etc/ssh/sshd_config.d";

/// a key line for the list: its type, the end of the key (enough to tell keys apart) and its
/// comment.
fn key_label(line: &str) -> String {
    let words: Vec<&str> = line.split_whitespace().collect();
    let at = words.iter().position(|w| {
        ["ssh-", "ecdsa-", "sk-ssh-", "sk-ecdsa-"]
            .iter()
            .any(|p| w.starts_with(p))
    });
    let Some(at) = at else {
        return line.to_string();
    };
    let kind = words[at];
    let data = words.get(at + 1).copied().unwrap_or("");
    let tail: String = data
        .chars()
        .rev()
        .take(12)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let comment = words.get(at + 2..).map(|c| c.join(" ")).unwrap_or_default();
    format!("{kind} ...{tail} {comment}").trim_end().to_string()
}

/// what the firewall does to ssh from the LANlord networks, as the settings have it, and where
/// to change it. none when there is no network with ssh off: nothing is blocked then. a computer
/// whose grant has ended at `now` is not counted.
fn ssh_networks_notice(list: &[LanNetwork], now: i64) -> Option<Node<Msg>> {
    let off: Vec<&str> = list
        .iter()
        .filter(|n| !n.ssh.enabled)
        .map(|n| n.name.as_str())
        .collect();
    if off.is_empty() {
        return None;
    }
    let on: Vec<&str> = list
        .iter()
        .filter(|n| n.ssh.enabled)
        .map(|n| n.name.as_str())
        .collect();
    let computers: Vec<String> = list
        .iter()
        .filter(|n| !n.ssh.enabled)
        .filter_map(|n| {
            let count = n.ssh.live(now).count();
            (count > 0).then(|| {
                format!(
                    "{} ({count} computer{})",
                    n.name,
                    if count == 1 { "" } else { "s" }
                )
            })
        })
        .collect();
    let mut lines = vec![format!(
        "Ssh to this server is off from: {}.",
        off.join(", ")
    )];
    if !on.is_empty() {
        lines.push(format!(
            "Allowed from the whole network: {}.",
            on.join(", ")
        ));
    }
    if !computers.is_empty() {
        lines.push(format!(
            "Allowed for single computers on: {}.",
            computers.join(", ")
        ));
    }
    lines.push(
        "To change it, use \"Allow ssh to this server\" on a network in the Networks window."
            .into(),
    );
    let mut body = vec![Node::Gap(4.0)];
    body.extend(lines.into_iter().map(Node::Text));
    Some(Node::Group {
        title: "Ssh from LANlord networks".into(),
        body,
    })
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
        assert_eq!(
            fake.calls().len(),
            4,
            "the facts, the networks, ssh sign-in and the preview"
        );
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
            // the box of details shows the picked check, as a plain group
            Some(Node::Tabs { body, .. }) => body
                .into_iter()
                .map(|n| match n {
                    Node::Details { title, body, .. } => Node::Group { title, body },
                    n => n,
                })
                .collect(),
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
                Node::Lines { lines, .. } => Some(lines.join("\n")),
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
                Node::Lines { lines, .. } => Some(lines.join("\n")),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("lxc.cgroup2.devices.allow: c 10:200 rwm"));
        assert!(text.contains("host must restart"));
    }

    #[test]
    fn the_fix_box_is_disabled_unless_the_picked_check_has_a_fix() {
        let (mut window, services, _) = compat_window(UNPRIVILEGED);
        let fix_box = |window: &MyServer| {
            page(window.view(true))
                .into_iter()
                .find_map(|n| match n {
                    Node::Lines { enabled, lines, .. } => Some((enabled, lines.len())),
                    _ => None,
                })
                .expect("the fix box")
        };
        let checks = window.report().expect("a report").checks.clone();
        let with = checks.iter().position(|c| c.fix.is_some()).expect("a fix");
        let without = checks.iter().position(|c| c.fix.is_none()).expect("no fix");
        window.update(Msg::Check(with), &services, "me@gw");
        let (enabled, lines) = fix_box(&window);
        assert!(enabled && lines > 0);
        window.update(Msg::Check(without), &services, "me@gw");
        assert_eq!(fix_box(&window), (false, 0));
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

    /// every text a node tree shows, flattened.
    fn texts(nodes: &[Node<Msg>], out: &mut Vec<String>) {
        for node in nodes {
            match node {
                Node::Text(t) | Node::Alert(t) => out.push(t.clone()),
                Node::Group { body, .. }
                | Node::Danger(body)
                | Node::Disabled(body)
                | Node::Labelled { body, .. } => texts(body, out),
                Node::Choice { current, .. } => out.push(current.clone()),
                Node::Field { value, .. } => out.push(value.clone()),
                _ => {}
            }
        }
    }

    fn apply_enabled(nodes: &[Node<Msg>]) -> Option<bool> {
        nodes.iter().find_map(|node| match node {
            Node::Group { body, .. } | Node::Danger(body) => apply_enabled(body),
            Node::Button(b) if b.label == "Apply" => Some(b.enabled),
            _ => None,
        })
    }

    /// whether the root ssh choice is shown and not greyed out; None when it is not shown.
    fn root_choice_enabled(nodes: &[Node<Msg>], disabled: bool) -> Option<bool> {
        nodes.iter().find_map(|node| match node {
            Node::Choice { id: "root-ssh", .. } => Some(!disabled),
            Node::Group { body, .. } | Node::Danger(body) | Node::Labelled { body, .. } => {
                root_choice_enabled(body, disabled)
            }
            Node::Disabled(body) => root_choice_enabled(body, true),
            _ => None,
        })
    }

    fn settle(window: &mut MyServer, services: &Services) {
        let start = Instant::now();
        loop {
            window.refresh(services, "me@gw");
            if window.ssh.data.is_some() && !window.ssh.busy() && !window.facts.busy() {
                return;
            }
            assert!(start.elapsed() < Duration::from_secs(5), "timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn root_ssh_access_is_read_picked_and_applied_by_an_administrator() {
        let fake = Arc::new(Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
        let mut window = MyServer::default();
        settle(&mut window, &services);
        let mut seen = Vec::new();
        texts(&window.ssh_page(true), &mut seen);
        assert!(
            seen.iter().any(|t| t == "Not managed by LANlord"),
            "{seen:?}"
        );
        // an administrator can use the choice
        assert_eq!(
            root_choice_enabled(&window.ssh_page(true), false),
            Some(true)
        );
        // nothing picked yet: nothing to apply
        assert_eq!(apply_enabled(&window.ssh_page(true)), Some(false));
        window.update(Msg::PickRootSsh(2), &services, "me@gw");
        assert_eq!(window.root_ssh_pick(), Some(RootSsh::Off));
        let mut seen = Vec::new();
        texts(&window.ssh_page(true), &mut seen);
        assert!(
            seen.iter().any(|t| t.contains("LANlord clients")),
            "the warning about clients that connect as root: {seen:?}"
        );
        assert_eq!(apply_enabled(&window.ssh_page(true)), Some(true));
        window.update(Msg::ApplyRootSsh, &services, "me@gw");
        let start = Instant::now();
        while window.ssh.busy() {
            window.refresh(&services, "me@gw");
            assert!(start.elapsed() < Duration::from_secs(5), "timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(fake.calls().contains(&"set-root-ssh me@gw no".to_string()));
        assert_eq!(window.ssh.data.as_ref().unwrap().chosen, Some(RootSsh::Off));
        assert_eq!(window.root_ssh_pick(), None);
    }

    #[test]
    fn root_ssh_access_cannot_be_changed_by_a_read_only_window() {
        let fake = Arc::new(Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
        let mut window = MyServer::default();
        window.set_read_only(true);
        settle(&mut window, &services);
        window.update(Msg::PickRootSsh(0), &services, "me@gw");
        window.update(Msg::ApplyRootSsh, &services, "me@gw");
        assert_eq!(window.root_ssh_pick(), None);
        assert_eq!(apply_enabled(&window.ssh_page(true)), Some(false));
        assert_eq!(root_choice_enabled(&window.ssh_page(true), false), None);
        assert!(fake.calls().iter().all(|c| !c.starts_with("set-root-ssh")));
    }

    #[test]
    fn a_server_whose_sshd_ignores_the_folder_says_so_and_offers_no_apply() {
        let fake = Arc::new(Fake::new());
        fake.root_ssh.lock().unwrap().dropins = false;
        let services = Services::new(fake.clone(), Wake::none());
        let mut window = MyServer::default();
        settle(&mut window, &services);
        window.update(Msg::PickRootSsh(1), &services, "me@gw");
        let mut seen = Vec::new();
        texts(&window.ssh_page(true), &mut seen);
        assert!(
            seen.iter()
                .any(|t| t.contains("does not read /etc/ssh/sshd_config.d")),
            "{seen:?}"
        );
        assert_eq!(apply_enabled(&window.ssh_page(true)), Some(false));
        window.update(Msg::ApplyRootSsh, &services, "me@gw");
        assert!(fake.calls().iter().all(|c| !c.starts_with("set-root-ssh")));
    }

    /// a window that has read ssh sign-in.
    fn settled_auth(fake: &Arc<Fake>) -> (MyServer, Services) {
        let services = Services::new(fake.clone(), Wake::none());
        let mut window = MyServer::default();
        settle(&mut window, &services);
        let start = Instant::now();
        while window.auth.data.is_none() || window.auth.busy() {
            window.refresh(&services, "me@gw");
            assert!(start.elapsed() < Duration::from_secs(5), "timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
        (window, services)
    }

    fn wait_auth(window: &mut MyServer, services: &Services) {
        let start = Instant::now();
        while window.auth.busy() {
            window.refresh(services, "me@gw");
            assert!(start.elapsed() < Duration::from_secs(5), "timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn turning_passwords_off_asks_first_while_an_administrator_has_no_key() {
        let fake = Arc::new(Fake::new());
        let (mut window, services) = settled_auth(&fake);
        // the demo's pat has no key
        window.update(Msg::TogglePasswords, &services, "me@gw");
        assert_eq!(
            window.ssh_dialog(),
            Some(&SshDialog::PasswordsOff(vec!["pat".into()]))
        );
        assert_eq!(window.dialog_host(), "me@gw");
        assert!(window.dialog(&services).is_some());
        assert!(fake
            .calls()
            .iter()
            .all(|c| !c.starts_with("password-login")));
        // cancelling sends nothing
        window.update(Msg::Cancel, &services, "me@gw");
        assert!(window.ssh_dialog().is_none());
        // going on sends it confirmed
        window.update(Msg::TogglePasswords, &services, "me@gw");
        window.update(Msg::ConfirmPasswordsOff, &services, "me@gw");
        wait_auth(&mut window, &services);
        assert!(fake
            .calls()
            .contains(&"password-login me@gw false true".to_string()));
        assert_eq!(window.auth.data.as_ref().unwrap().passwords, Some(false));
        // and on again needs no question
        window.update(Msg::TogglePasswords, &services, "me@gw");
        assert!(window.ssh_dialog().is_none());
        wait_auth(&mut window, &services);
        assert_eq!(window.auth.data.as_ref().unwrap().passwords, Some(true));
    }

    #[test]
    fn with_every_administrator_keyed_passwords_go_off_without_a_question() {
        let fake = Arc::new(Fake::new());
        fake.ssh_auth.lock().unwrap().accounts[1]
            .keys
            .push("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPatKey pat@desk".into());
        let (mut window, services) = settled_auth(&fake);
        window.update(Msg::TogglePasswords, &services, "me@gw");
        assert!(window.ssh_dialog().is_none());
        wait_auth(&mut window, &services);
        assert!(fake
            .calls()
            .contains(&"password-login me@gw false false".to_string()));
        assert_eq!(window.auth.data.as_ref().unwrap().chosen, Some(false));
    }

    #[test]
    fn keys_are_listed_per_account_added_and_removed() {
        let fake = Arc::new(Fake::new());
        let (mut window, services) = settled_auth(&fake);
        assert_eq!(window.key_account().unwrap().user, "dana");
        window.update(Msg::PickKeyUser(1), &services, "me@gw");
        assert_eq!(window.key_account().unwrap().user, "pat");
        // a line that is not a key cannot be added
        window.update(Msg::AskAddKey, &services, "me@gw");
        assert_eq!(window.ssh_dialog(), Some(&SshDialog::AddKey("pat".into())));
        window.update(Msg::KeyText("not a key".into()), &services, "me@gw");
        window.update(Msg::AddKey, &services, "me@gw");
        assert!(fake.calls().iter().all(|c| !c.starts_with("add-admin-key")));
        let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPatKey pat@desk";
        window.update(Msg::KeyText(format!("  {key}  ")), &services, "me@gw");
        window.update(Msg::AddKey, &services, "me@gw");
        wait_auth(&mut window, &services);
        assert!(window.ssh_dialog().is_none());
        assert_eq!(window.key_account().unwrap().keys, [key]);
        // pat has a key now: the question is gone
        window.update(Msg::TogglePasswords, &services, "me@gw");
        assert!(window.ssh_dialog().is_none());
        wait_auth(&mut window, &services);
        window.update(Msg::PickKey(0), &services, "me@gw");
        window.update(Msg::RemoveKey, &services, "me@gw");
        wait_auth(&mut window, &services);
        assert!(window.key_account().unwrap().keys.is_empty());
        assert!(fake
            .calls()
            .contains(&"remove-admin-key me@gw pat".to_string()));
    }

    #[test]
    fn a_read_only_window_changes_no_ssh_sign_in() {
        let fake = Arc::new(Fake::new());
        let (mut window, services) = settled_auth(&fake);
        window.set_read_only(true);
        for msg in [Msg::TogglePasswords, Msg::AskAddKey, Msg::RemoveKey] {
            window.update(msg, &services, "me@gw");
        }
        assert!(window.ssh_dialog().is_none());
        assert!(fake
            .calls()
            .iter()
            .all(|c| { !c.starts_with("password-login") && !c.contains("admin-key") }));
    }

    #[test]
    fn the_key_list_shows_a_short_label_for_each_key() {
        assert_eq!(
            key_label("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIDemoKeyOne dana@laptop"),
            "ssh-ed25519 ...AIDemoKeyOne dana@laptop"
        );
        assert_eq!(
            key_label("no-pty ssh-rsa AAAAB3Nza"),
            "ssh-rsa ...AAAAB3Nza"
        );
        assert_eq!(key_label("odd line"), "odd line");
    }

    fn networks(text: &str) -> Vec<LanNetwork> {
        lan_networks::read(&lanlord_model::settings::Settings::parse(text).unwrap())
            .unwrap()
            .list
    }

    const NOW: i64 = 1_900_000_000;

    #[test]
    fn the_ssh_notice_counts_only_computers_whose_grant_has_not_ended() {
        let list = networks(
            "networks:\n  - {name: lab, interface: eth1, address: 192.0.2.1/24, ssh_allow: [{mac: \"02:00:00:00:00:01\"}, {mac: \"02:00:00:00:00:02\", until: \"2040-01-01T00:00:00Z\"}, {mac: \"02:00:00:00:00:03\", until: \"2020-01-01T00:00:00Z\"}]}\n  - {name: old, interface: eth2, address: 198.51.100.1/24, ssh_allow: [{mac: \"02:00:00:00:00:04\", until: \"2020-01-01T00:00:00Z\"}]}\n",
        );
        let lines = notice_lines(ssh_networks_notice(&list, NOW).unwrap());
        assert_eq!(
            lines[1],
            "Allowed for single computers on: lab (2 computers)."
        );
        // after the later grant ends too, one computer is left of lab and none is named for old
        let later = notice_lines(ssh_networks_notice(&list, 2_300_000_000).unwrap());
        assert_eq!(
            later[1],
            "Allowed for single computers on: lab (1 computer)."
        );
    }

    fn notice_lines(node: Node<Msg>) -> Vec<String> {
        let Node::Group { body, .. } = node else {
            panic!("a group");
        };
        body.into_iter()
            .filter_map(|n| match n {
                Node::Text(t) => Some(t),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_ssh_notice_names_the_networks_it_is_off_for_and_what_is_allowed() {
        let list = networks(
            "networks:\n  - {name: lab, interface: eth1, address: 192.0.2.1/24, ssh_allow: [{mac: \"02:00:00:00:00:01\"}]}\n  - {name: dmz, interface: eth2, address: 198.51.100.1/24}\n  - {name: office, interface: eth3, address: 203.0.113.1/24, ssh: true}\n",
        );
        let lines = notice_lines(ssh_networks_notice(&list, NOW).unwrap());
        assert_eq!(lines[0], "Ssh to this server is off from: lab, dmz.");
        assert!(lines[1].contains("office"), "{lines:?}");
        assert!(lines[2].contains("lab (1 computer)"), "{lines:?}");
        assert!(lines[3].contains("Networks window"), "{lines:?}");
    }

    #[test]
    fn the_ssh_notice_is_not_shown_when_nothing_is_blocked() {
        assert!(ssh_networks_notice(&[], NOW).is_none());
        let on = networks(
            "networks:\n  - {name: lab, interface: eth1, address: 192.0.2.1/24, ssh: true}\n",
        );
        assert!(ssh_networks_notice(&on, NOW).is_none());
    }

    #[test]
    fn the_ssh_page_shows_the_notice_once_the_networks_are_read() {
        let fake = Arc::new(Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
        let mut window = MyServer::default();
        let has_notice = |window: &MyServer| {
            window.ssh_page(true).iter().any(
                |n| matches!(n, Node::Group { title, .. } if title == "Ssh from LANlord networks"),
            )
        };
        assert!(!has_notice(&window));
        *fake.stored.lock().unwrap() =
            "networks:\n  - {name: lab, interface: eth1, address: 192.0.2.1/24}\n".to_string();
        window.refresh(&services, "me@gw");
        let until = Instant::now() + Duration::from_secs(5);
        while !has_notice(&window) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
            window.refresh(&services, "me@gw");
        }
        assert!(has_notice(&window));
    }
}
