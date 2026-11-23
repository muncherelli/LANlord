//! the LANlord Components window: a setup wizard in the style of the nt "Windows Components"
//! wizard. page one ticks the components this host runs (Details... for their parts), page two
//! picks the LANlord networks each one joins and, where it can have one, an address of its own
//! there, and page three sums it up. Finish stores the choice in the server's settings; making
//! the server match it is the job of an apply, which comes later.
//!
//! it also keeps what is switched on now, which the desktop reads to show only those icons.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use lanlord_model::compat::{self, mark};
pub use lanlord_model::components::Selection;
use lanlord_model::components::{
    catalogue, networks_from_settings, Attachment, Choice, Component, Mechanism, Network,
};
use lanlord_model::doctor::{Check, Report, Status};
use lanlord_model::settings::Settings;
use lanlord_model::validate::ipv4_partial;

use crate::remote::Remote;
use crate::services::Services;
use crate::view::{self, Button, Dialog, HeldFor, Node};
use crate::view_wizard::{CheckList, CheckRow, KeyedInput};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Step {
    #[default]
    Choose,
    Attach,
    Summary,
}

/// what a click or a keystroke in the window means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    /// highlight a row of the component list.
    Pick(usize),
    /// tick or clear a component.
    Toggle(usize),
    /// open the parts of the highlighted component.
    Details,
    /// tick or clear a part in the Details box.
    TogglePart(usize),
    CloseDetails,
    /// join or leave a network: (component on the networks page, network).
    ToggleNet(usize, usize),
    /// the address field `key` now holds this text.
    Address((usize, usize), String),
    /// choose its own address (true) or the gateway's (false) on a network.
    OwnAddress((usize, usize), bool),
    /// open the Compatibility tab of My Server.
    Compat,
    Back,
    Next,
    Finish,
    Cancel,
}

#[derive(Default)]
pub struct Components {
    /// the server's settings as last read or saved.
    pub settings: Remote<Settings>,
    /// what is switched on now; `None` until the settings were read.
    current: Option<Selection>,
    networks: Vec<Network>,
    /// why the `components` section could not be read; the desktop then falls back to the old
    /// top-level module sections and the wizard only says so.
    error: Option<String>,
    /// the wizard's working copy, made when the window opens.
    draft: Option<Selection>,
    /// what was typed into the address fields, by (component, network).
    typed: BTreeMap<(String, String), String>,
    /// the server `current` was read from.
    taken_from: Option<String>,
    /// the settings are read again at the next chance (the window opened or others saved).
    stale: bool,
    pub step: Step,
    /// the highlighted row on the first page.
    pub row: usize,
    details: bool,
    saving: Remote<Settings>,
    /// the wizard finished or was cancelled; the window closes.
    done: bool,
    /// the doctor's report of the server, copied from the shared cache on each refresh.
    report: Option<Report>,
    /// the Details link asked for the Compatibility tab.
    show_compat: bool,
}

impl Components {
    /// what is switched on now, once the settings were read.
    pub fn current(&self) -> Option<&Selection> {
        self.current.as_ref()
    }

    /// the server's networks, the outside one first.
    pub fn networks(&self) -> &[Network] {
        &self.networks
    }

    /// start over from what is switched on now.
    pub fn open(&mut self) {
        self.invalidate();
        self.draft = self.current.clone();
        self.typed = BTreeMap::new();
        if let Some(draft) = &self.draft {
            for (id, choice) in &draft.enabled {
                for attachment in &choice.attach {
                    if let Some(address) = attachment.address {
                        self.typed.insert(
                            (id.clone(), attachment.network.clone()),
                            address.to_string(),
                        );
                    }
                }
            }
        }
        self.step = Step::Choose;
        self.row = 0;
        self.details = false;
        self.done = false;
    }

    /// the cached settings may be out of date (another window saved): read them again.
    pub fn invalidate(&mut self) {
        self.stale = true;
    }

    /// the Details link was clicked, once: the desktop opens the Compatibility tab.
    pub fn take_compat(&mut self) -> bool {
        std::mem::take(&mut self.show_compat)
    }

    /// what stops `component` on this server: the checks that definitely fail for it (and for
    /// the components it needs, which it brings along). empty while the report is not read.
    fn blockers(&self, component: &Component) -> Vec<&Check> {
        let Some(report) = &self.report else {
            return Vec::new();
        };
        let mut found = compat::blockers(report, component.id());
        for id in component.requirements(&catalogue()) {
            for check in compat::blockers(report, id) {
                if !found.iter().any(|c| c.id == check.id) {
                    found.push(check);
                }
            }
        }
        found
    }

    /// the wizard finished or was cancelled, once.
    pub fn take_done(&mut self) -> bool {
        std::mem::take(&mut self.done)
    }

    /// take over freshly read (or saved) settings of `host`; a working copy made from another
    /// server's settings is dropped.
    pub fn take_settings(&mut self, settings: &Settings, host: &str) {
        if self.taken_from.as_deref() != Some(host) {
            self.draft = None;
            self.typed = BTreeMap::new();
            self.taken_from = Some(host.to_string());
        }
        self.networks = networks_from_settings(settings);
        match Selection::from_settings(settings) {
            Ok(selection) => {
                self.current = Some(selection);
                self.error = None;
            }
            Err(error) => {
                self.current = Some(Selection::from_sections(settings));
                self.error = Some(error.to_string());
            }
        }
    }

    /// pick up finished reads and saves; read `host` when the window shows another server.
    pub fn refresh(&mut self, services: &Services, host: &str) {
        self.report = services.compat().report(host);
        // the server is probed once something needs the answer: a switched-on component that
        // has requirements, or the wizard being open
        let needed = self.draft.is_some()
            || self
                .current
                .as_ref()
                .is_some_and(|c| c.enabled.keys().any(|m| compat::needs_probe(m)));
        if needed && self.current.is_some() {
            services.compat().background(services, host);
        }
        if self.settings.poll() {
            if let Some(settings) = self.settings.data.clone() {
                self.take_settings(&settings, host);
            }
        }
        if self.saving.poll() {
            if let Some(settings) = self.saving.data.take() {
                self.take_settings(&settings, host);
                self.settings.data = Some(settings);
                self.draft = None;
                self.done = true;
            }
        }
        let reread = self.stale && !self.settings.busy() && !self.saving.busy();
        if self.settings.wants_load(host, false) || reread {
            let target = host.to_string();
            let job = services.spawn(host, move |gateway, ask| {
                gateway.load_settings(&target, ask)
            });
            let started = self
                .settings
                .track(host, job, "reading the components...", "", true);
            self.stale = !started;
        }
    }

    fn draft(&self) -> Selection {
        self.draft
            .clone()
            .or_else(|| self.current.clone())
            .unwrap_or_default()
    }

    fn draft_mut(&mut self) -> &mut Selection {
        if self.draft.is_none() {
            self.draft = Some(self.draft());
        }
        self.draft.get_or_insert_with(Selection::default)
    }

