//! the LANlord Components window: a setup wizard in the style of the nt "Windows Components"
//! wizard. page one ticks the components this host runs (Details... for their parts), page two
//! picks the LANlord networks each one joins and, where it can have one, an address of its own
//! there, and page three sums it up. Finish stores the choice in the server's settings; making
//! the server match it is the job of an apply, which comes later.
//!
//! it also keeps what is switched on now, which the desktop reads to show only those icons.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use lanlord_core::settings::Settings;
pub use lanlord_model::components::Selection;
use lanlord_model::components::{
    catalogue, networks_from_settings, Attachment, Choice, Component, Network,
};

use crate::remote::Remote;
use crate::services::Services;
use crate::view::{self, Button, Dialog, Node};
use crate::view_wizard::{CheckList, CheckRow, KeyedInput};

/// address fields are keyed by `component * KEYS + network`.
const KEYS: usize = 64;

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
    Address(usize, String),
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
    /// the wizard's working copy, made when the window opens.
    draft: Option<Selection>,
    /// what was typed into the address fields, by (component, network).
    typed: BTreeMap<(String, String), String>,
    pub step: Step,
    /// the highlighted row on the first page.
    pub row: usize,
    details: bool,
    saving: Remote<Settings>,
    /// the wizard finished or was cancelled; the window closes.
    done: bool,
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

    /// the wizard finished or was cancelled, once.
    pub fn take_done(&mut self) -> bool {
        std::mem::take(&mut self.done)
    }

    /// take over freshly read (or saved) settings.
    pub fn take_settings(&mut self, settings: &Settings) {
        self.networks = networks_from_settings(settings);
        self.current = Selection::from_settings(settings).ok();
    }

    /// pick up finished reads and saves; read `host` when the window shows another server.
    pub fn refresh(&mut self, services: &Services, host: &str) {
        if self.settings.poll() {
            if let Some(settings) = self.settings.data.clone() {
                self.take_settings(&settings);
            }
        }
        if self.saving.poll() {
            if let Some(settings) = self.saving.data.take() {
                self.take_settings(&settings);
                self.settings.data = Some(settings);
                self.draft = None;
                self.done = true;
            }
        }
        if self.settings.wants_load(host, false) {
            let target = host.to_string();
            let job = services.spawn(host, move |gateway, ask| {
                gateway.load_settings(&target, ask)
            });
            self.settings
                .track(host, job, "reading the components...", "", true);
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
                let Some(component) = catalogue.get(row) else {
                    return;
                };
                if component.required {
                    return;
                }
                let first = self
                    .networks
                    .iter()
                    .find(|n| !n.external)
                    .map(|n| n.id.clone());
                let draft = self.draft_mut();
                if component.id() == "docker" {
                    // like the nt wizard, a component others need stays ticked
                    let needed = catalogue
                        .iter()
                        .any(|c| c.requires_docker && draft.is_enabled(c.id()));
                    if needed {
                        return;
                    }
                }
                if draft.enabled.remove(component.id()).is_none() {
                    if component.requires_docker {
                        draft.enabled.entry("docker".into()).or_default();
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
            }
            Msg::Details => {
                self.details = catalogue.get(self.row).is_some_and(|c| {
                    !c.subcomponents.is_empty() && self.draft().is_enabled(c.id())
                });
            }
            Msg::TogglePart(i) => {
                let Some(component) = catalogue.get(self.row) else {
                    return;
                };
                let Some(part) = component.subcomponents.get(i) else {
                    return;
                };
                if let Some(choice) = self.draft_mut().enabled.get_mut(component.id()) {
                    let parts = &mut choice.subcomponents;
                    if let Some(at) = parts.iter().position(|p| p == part.id) {
                        parts.remove(at);
                    } else {
                        parts.push(part.id.to_string());
                    }
                }
            }
            Msg::CloseDetails => self.details = false,
            Msg::ToggleNet(c, n) => {
                let (Some(component), Some(network)) = (
                    self.joining().into_iter().nth(c),
                    self.networks.get(n).cloned(),
                ) else {
                    return;
                };
                if let Some(choice) = self.draft_mut().enabled.get_mut(component.id()) {
                    if let Some(at) = choice.attach.iter().position(|a| a.network == network.id) {
                        choice.attach.remove(at);
                    } else {
                        choice.attach.push(Attachment {
                            network: network.id,
                            address: None,
                        });
                    }
                }
            }
            Msg::Address(key, text) => {
                let (c, n) = (key / KEYS, key % KEYS);
                if let (Some(component), Some(network)) =
                    (self.joining().into_iter().nth(c), self.networks.get(n))
                {
                    self.typed
                        .insert((component.id().to_string(), network.id.clone()), text);
                }
            }
            Msg::Back => {
                self.step = match self.step {
                    Step::Summary if !self.joining().is_empty() => Step::Attach,
                    Step::Summary | Step::Attach | Step::Choose => Step::Choose,
                };
            }
            Msg::Next => {
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

    /// store the choice in the server's settings.
    fn finish(&mut self, services: &Services, host: &str) {
        let (selection, problems) = self.candidate();
        if !problems.is_empty() || !selection.validate(&self.networks).is_empty() {
            self.step = Step::Attach;
            return;
        }
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| {
            let mut settings = gateway.load_settings(&target, ask)?;
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
        let mut page = match self.step {
            Step::Choose => self.choose(),
            Step::Attach => self.attach(),
            Step::Summary => self.summary(),
        };
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
                    detail: c.needs(),
                    checked: choice.is_some(),
                    partial: choice.is_some_and(|ch| {
                        !c.subcomponents.is_empty()
                            && ch.subcomponents.len() < c.subcomponents.len()
                    }),
                    enabled: !c.required,
                }
            })
            .collect();
        let highlighted = catalogue.get(self.row);
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
        vec![
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
            Node::Description(description),
            Node::Gap(4.0),
            Node::Row(vec![Node::Button(Button::new(
                "Details...",
                details,
                Msg::Details,
            ))]),
        ]
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
        } else {
            page.push(Node::Text(
                "Leave the address blank to use the gateway's address on that network.".into(),
            ));
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
                let mut row = vec![Node::Check {
                    label,
                    checked: joined,
                    enabled: true,
                    toggle: Msg::ToggleNet(c, n),
                }];
                if joined && component.own_address() && !network.external {
                    row.push(Node::Text("Address:".into()));
                    row.push(Node::KeyedInput(KeyedInput {
                        key: c * KEYS + n,
                        value: self
                            .typed
                            .get(&(component.id().to_string(), network.id.clone()))
                            .cloned()
                            .unwrap_or_default(),
                        width: 110.0,
                        edit: Msg::Address,
                    }));
                }
                body.push(Node::Row(row));
            }
            page.push(Node::Gap(4.0));
            page.push(Node::Group {
                title: component.name().into(),
                body,
            });
        }
        if let Some(problem) = self.problems().first() {
            page.push(Node::Gap(4.0));
            page.push(Node::Status {
                text: problem.clone(),
                error: true,
            });
        }
        page
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
        })
    }
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
        window.take_settings(&Settings::parse(SETTINGS).unwrap());
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
        window.update(Msg::Next, &services, "gw");
        assert_eq!(window.step, Step::Summary);
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
            Msg::Address(nt4 * KEYS + 1, "192.0.2.150".into()),
            &services,
            "gw",
        );
        window.update(Msg::Next, &services, "gw");
        assert_eq!(window.step, Step::Attach);
        assert!(window.problems()[0].contains("DHCP range"));
        window.update(
            Msg::Address(nt4 * KEYS + 1, "nonsense".into()),
            &services,
            "gw",
        );
        assert!(window.problems()[0].contains("not an ipv4 address"));
        window.update(
            Msg::Address(nt4 * KEYS + 1, "192.0.2.5".into()),
            &services,
            "gw",
        );
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
                .any(|t| t == "Add NT4 Domain on Retro LAN at 192.0.2.5, Lab"),
            "{text:?}"
        );
    }

    #[test]
    fn details_toggle_parts_and_shade_the_box() {
        let (mut window, services, _) = ready();
        window.update(Msg::Pick(row("dns")), &services, "gw");
        window.update(Msg::Details, &services, "gw");
        assert!(window.dialog().is_some());
        window.update(Msg::TogglePart(1), &services, "gw");
        assert_eq!(window.draft().enabled["dns"].subcomponents, ["dns"]);
        window.update(Msg::CloseDetails, &services, "gw");
        assert!(window.dialog().is_none());
        let Node::CheckList(list) = window.choose().remove(5) else {
            panic!("the list is the sixth node");
        };
        assert!(list.rows[row("dns")].partial);
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
        assert!(!window.draft().is_enabled("docker"));
        window.update(Msg::Toggle(row("web-proxy")), &services, "gw");
        assert!(
            window.draft().is_enabled("docker"),
            "ticking the proxy brings docker"
        );
    }

    #[test]
    fn cancel_drops_the_draft() {
        let (mut window, services, _) = ready();
        window.update(Msg::Toggle(row("web-proxy")), &services, "gw");
        window.update(Msg::Cancel, &services, "gw");
        assert!(window.take_done());
        assert!(!window.draft().is_enabled("web-proxy"));
    }
}