    /// the components on the networks page: the chosen ones that join networks.
    fn joining(&self) -> Vec<Component> {
        let draft = self.draft();
        catalogue()
            .into_iter()
            .filter(|c| c.attaches() && draft.is_enabled(c.id()))
            .collect()
    }

    /// the draft with the typed addresses filled in, and the fields that are not addresses.
    fn candidate(&self) -> (Selection, Vec<String>) {
        let mut selection = self.draft();
        let mut problems = Vec::new();
        for (id, choice) in &mut selection.enabled {
            for attachment in &mut choice.attach {
                let typed = self
                    .typed
                    .get(&(id.clone(), attachment.network.clone()))
                    .map(|t| t.trim())
                    .unwrap_or("");
                attachment.address = if typed.is_empty() {
                    None
                } else if let Ok(address) = typed.parse::<Ipv4Addr>() {
                    Some(address)
                } else {
                    problems.push(format!("{typed:?} is not an ipv4 address"));
                    None
                };
            }
        }
        (selection, problems)
    }

    /// the address field a problem line is about, when it quotes one: its wizard key and
    /// whether the text could still become an address.
    fn typed_address_for(&self, problem: &str) -> Option<((usize, usize), bool)> {
        let joining = self.joining();
        self.typed.iter().find_map(|((id, network), text)| {
            let text = text.trim();
            if text.parse::<Ipv4Addr>().is_ok()
                || *problem != format!("{text:?} is not an ipv4 address")
            {
                return None;
            }
            let c = joining.iter().position(|comp| comp.id() == id)?;
            let n = self.networks.iter().position(|net| net.id == *network)?;
            Some(((c, n), ipv4_partial(text)))
        })
    }

    /// what is wrong with the choice so far, first problem first.
    fn problems(&self) -> Vec<String> {
        let (selection, mut problems) = self.candidate();
        problems.extend(
            selection
                .validate(&self.networks)
                .into_iter()
                .map(|e| e.message),
        );
        problems
    }

    pub fn update(&mut self, msg: Msg, services: &Services, host: &str) {
        let catalogue = catalogue();
        match msg {
            Msg::Pick(row) => self.row = row.min(catalogue.len().saturating_sub(1)),
            Msg::Toggle(row) => {
                self.row = row;
                if let Some(component) = catalogue.get(row) {
                    self.toggle(component, &catalogue);
                }
            }
            Msg::Compat => self.show_compat = true,
            Msg::Details => {
                self.details = catalogue.get(self.row).is_some_and(|c| {
                    !c.subcomponents.is_empty() && self.draft().is_enabled(c.id())
                });
            }
            Msg::TogglePart(i) => {
                if let Some(component) = catalogue.get(self.row) {
                    self.toggle_part(component, i);
                }
            }
            Msg::CloseDetails => self.details = false,
            Msg::ToggleNet(c, n) => self.toggle_net(c, n),
            Msg::Address((c, n), text) => {
                if let (Some(component), Some(network)) =
                    (self.joining().into_iter().nth(c), self.networks.get(n))
                {
                    self.typed
                        .insert((component.id().to_string(), network.id.clone()), text);
                }
            }
            Msg::OwnAddress((c, n), own) => {
                if let (Some(component), Some(network)) =
                    (self.joining().into_iter().nth(c), self.networks.get(n))
                {
                    let key = (component.id().to_string(), network.id.clone());
                    if own {
                        self.typed.entry(key).or_default();
                    } else {
                        self.typed.remove(&key);
                    }
                }
            }
            Msg::Back => {
                self.step = match self.step {
                    Step::Summary if !self.joining().is_empty() => Step::Attach,
                    Step::Summary | Step::Attach | Step::Choose => Step::Choose,
                };
            }
            Msg::Next => {
                if self.step == Step::Choose {
                    // the networks page lists what the server has now
                    self.invalidate();
                }
                self.step = match self.step {
                    Step::Choose if !self.joining().is_empty() => Step::Attach,
                    Step::Choose => Step::Summary,
                    Step::Attach if self.problems().is_empty() => Step::Summary,
                    other => other,
                };
            }
            Msg::Finish => self.finish(services, host),
            Msg::Cancel => {
                self.draft = None;
                self.details = false;
                self.step = Step::Choose;
                self.done = true;
            }
        }
    }

    /// tick or untick a component, with what it brings along.
    fn toggle(&mut self, component: &Component, catalogue: &[Component]) {
        if component.required {
            return;
        }
        let on = self.draft().is_enabled(component.id());
        if !on && !self.blockers(component).is_empty() {
            return;
        }
        let first = self
            .networks
            .iter()
            .find(|n| !n.external)
            .map(|n| n.id.clone());
        let draft = self.draft_mut();
        // like the nt wizard, a component others need stays ticked
        if catalogue
            .iter()
            .any(|c| c.requires.contains(&component.id()) && draft.is_enabled(c.id()))
        {
            return;
        }
        if draft.enabled.remove(component.id()).is_some() {
            self.typed.retain(|(id, _), _| id != component.id());
            return;
        }
        // what it needs comes in the way a user tick would bring it, deepest first
        for id in component.requirements(catalogue) {
            if let Some(required) = catalogue.iter().find(|c| c.id() == id) {
                tick(draft, required, first.clone());
            }
        }
        tick(draft, component, first);
    }

    /// tick or untick part `i` of the component.
    fn toggle_part(&mut self, component: &Component, i: usize) {
        let Some(part) = component.subcomponents.get(i) else {
            return;
        };
        let Some(choice) = self.draft_mut().enabled.get_mut(component.id()) else {
            return;
        };
        let parts = &mut choice.subcomponents;
        match parts.iter().position(|p| p == part.id) {
            Some(at) => {
                parts.remove(at);
            }
            None => parts.push(part.id.to_string()),
        }
    }

    /// join or leave network `n` for the `c`th component that joins networks.
    fn toggle_net(&mut self, c: usize, n: usize) {
        let (Some(component), Some(network)) = (
            self.joining().into_iter().nth(c),
            self.networks.get(n).cloned(),
        ) else {
            return;
        };
        let Some(choice) = self.draft_mut().enabled.get_mut(component.id()) else {
            return;
        };
        match choice.attach.iter().position(|a| a.network == network.id) {
            Some(at) => {
                choice.attach.remove(at);
                // what was typed for a network it has left is not kept
                self.typed.remove(&(component.id().to_string(), network.id));
            }
            None => choice.attach.push(Attachment {
                network: network.id,
                address: None,
            }),
        }
    }

    /// store the choice in the server's settings.
    fn finish(&mut self, services: &Services, host: &str) {
        let (selection, problems) = self.candidate();
        if !problems.is_empty() || !selection.validate(&self.networks).is_empty() {
            self.step = Step::Attach;
            return;
        }
        // a component switched on before the report arrived is checked now
        let catalogue = catalogue();
        let mut refused: Vec<String> = Vec::new();
        for component in catalogue
            .iter()
            .filter(|c| selection.is_enabled(c.id()))
            // only what is newly switched on: what the server already runs stays
            .filter(|c| !self.current.as_ref().is_some_and(|s| s.is_enabled(c.id())))
            .filter(|c| !self.blockers(c).is_empty())
        {
            let why: Vec<&str> = self
                .blockers(component)
                .iter()
                .map(|c| c.detail.as_str())
                .collect();
            refused.push(format!("{}: {}", component.name(), why.join("; ")));
        }
        if !refused.is_empty() {
            self.step = Step::Choose;
            self.saving.say(
                true,
                format!(
                    "this server cannot run what is ticked, clear it first ({})",
                    refused.join(" | ")
                ),
            );
            return;
        }
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| {
            let mut settings = gateway.load_settings(&target, ask)?;
            // the networks may have changed since the window read them
            let problems = selection.validate(&networks_from_settings(&settings));
            if let Some(problem) = problems.first() {
                anyhow::bail!("the server's settings changed: {problem}");
            }
            selection.apply_to(&mut settings);
            gateway.save_settings(&target, &settings, ask)?;
            Ok(settings)
        });
        self.saving.track(
            host,
            job,
            "saving the components...",
            "the components were saved",
            false,
        );
    }

    /// the window's contents; `idle` when nothing runs against the server.
    pub fn view(&self, idle: bool) -> Vec<Node<Msg>> {
        if self.current.is_none() {
            return vec![
                Node::Text("Reading the server's components...".into()),
                view::status(&self.settings),
            ];
        }
        if let Some(error) = &self.error {
            return vec![
                Node::banner(
                    "LANlord Components",
                    "The server's component settings cannot be read.",
                    "settings",
                ),
                Node::Gap(4.0),
                Node::Text(
                    "Fix the components section in the Settings window, then open this \
                     window again."
                        .into(),
                ),
                Node::Gap(6.0),
                Node::Buttons(vec![Button::new("Cancel", true, Msg::Cancel)]),
                Node::Status {
                    text: error.clone(),
                    error: true,
                },
            ];
        }
        // every page is as tall as the tallest, so the wizard keeps one size and its buttons stay
        // where they are, as the pages of a property sheet do
        let mut pages = vec![
            (Step::Choose, self.choose()),
            (Step::Attach, self.attach()),
            (Step::Summary, self.summary()),
        ];
        // what the pages show, which can make one taller
        let key = view::pages_fingerprint_of(
            pages
                .iter()
                .enumerate()
                .map(|(i, (_, nodes))| (i, nodes.as_slice())),
        );
        let at = pages.iter().position(|(s, _)| *s == self.step).unwrap_or(0);
        let (_, body) = pages.remove(at);
        let mut page = vec![Node::SameSize {
            key,
            body,
            others: pages.into_iter().map(|(_, p)| p).collect(),
        }];
        let can_next = match self.step {
            Step::Choose => true,
            Step::Attach => self.problems().is_empty(),
            Step::Summary => false,
        };
        let busy = self.saving.busy();
        let forward = if self.step == Step::Summary {
            Button::new("Finish", idle && !busy, Msg::Finish)
        } else {
            Button::new("Next >", can_next, Msg::Next)
        };
        page.push(Node::Gap(6.0));
        page.push(Node::Buttons(vec![
            Button::new("Cancel", !busy, Msg::Cancel),
            forward,
            Button::new("< Back", self.step != Step::Choose && !busy, Msg::Back),
        ]));
        if self.saving.note.is_some() {
            page.push(view::status(&self.saving));
        }
        page
    }

    fn choose(&self) -> Vec<Node<Msg>> {
        let draft = self.draft();
        let catalogue = catalogue();
        let rows = catalogue
            .iter()
            .map(|c| {
                let choice = draft.enabled.get(c.id());
                CheckRow {
                    icon: c.id(),
                    label: c.name().to_string(),
                    detail: self.detail(c),
                    checked: choice.is_some(),
                    partial: choice.is_some_and(|ch| {
                        !c.subcomponents.is_empty()
                            && ch.subcomponents.len() < c.subcomponents.len()
                    }),
                    enabled: !c.required && (choice.is_some() || self.blockers(c).is_empty()),
                }
            })
            .collect();
        let highlighted = catalogue.get(self.row);
        let blocked: Vec<&Check> = highlighted.map_or(Vec::new(), |c| self.blockers(c));
        let description = highlighted.map_or(String::new(), |c| {
            let ports = c.ports();
            if ports.is_empty() {
                c.description().to_string()
            } else {
                format!("{}\n\nListens on: {ports}", c.description())
            }
        });
        let details =
            highlighted.is_some_and(|c| !c.subcomponents.is_empty() && draft.is_enabled(c.id()));
        let mut page = vec![
            Node::banner(
                "LANlord Components",
                "You can add or remove components of LANlord.",
                "settings",
            ),
            Node::Gap(4.0),
            Node::Text(
                "To add or remove a component, click the checkbox. A shaded box means that \
                 only part of the component will be installed. To see what's included in a \
                 component, click Details."
                    .into(),
            ),
            Node::Gap(4.0),
            Node::Text("Components:".into()),
            Node::CheckList(CheckList {
                id: "components",
                rows,
                selected: Some(self.row),
                height: 150.0,
                select: Msg::Pick,
                toggle: Msg::Toggle,
            }),
            Node::Gap(4.0),
        ];
        // what the highlighted component is and what stops it: a box of a fixed height, which
        // scrolls, so ticking things does not change the size of the page
        let mut about = vec![Node::Text(description)];
        if !blocked.is_empty() {
            about.push(Node::Gap(4.0));
            about.push(Node::Warning(
                "This server cannot run it yet, so it cannot be switched on:".into(),
            ));
            for check in &blocked {
                about.push(Node::Text(format!("  - {}", check.detail)));
            }
        }
        page.push(Node::Details {
            title: highlighted.map_or("Description".into(), |c| c.name().to_string()),
            lines: 6,
            body: about,
        });
        page.push(Node::Gap(4.0));
        page.push(Node::Row(vec![
            Node::Button(Button::new("Details...", details, Msg::Details)),
            Node::Gap(6.0),
            Node::Button(Button::new(
                "Compatibility...",
                !blocked.is_empty(),
                Msg::Compat,
            )),
        ]));
        page
    }

    /// the right-hand column of a component: what the server says about its requirements, then
    /// what it needs.
    fn detail(&self, component: &Component) -> String {
        let needs = component.needs();
        let Some(report) = &self.report else {
            return needs;
        };
        if !compat::needs_probe(component.id()) {
            return needs;
        }
        let worst = compat::problems(report, component.id())
            .into_iter()
            .map(|c| c.status)
            .max_by_key(|s| match s {
                Status::Fail => 3,
                Status::Warn => 2,
                _ => 1,
            });
        format!("{}  {needs}", mark(worst.unwrap_or(Status::Ok)))
    }

    fn attach(&self) -> Vec<Node<Msg>> {
        let draft = self.draft();
        let mut page = vec![
            Node::banner(
                "Networks",
                "Choose the LANlord networks each component joins.",
                "networks",
            ),
            Node::Gap(4.0),
        ];
        if !self.networks.iter().any(|n| !n.external) {
            page.push(Node::Text(
                "This server has no LANlord networks yet. Add one in the Networks window \
                 first, then come back."
                    .into(),
            ));
            return page;
        }
        for (c, component) in self.joining().iter().enumerate() {
            let choice = draft.enabled.get(component.id());
            let mut body = Vec::new();
            for (n, network) in self.networks.iter().enumerate() {
                if network.external && !component.external {
                    continue;
                }
                let joined =
                    choice.is_some_and(|ch| ch.attach.iter().any(|a| a.network == network.id));
                let label = match network.subnet {
                    Some(subnet) => format!("{} ({subnet})", network.name),
                    None => network.name.clone(),
                };
                body.push(Node::Row(vec![Node::Check {
                    label,
                    checked: joined,
                    enabled: true,
                    toggle: Msg::ToggleNet(c, n),
                }]));
                if joined && component.id() == "nt4-domain" && !network.external {
                    // its network and address are decided by its own settings, not here
                    body.push(Node::Row(vec![
                        Node::Gap(22.0),
                        Node::Text(
                            "Its address is the address of its nt4-servers entry in the Settings \
                             window (the gateway's when blank)."
                                .into(),
                        ),
                    ]));
                } else if joined && component.own_address() && !network.external {
                    // the model asks for an address when it is not a host bind
                    let required = component.needs_own_address
                        || choice.is_some_and(|ch| ch.mechanism != Mechanism::HostBind);
                    body.extend(self.address_rows(component, network, (c, n), required));
                }
            }
            page.push(Node::Gap(4.0));
            page.push(Node::Group {
                title: component.name().into(),
                body,
            });
        }
        if let Some(problem) = self.problems().first() {
            page.push(Node::Gap(4.0));
            page.push(self.problem_line(problem));
        }
        page
    }

    /// the rows under a joined network: the gateway's address or its own, with the box for it.
    fn address_rows(
        &self,
        component: &Component,
        network: &Network,
        at: (usize, usize),
        required: bool,
    ) -> Vec<Node<Msg>> {
        let key = (component.id().to_string(), network.id.clone());
        // an address held for this network means it has chosen its own
        let own = self.typed.contains_key(&key);
        let mut rows = Vec::new();
        if !required {
            let gateway = match network.gateway {
                Some(ip) => format!("Gateway address ({ip})"),
                None => "Gateway address".to_string(),
            };
            rows.push(Node::Row(vec![
                Node::Gap(22.0),
                Node::Check {
                    label: gateway,
                    checked: !own,
                    enabled: true,
                    toggle: Msg::OwnAddress(at, false),
                },
            ]));
        }
        let mut own_row = vec![Node::Gap(22.0)];
        if required {
            own_row.push(Node::Text("Its own address:".into()));
        } else {
            own_row.push(Node::Check {
                label: "Its own address:".into(),
                checked: own,
                enabled: true,
                toggle: Msg::OwnAddress(at, true),
            });
        }
        if own || required {
            own_row.push(Node::KeyedInput(KeyedInput {
                key: at,
                value: self.typed.get(&key).cloned().unwrap_or_default(),
                width: 110.0,
                edit: Msg::Address,
            }));
        }
        rows.push(Node::Row(own_row));
        rows
    }

    /// the problem line of the networks page. an address still being typed is not called wrong
    /// until the focus leaves its own field: the problem is matched to the address it quotes.
    fn problem_line(&self, problem: &str) -> Node<Msg> {
        match self.typed_address_for(problem) {
            Some((field, true)) => Node::HeldError {
                text: problem.to_string(),
                partial: true,
                field: HeldFor::Keyed(field),
            },
            _ => Node::Status {
                text: problem.to_string(),
                error: true,
            },
        }
    }

    fn summary(&self) -> Vec<Node<Msg>> {
        let (draft, _) = self.candidate();
        let now = self.current.clone().unwrap_or_default();
        let mut lines = Vec::new();
        for component in catalogue() {
            let (before, after) = (
                now.enabled.get(component.id()),
                draft.enabled.get(component.id()),
            );
            let joins = |choice: &Choice| -> String {
                let names: Vec<String> = choice
                    .attach
                    .iter()
                    .map(|a| {
                        let name = self
                            .networks
                            .iter()
                            .find(|n| n.id == a.network)
                            .map_or(a.network.as_str(), |n| n.name.as_str());
                        match a.address {
                            Some(address) => format!("{name} at {address}"),
                            None => name.to_string(),
                        }
                    })
                    .collect();
                if names.is_empty() {
                    String::new()
                } else {
                    format!(" on {}", names.join(", "))
                }
            };
            match (before, after) {
                (None, Some(choice)) => {
                    lines.push(format!("Add {}{}", component.name(), joins(choice)));
                }
                (Some(_), None) => lines.push(format!("Remove {}", component.name())),
                (Some(old), Some(new)) if old != new => {
                    lines.push(format!("Change {}{}", component.name(), joins(new)));
                }
                _ => {}
            }
        }
        if lines.is_empty() {
            lines.push("Nothing changes.".into());
        }
        let mut page = vec![
            Node::banner(
                "Completing the LANlord Components Wizard",
                "Check what will change, then click Finish.",
                "settings",
            ),
            Node::Gap(4.0),
        ];
        page.extend(lines.into_iter().map(Node::Text));
        page.push(Node::Gap(6.0));
        page.push(Node::Text(
            "Finish saves this in the server's settings. The server takes it up the next \
             time the settings are applied."
                .into(),
        ));
        page
    }

    /// the Details box for the highlighted component's parts.
    pub fn dialog(&self) -> Option<Dialog<Msg>> {
        if !self.details {
            return None;
        }
        let component = catalogue().into_iter().nth(self.row)?;
        let draft = self.draft();
        let chosen = draft.enabled.get(component.id())?;
        let mut body = vec![
            Node::Text(format!(
                "To add or remove a part of {}, click its checkbox.",
                component.name()
            )),
            Node::Gap(4.0),
        ];
        for (i, part) in component.subcomponents.iter().enumerate() {
            body.push(Node::Check {
                label: part.name.into(),
                checked: chosen.subcomponents.iter().any(|p| p == part.id),
                enabled: true,
                toggle: Msg::TogglePart(i),
            });
            body.push(Node::Text(format!("    {}", part.description)));
        }
        body.push(Node::Gap(6.0));
        body.push(Node::Buttons(vec![Button::new(
            "OK",
            true,
            Msg::CloseDetails,
        )]));
        Some(Dialog {
            id: "component-details",
            title: component.name().to_string(),
            body,
            escape: Some(Msg::CloseDetails),
        })
    }
}

/// switch `component` on with its first network and default parts, unless it already is.
fn tick(draft: &mut Selection, component: &Component, first: Option<String>) {
    if draft.is_enabled(component.id()) {
        return;
    }
    let attach = match (component.attaches(), first) {
        (true, Some(network)) => vec![Attachment {
            network,
            address: None,
        }],
        _ => Vec::new(),
    };
    draft.enabled.insert(
        component.id().to_string(),
        Choice {
            attach,
            subcomponents: component.default_subcomponents(),
            ..Choice::default()
        },
    );
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::gateway::Fake;
    use crate::wake::Wake;

    const SETTINGS: &str = "networks:\n  - {id: retronet, name: Retro LAN, subnet: 192.0.2.0/24, gateway: 192.0.2.1, dhcp: {range: [192.0.2.100, 192.0.2.200]}}\n  - {id: lab, name: Lab, subnet: 198.51.100.0/24, gateway: 198.51.100.1}\ndocker: {}\ndns: {}\n";

    fn ready() -> (Components, Services, Arc<Fake>) {
        let fake = Arc::new(Fake::default());
        *fake.stored.lock().unwrap() = SETTINGS.into();
        let services = Services::new(fake.clone(), Wake::none());
        let mut window = Components::default();
        window.take_settings(&Settings::parse(SETTINGS).unwrap(), "gw");
        window.open();
        (window, services, fake)
    }

    fn row(id: &str) -> usize {
        catalogue().iter().position(|c| c.id() == id).unwrap()
    }

    #[test]
    fn old_settings_show_what_they_configure() {
        let (window, _, _) = ready();
        let current = window.current().unwrap();
        assert!(current.is_enabled("gateway") && current.is_enabled("dns"));
        assert!(current.is_enabled("dhcp"), "the old dns entry brings dhcp");
        assert!(!current.is_enabled("nt4-domain"));
        assert_eq!(
            window.networks().len(),
            3,
            "the outside network and two lans"
        );
    }

    #[test]
    fn ticking_a_component_joins_the_first_network_and_the_gateway_stays() {
        let (mut window, services, _) = ready();
        window.update(Msg::Toggle(row("nt4-domain")), &services, "gw");
        let draft = window.draft();
        assert_eq!(draft.enabled["nt4-domain"].attach[0].network, "retronet");
        assert_eq!(
            draft.enabled["nt4-domain"].subcomponents,
            ["domain-controller", "wins"]
        );
        window.update(Msg::Toggle(row("gateway")), &services, "gw");
        assert!(
            window.draft().is_enabled("gateway"),
            "the gateway cannot be cleared"
        );
        window.update(Msg::Toggle(row("nt4-domain")), &services, "gw");
        assert!(!window.draft().is_enabled("nt4-domain"));
    }

    #[test]
    fn another_servers_settings_drop_the_working_copy() {
        let (mut window, services, _) = ready();
        window.update(Msg::Toggle(row("nt4-domain")), &services, "gw");
        assert!(window.draft().is_enabled("nt4-domain"));
        window.take_settings(&Settings::parse(SETTINGS).unwrap(), "gw");
        assert!(window.draft().is_enabled("nt4-domain"), "same server");
        window.take_settings(&Settings::parse(SETTINGS).unwrap(), "other");
        assert!(!window.draft().is_enabled("nt4-domain"));
        assert!(window.typed.is_empty());
    }

    #[test]
    fn clearing_a_component_drops_what_was_typed_for_it() {
        let (mut window, services, _) = ready();
        window.update(Msg::Toggle(row("nt4-domain")), &services, "gw");
        let nt4 = window
            .joining()
            .iter()
            .position(|c| c.id() == "nt4-domain")
            .unwrap();
        window.update(
            Msg::Address((nt4, 1), "192.0.2.150".into()),
            &services,
            "gw",
        );
        assert!(!window.typed.is_empty());
        window.update(Msg::Toggle(row("nt4-domain")), &services, "gw");
        assert!(window.typed.is_empty());
    }

    #[test]
    fn opening_reads_the_settings_again() {
        let (mut window, services, fake) = ready();
        window.refresh(&services, "gw");
        while window.settings.busy() || !window.settings.belongs_to("gw") {
            window.refresh(&services, "gw");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let reads = || fake.calls().iter().filter(|c| c.contains("load")).count();
        let before = reads();
        window.refresh(&services, "gw");
        assert_eq!(reads(), before, "cached while nothing changed");
        window.open();
        window.refresh(&services, "gw");
        while window.settings.busy() {
            window.refresh(&services, "gw");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(reads(), before + 1);
    }

    #[test]
    fn the_pages_go_choose_networks_summary_and_back() {
        let (mut window, services, _) = ready();
        window.update(Msg::Next, &services, "gw");
        assert_eq!(window.step, Step::Attach, "dns joins networks");
        window.update(Msg::Next, &services, "gw");
        assert_eq!(window.step, Step::Summary);
        window.update(Msg::Back, &services, "gw");
        assert_eq!(window.step, Step::Attach);
        window.update(Msg::Back, &services, "gw");
        assert_eq!(window.step, Step::Choose);
        // with nothing that joins networks the networks page is skipped
        window.update(Msg::Toggle(row("dns")), &services, "gw");
        window.update(Msg::Toggle(row("dhcp")), &services, "gw");
        window.update(Msg::Next, &services, "gw");
        assert_eq!(window.step, Step::Summary);
    }

    #[test]
    fn next_reads_the_networks_again_and_the_page_lists_the_new_one() {
        let (mut window, services, fake) = ready();
        let added = lanlord_model::lan_edit::Edit::Add(
            lanlord_model::lan_networks::LanNetwork::new("newnet", "eth1", "172.16.120.1/22"),
        );
        let edited = lanlord_model::lan_edit::edit_settings("docker: {}\n", &added).unwrap();
        *fake.stored.lock().unwrap() = edited;
        assert_eq!(window.networks().len(), 3);
        window.update(Msg::Next, &services, "gw");
        window.refresh(&services, "gw");
        while window.settings.busy() {
            window.refresh(&services, "gw");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(window.networks().iter().any(|n| n.id == "newnet"));
    }

    #[test]
    fn with_no_lanlord_network_the_page_only_says_so() {
        let mut window = Components::default();
        window.take_settings(&Settings::parse("docker: {}\ndns: {}\n").unwrap(), "gw");
        window.open();
        let services = Services::new(Arc::new(Fake::default()), Wake::none());
        window.update(Msg::Next, &services, "gw");
        assert_eq!(window.step, Step::Attach);
        let page = window.attach();
        assert!(!page.iter().any(|n| matches!(n, Node::Group { .. })));
        assert!(page
            .iter()
            .any(|n| matches!(n, Node::Text(t) if t.contains("no LANlord networks yet"))));
    }

    fn has_check(page: &[Node<Msg>], text: &str) -> bool {
        fn walk(nodes: &[Node<Msg>], text: &str) -> bool {
            nodes.iter().any(|n| match n {
                Node::Check { label, .. } => label == text,
                Node::Row(body) | Node::Group { body, .. } => walk(body, text),
                _ => false,
            })
        }
        walk(page, text)
    }

    fn has_input(page: &[Node<Msg>]) -> bool {
        fn walk(nodes: &[Node<Msg>]) -> bool {
            nodes.iter().any(|n| match n {
                Node::KeyedInput(_) => true,
                Node::Row(body) | Node::Group { body, .. } => walk(body),
                _ => false,
            })
        }
        walk(page)
    }

    fn dns_address(window: &Components) -> Option<Option<Ipv4Addr>> {
        window
            .candidate()
            .0
            .enabled
            .get("dns")
            .and_then(|c| c.attach.first())
            .map(|a| a.address)
    }

    #[test]
    fn a_joined_network_defaults_to_the_gateway_address() {
        let (mut window, services, _) = ready();
        window.update(Msg::Next, &services, "gw");
        let page = window.attach();
        assert!(has_check(&page, "Gateway address (192.0.2.1)"));
        assert!(!has_input(&page), "no box until it has its own address");
        assert_eq!(dns_address(&window), Some(None));
    }

    #[test]
    fn choosing_its_own_address_and_going_back_to_the_gateway() {
        let (mut window, services, _) = ready();
        window.update(Msg::Next, &services, "gw");
        let dns = window
            .joining()
            .iter()
            .position(|c| c.id() == "dns")
            .unwrap();
        let own: Ipv4Addr = "192.0.2.150".parse().unwrap();
        window.update(Msg::OwnAddress((dns, 1), true), &services, "gw");
        assert!(has_input(&window.attach()));
        window.update(Msg::Address((dns, 1), own.to_string()), &services, "gw");
        assert_eq!(dns_address(&window), Some(Some(own)));
        window.update(Msg::OwnAddress((dns, 1), false), &services, "gw");
        assert!(!has_input(&window.attach()));
        assert_eq!(
            dns_address(&window),
            Some(None),
            "the typed address is gone"
        );
    }

    #[test]
    fn nt4_points_to_its_own_window_for_the_address() {
        let (mut window, services, _) = ready();
        window.update(Msg::Toggle(row("nt4-domain")), &services, "gw");
        window.update(Msg::Next, &services, "gw");
        let page = window.attach();
        let body = page
            .iter()
            .find_map(|n| match n {
                Node::Group { title, body } if title == "NT4 Server" => Some(body),
                _ => None,
            })
            .unwrap();
        assert!(!has_input(body), "no address box for it here");
        assert!(body.iter().any(|n| matches!(n, Node::Row(r)
            if r.iter().any(|n| matches!(n, Node::Text(t) if t.contains("nt4-servers entry"))))));
    }

    #[test]
    fn a_mechanism_other_than_host_bind_requires_an_address() {
        let (mut window, services, _) = ready();
        window.draft_mut().enabled.get_mut("dns").unwrap().mechanism = Mechanism::Ipvlan;
        window.update(Msg::Next, &services, "gw");
        let page = window.attach();
        let group = |title: &str| {
            page.iter()
                .find_map(|n| match n {
                    Node::Group { title: t, body } if t == title => Some(body),
                    _ => None,
                })
                .unwrap()
        };
        let dns = group("DNS");
        assert!(has_input(dns), "the box is there with no gateway option");
        assert!(!has_check(dns, "Gateway address (192.0.2.1)"));
        assert!(has_check(group("DHCP"), "Gateway address (192.0.2.1)"));
    }

    #[test]
    fn leaving_a_network_forgets_the_address_typed_for_it() {
        let (mut window, services, _) = ready();
        window.update(Msg::Next, &services, "gw");
        let dns = window
            .joining()
            .iter()
            .position(|c| c.id() == "dns")
            .unwrap();
        window.update(Msg::OwnAddress((dns, 1), true), &services, "gw");
        window.update(
            Msg::Address((dns, 1), "192.0.2.150".into()),
            &services,
            "gw",
        );
        window.update(Msg::ToggleNet(dns, 1), &services, "gw");
        assert!(window.typed.is_empty());
        window.update(Msg::ToggleNet(dns, 1), &services, "gw");
        assert!(!has_input(&window.attach()), "it rejoins on the gateway");
    }

    #[test]
    fn an_existing_own_address_opens_as_its_own_address() {
        let (mut window, services, _) = ready();
        let own: Ipv4Addr = "192.0.2.150".parse().unwrap();
        window.draft_mut().enabled.get_mut("dns").unwrap().attach[0].address = Some(own);
        window.current = Some(window.draft());
        window.open();
        window.update(Msg::Next, &services, "gw");
        assert!(has_input(&window.attach()));
        assert_eq!(dns_address(&window), Some(Some(own)));
    }

    #[test]
    fn an_address_typed_in_the_networks_page_is_held_until_it_is_whole() {
        let (mut window, services, _) = ready();
        window.update(Msg::Toggle(row("nt4-domain")), &services, "gw");
        window.update(Msg::Next, &services, "gw");
        let nt4 = window
            .joining()
            .iter()
            .position(|c| c.id() == "nt4-domain")
            .unwrap();
        let mut typed = String::new();
        for ch in "192.0.2.77".chars() {
            typed.push(ch);
            window.update(Msg::Address((nt4, 1), typed.clone()), &services, "gw");
            let page = window.attach();
            let held = page
                .iter()
                .any(|n| matches!(n, Node::HeldError { partial: true, .. }));
            let complete = typed.parse::<Ipv4Addr>().is_ok();
            // a partial address is held at every step; the finished one has no error left
            assert!(held || complete, "{typed:?}");
            assert!(!page.iter().any(|n| matches!(n, Node::Status { .. })) || complete);
            // Next stays shut until the address is whole and fits
            assert_eq!(window.problems().is_empty(), complete, "{typed:?}");
        }
        // a value that is wrong whatever comes next is not held
        window.update(Msg::Address((nt4, 1), "192.0.2.x".into()), &services, "gw");
        assert!(window
            .attach()
            .iter()
            .any(|n| matches!(n, Node::Status { error: true, .. })));
    }

    #[test]
    fn the_held_line_is_the_one_for_the_address_still_being_typed() {
        let (mut window, services, _) = ready();
        window.update(Msg::Toggle(row("nt4-domain")), &services, "gw");
        window.update(Msg::Next, &services, "gw");
        let at = |w: &Components, id: &str| w.joining().iter().position(|c| c.id() == id).unwrap();
        let (nt4, dns) = (at(&window, "nt4-domain"), at(&window, "dns"));
        let held = |w: &Components| {
            w.attach().into_iter().find_map(|n| match n {
                Node::HeldError { field, .. } => Some(field),
                _ => None,
            })
        };
        // one finished but wrong address, the other whole: the wrong one is called wrong
        window.update(Msg::Address((nt4, 1), "192.0.2.x".into()), &services, "gw");
        window.update(Msg::Address((dns, 1), "192.0.2.5".into()), &services, "gw");
        assert_eq!(held(&window), None);
        assert!(window.attach().iter().any(
            |n| matches!(n, Node::Status { error: true, text } if text.contains("192.0.2.x"))
        ));
        // a half typed address is held for its own field, whichever one it is
        window.update(Msg::Address((dns, 1), "192.0.".into()), &services, "gw");
        assert_eq!(held(&window), Some(HeldFor::Keyed((dns, 1))));
        window.update(Msg::Address((dns, 1), "192.0.2.5".into()), &services, "gw");
        window.update(Msg::Address((nt4, 1), " 192.0. ".into()), &services, "gw");
        assert_eq!(held(&window), Some(HeldFor::Keyed((nt4, 1))));
        // padding on either side does not make a whole address wrong
        window.update(
            Msg::Address((nt4, 1), "  192.0.2.6 ".into()),
            &services,
            "gw",
        );
        assert!(window.problems().is_empty());
    }

    #[test]
    fn a_bad_address_holds_the_networks_page() {
        let (mut window, services, _) = ready();
        window.update(Msg::Toggle(row("nt4-domain")), &services, "gw");
        window.update(Msg::Next, &services, "gw");
        let nt4 = window
            .joining()
            .iter()
            .position(|c| c.id() == "nt4-domain")
            .unwrap();
        // join the lab too (networks: outside, retronet, lab), then give the domain an address
        // inside the retro lan's dhcp range
        window.update(Msg::ToggleNet(nt4, 2), &services, "gw");
        window.update(
            Msg::Address((nt4, 1), "192.0.2.150".into()),
            &services,
            "gw",
        );
        window.update(Msg::Next, &services, "gw");
        assert_eq!(window.step, Step::Attach);
        assert!(window.problems()[0].contains("DHCP range"));
        window.update(Msg::Address((nt4, 1), "nonsense".into()), &services, "gw");
        assert!(window.problems()[0].contains("not an ipv4 address"));
        window.update(Msg::Address((nt4, 1), "192.0.2.5".into()), &services, "gw");
        window.update(Msg::Next, &services, "gw");
        assert_eq!(window.step, Step::Summary);
        let (selection, _) = window.candidate();
        let attach = &selection.enabled["nt4-domain"].attach;
        assert_eq!(attach.len(), 2);
        assert_eq!(attach[0].address, Some(Ipv4Addr::new(192, 0, 2, 5)));
        assert_eq!(attach[1].network, "lab");
        let text: Vec<String> = window
            .summary()
            .into_iter()
            .filter_map(|n| match n {
                Node::Text(t) => Some(t),
                _ => None,
            })
            .collect();
        assert!(
            text.iter()
                .any(|t| t == "Add NT4 Server on Retro LAN at 192.0.2.5, Lab"),
            "{text:?}"
        );
    }

    #[test]
    fn details_toggle_parts_and_shade_the_box() {
        let (mut window, services, _) = ready();
        window.update(Msg::Toggle(row("nt4-domain")), &services, "gw");
        window.update(Msg::Pick(row("nt4-domain")), &services, "gw");
        window.update(Msg::Details, &services, "gw");
        assert!(window.dialog().is_some());
        window.update(Msg::TogglePart(1), &services, "gw");
        assert_eq!(
            window.draft().enabled["nt4-domain"].subcomponents,
            ["domain-controller"]
        );
        window.update(Msg::CloseDetails, &services, "gw");
        assert!(window.dialog().is_none());
        let Node::CheckList(list) = window.choose().remove(5) else {
            panic!("the list is the sixth node");
        };
        assert!(list.rows[row("nt4-domain")].partial);
    }

    #[test]
    fn finish_saves_the_choice_and_closes() {
        let (mut window, services, fake) = ready();
        window.update(Msg::Toggle(row("web-proxy")), &services, "gw");
        window.update(Msg::Next, &services, "gw");
        window.update(Msg::Next, &services, "gw");
        window.update(Msg::Finish, &services, "gw");
        for _ in 0..200 {
            window.refresh(&services, "gw");
            if window.take_done() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(window.current().unwrap().is_enabled("web-proxy"));
        let stored = fake.stored.lock().unwrap().clone();
        assert!(stored.contains("components:"), "{stored}");
        assert!(stored.contains("web-proxy"), "{stored}");
    }

    #[test]
    fn docker_comes_along_and_stays_while_needed() {
        let (mut window, services, _) = ready();
        window.update(Msg::Toggle(row("docker")), &services, "gw");
        assert!(window.draft().is_enabled("docker"), "dns needs docker");
        window.update(Msg::Toggle(row("dns")), &services, "gw");
        window.update(Msg::Toggle(row("docker")), &services, "gw");
        assert!(
            window.draft().is_enabled("docker"),
            "dhcp still needs docker"
        );
        window.update(Msg::Toggle(row("dhcp")), &services, "gw");
        window.update(Msg::Toggle(row("docker")), &services, "gw");
        assert!(!window.draft().is_enabled("docker"));
        window.update(Msg::Toggle(row("web-proxy")), &services, "gw");
        assert!(
            window.draft().is_enabled("docker"),
            "ticking the proxy brings docker"
        );
    }

    #[test]
    fn a_broken_components_section_is_shown_and_the_old_sections_still_count() {
        let (mut window, services, _) = ready();
        window.take_settings(
            &Settings::parse("docker: {}\ndns: {}\ncomponents: 7\n").unwrap(),
            "gw",
        );
        assert!(window.current().unwrap().is_enabled("dns"));
        let page = window.view(true);
        assert!(page
            .iter()
            .any(|n| matches!(n, Node::Status { error: true, .. })));
        window.update(Msg::Cancel, &services, "gw");
        assert!(window.take_done());
    }

    #[test]
    fn finish_checks_the_settings_it_saves_into() {
        let (mut window, services, fake) = ready();
        window.update(Msg::Toggle(row("web-proxy")), &services, "gw");
        // meanwhile the server lost its retro lan
        *fake.stored.lock().unwrap() =
            "networks:\n  - {id: lab, name: Lab, subnet: 198.51.100.0/24, gateway: 198.51.100.1}\ndocker: {}\n".into();
        window.update(Msg::Finish, &services, "gw");
        for _ in 0..200 {
            window.refresh(&services, "gw");
            if !window.saving.busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let (error, text) = window.saving.note.clone().unwrap();
        assert!(error && text.contains("changed"), "{text}");
        assert!(!fake.stored.lock().unwrap().contains("components:"));
        assert!(!window.take_done());
    }

    #[test]
    fn cancel_drops_the_draft() {
        let (mut window, services, _) = ready();
        window.update(Msg::Toggle(row("web-proxy")), &services, "gw");
        window.update(Msg::Cancel, &services, "gw");
        assert!(window.take_done());
        assert!(!window.draft().is_enabled("web-proxy"));
    }

    const UNPRIVILEGED: &str = "virt_container=lxc\nvirt_vm=none\nuid_map=0 100000 65536\npve_marker=1\nnesting=no\nkeyctl=no\n";

    /// a window on the unprivileged container, with the report read and nothing switched on.
    fn blocked() -> (Components, Services) {
        let fake = Arc::new(Fake::default());
        *fake.probe.lock().unwrap() = UNPRIVILEGED.into();
        *fake.stored.lock().unwrap() = SETTINGS.replace("docker: {}\ndns: {}\n", "");
        let services = Services::new(fake, Wake::none());
        let mut window = Components::default();
        window.take_settings(
            &Settings::parse(&SETTINGS.replace("docker: {}\ndns: {}\n", "")).unwrap(),
            "gw",
        );
        window.open();
        let start = std::time::Instant::now();
        while window.report.is_none() {
            window.refresh(&services, "gw");
            assert!(start.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        (window, services)
    }

    #[test]
    fn a_component_whose_requirements_fail_cannot_be_switched_on_and_says_why() {
        let (mut window, services) = blocked();
        let nt4 = row("nt4-domain");
        window.update(Msg::Toggle(nt4), &services, "gw");
        assert!(
            !window.draft().is_enabled("nt4-domain"),
            "refused: nesting and keyctl are off"
        );
        assert!(
            !window.draft().is_enabled("docker"),
            "docker did not come along"
        );
        let page = window.choose();
        // what stops it is in the box under the list
        let nodes: Vec<&Node<Msg>> = page
            .iter()
            .flat_map(|n| match n {
                Node::Details { body, .. } => body.iter().collect(),
                n => vec![n],
            })
            .collect();
        assert!(nodes
            .iter()
            .any(|n| matches!(n, Node::Warning(w) if w.contains("cannot be switched on"))));
        assert!(nodes
            .iter()
            .any(|n| matches!(n, Node::Text(t) if t.contains("nesting"))));
        let nodes = page;
        // the Details link goes to the Compatibility tab
        let link = nodes.iter().find_map(|n| match n {
            Node::Row(items) => items.iter().find_map(|i| match i {
                Node::Button(b) if b.label == "Compatibility..." => Some(b.enabled),
                _ => None,
            }),
            _ => None,
        });
        assert_eq!(link, Some(true));
        window.update(Msg::Compat, &services, "gw");
        assert!(window.take_compat());
        assert!(!window.take_compat(), "once");
        // a component with no requirements still switches on
        let web = row("remote-commands");
        window.update(Msg::Toggle(web), &services, "gw");
        assert!(window.draft().is_enabled("remote-commands"));
    }

    #[test]
    fn the_rows_carry_the_mark_of_their_requirements() {
        let (window, _) = blocked();
        let nodes = window.choose();
        let Some(Node::CheckList(list)) = nodes.iter().find(|n| matches!(n, Node::CheckList(_)))
        else {
            panic!("no list");
        };
        let nt4 = &list.rows[row("nt4-domain")];
        assert!(
            nt4.detail.starts_with(&format!("{}  ", mark(Status::Fail))),
            "{}",
            nt4.detail
        );
        assert!(!nt4.enabled, "greyed out");
        let gateway = &list.rows[row("gateway")];
        assert!(!gateway.detail.starts_with(mark(Status::Fail)));
    }

    #[test]
    fn a_privileged_container_with_nesting_lets_docker_components_on() {
        let fake = Arc::new(Fake::default());
        *fake.probe.lock().unwrap() =
            "virt_container=lxc\nvirt_vm=none\nuid_map=0 0 4294967295\nnesting=yes\n".into();
        let services = Services::new(fake, Wake::none());
        let mut window = Components::default();
        window.take_settings(&Settings::parse("networks: []\n").unwrap(), "gw");
        window.open();
        let start = std::time::Instant::now();
        while window.report.is_none() {
            window.refresh(&services, "gw");
            assert!(start.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        window.update(Msg::Toggle(row("nt4-domain")), &services, "gw");
        assert!(window.draft().is_enabled("nt4-domain"));
    }

    #[test]
    fn finish_refuses_a_component_that_was_ticked_before_the_report_arrived() {
        let (mut window, services) = blocked();
        // ticked while nothing was known about the server
        let report = window.report.take();
        window.update(Msg::Toggle(row("nt4-domain")), &services, "gw");
        assert!(window.draft().is_enabled("nt4-domain"));
        window.report = report;
        window.update(Msg::Finish, &services, "gw");
        assert_eq!(window.step, Step::Choose);
        let (error, text) = window.saving.note.clone().unwrap();
        assert!(error && text.contains("nesting"), "{text}");
        assert!(!window.saving.busy(), "nothing was saved");
    }

    #[test]
    fn finish_still_saves_a_blocked_component_that_is_already_on() {
        let fake = Arc::new(Fake::default());
        *fake.probe.lock().unwrap() = UNPRIVILEGED.into();
        *fake.stored.lock().unwrap() = SETTINGS.into();
        let services = Services::new(fake.clone(), Wake::none());
        let mut window = Components::default();
        window.take_settings(&Settings::parse(SETTINGS).unwrap(), "gw");
        window.open();
        let start = std::time::Instant::now();
        while window.report.is_none() {
            window.refresh(&services, "gw");
            assert!(start.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(window.draft().is_enabled("dns"), "already on");
        assert!(!window
            .blockers(&lanlord_model::components::find("dns").unwrap())
            .is_empty());
        window.update(Msg::Toggle(row("remote-commands")), &services, "gw");
        window.update(Msg::Finish, &services, "gw");
        for _ in 0..200 {
            window.refresh(&services, "gw");
            if window.take_done() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(fake.stored.lock().unwrap().contains("components:"));
        assert!(window.current().unwrap().is_enabled("remote-commands"));
    }

    #[test]
    fn a_required_middle_link_comes_in_fully_configured() {
        use lanlord_model::components::SubComponent;
        use lanlord_model::modules::{Module, Status};
        static MODULES: [Module; 3] = [
            module("base", "Base"),
            module("middle", "Middle"),
            module("top", "Top"),
        ];
        static PARTS: [SubComponent; 1] = [SubComponent {
            id: "part",
            name: "Part",
            description: "",
            default: true,
        }];
        const fn module(id: &'static str, name: &'static str) -> Module {
            Module {
                id,
                name,
                icon: ["", "", ""],
                summary: "",
                example: "",
                status: Status::Available,
            }
        }
        let entry = |i: usize, requires, joins: bool| Component {
            module: &MODULES[i],
            required: false,
            requires,
            ports: &[],
            roles: &[],
            netbios: false,
            attach_min: usize::from(joins),
            attach_max: if joins { None } else { Some(0) },
            external: false,
            needs_own_address: false,
            mechanisms: &[Mechanism::HostBind],
            subcomponents: if i == 1 { &PARTS } else { &[] },
        };
        let catalogue = vec![
            entry(0, &[], false),
            entry(1, &["base"], true),
            entry(2, &["middle"], false),
        ];
        let (mut window, _, _) = ready();
        window.toggle(&catalogue[2], &catalogue);
        let draft = window.draft();
        assert!(draft.is_enabled("base") && draft.is_enabled("top"));
        let middle = &draft.enabled["middle"];
        assert_eq!(middle.subcomponents, ["part"]);
        assert_eq!(middle.attach.len(), 1, "it joins the first network");
    }
}
