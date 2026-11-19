//! the Networks window: the server's external network and the networks lanlord runs, in the
//! style of Network and Dial-up Connections. each network has a tab showing what Windows shows
//! in a connection's Status box (link, speed and traffic), and Details... opens the Network
//! Connection Details list. New network... adds a lanlord network on a card that no network uses,
//! and Properties of a lanlord network changes or removes it, the way `lanlord networks add`,
//! `set` and `remove` do; the external network is only shown.

use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use lanlord_model::lan_edit::{external_interface, network_users, Edit};
use lanlord_model::lan_networks::{self, LanNetwork};
use lanlord_model::netstatus::InterfaceStatus;
use lanlord_model::networks::{self, Draft, NetworkRow, Role};
use lanlord_model::server::Iface;
use lanlord_model::settings::Settings;

use crate::ask::AskPassword;
use crate::gateway::Gateway;
use crate::remote::Remote;
use crate::services::Services;
use crate::view::{self, Button, Dialog, Node, Size, Table};

/// how often an open window reads the status again, so the traffic counts move.
pub const STATUS_EVERY: f64 = 5.0;

/// a text field of the network form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    Address,
    RangeStart,
    RangeEnd,
    Lease,
    Zone,
    Mtu,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    /// the tab of this network.
    Tab(usize),
    Refresh,
    /// open the Network Connection Details of the tab that is open.
    Details,
    /// open the DNS & DHCP window.
    Dns,
    /// open the properties of the tab that is open.
    Properties,
    CloseProperties,
    CloseConnection,
    /// open the form for a new network.
    New,
    /// the interface list was clicked at this row: an unused card starts a new network.
    PickInterface(usize),
    /// a field of the form was edited.
    Edit(Field, String),
    /// the form's interface drop-down chose this card.
    PickCard(usize),
    /// check the form, save the network and apply.
    Save,
    CloseForm,
    /// ask to remove the network whose form is open.
    Remove,
    ConfirmRemove,
    CancelRemove,
    /// a click that means nothing, such as one on a row of a read-only list.
    Ignore,
}

/// what the window asks the desktop to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Ask {
    /// open the DNS & DHCP window.
    Dns,
}

/// what one read of the server gives the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Networks {
    /// the networks, one tab each.
    pub rows: Vec<NetworkRow>,
    /// every interface of the server, which the tabs do not all show.
    pub interfaces: Vec<Iface>,
    /// the lanlord networks as the settings hold them, for the form's checks.
    pub lan: Vec<LanNetwork>,
    /// the interface of the external network, which no lanlord network may use.
    pub external: Option<String>,
    /// what still uses a lanlord network, by network name: those cannot be removed.
    pub used: Vec<(String, Vec<String>)>,
}

impl Networks {
    /// what in the settings still names the network `name`.
    fn users_of(&self, name: &str) -> Vec<String> {
        self.used
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, users)| users.clone())
            .unwrap_or_default()
    }
}

/// the form that adds a network or changes one.
struct NetForm {
    /// the network being changed; `None` for a new one.
    existing: Option<String>,
    draft: Draft,
    /// the draft the form opened with, so a save sends only what was changed.
    original: Draft,
    /// the cards the interface drop-down offers.
    cards: Vec<Iface>,
    /// something was typed or picked, so what is wrong is worth showing.
    touched: bool,
    /// why the last save failed.
    error: Option<String>,
}

/// read the networks, the interfaces and the lanlord networks of the settings.
fn read(gateway: &dyn Gateway, target: &str, ask: AskPassword) -> Result<Networks> {
    let facts = gateway.server_facts(target, ask)?;
    // networks still show without settings, or without the key to read them
    let settings = gateway
        .load_settings(target, ask)
        .unwrap_or_else(|_| Settings::empty());
    let leases = gateway.leases(target, ask).ok().map(|l| l.len());
    let rows = networks::rows(&facts.interfaces, &settings, leases);
    let external = rows
        .iter()
        .find(|r| r.role == Role::External)
        .map(|r| r.interface.clone())
        .or_else(|| external_interface(&settings));
    Ok(Networks {
        lan: lan_networks::read(&settings)
            .map(|n| n.list)
            .unwrap_or_default(),
        external,
        used: lan_networks::read(&settings)
            .map(|n| n.list)
            .unwrap_or_default()
            .iter()
            .map(|n| (n.name.clone(), network_users(&settings, &n.name)))
            .collect(),
        rows,
        interfaces: facts.interfaces,
    })
}

/// how the server starts the error of a change that was stored but did not apply.
const SAVED_NOT_APPLIED: &str = "Saved, but applying failed";

#[derive(Default)]
pub struct NetworksWindow {
    rows: Remote<Networks>,
    /// the status of every connection, matched to a row by its interface.
    status: Remote<Vec<InterfaceStatus>>,
    /// the tab that is open.
    selected: usize,
    /// the network whose properties are open.
    details: Option<NetworkRow>,
    /// the connection whose Network Connection Details are open.
    connection: Option<InterfaceStatus>,
    /// the form that adds or changes a network.
    form: Option<NetForm>,
    /// the network a confirmation to remove is open for.
    confirm: Option<String>,
    /// a save or a removal is on its way, so its form stays open until it is known how it went.
    saving: bool,
    /// set by the job of a save that was stored but could not be applied.
    warning: Arc<Mutex<Option<String>>>,
    /// when the status was last asked for, in seconds on the caller's clock.
    status_at: f64,
    /// writes a unix time for the lease lines; this computer's time zone when `None`.
    time: Option<fn(i64) -> String>,
}

impl NetworksWindow {
    /// write lease times with `time` rather than in this computer's time zone, so a test or a
    /// scene shows the same text everywhere.
    pub fn with_time(mut self, time: fn(i64) -> String) -> Self {
        self.time = Some(time);
        self
    }

    pub fn rows(&self) -> &Remote<Networks> {
        &self.rows
    }

    pub fn status(&self) -> &Remote<Vec<InterfaceStatus>> {
        &self.status
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn details(&self) -> Option<&NetworkRow> {
        self.details.as_ref()
    }

    pub fn connection(&self) -> Option<&InterfaceStatus> {
        self.connection.as_ref()
    }

    /// is a form (new network or properties) or the remove confirmation open?
    pub fn editing(&self) -> bool {
        self.form.is_some() || self.confirm.is_some()
    }

    pub fn poll(&mut self) {
        let arrived = self.rows.poll();
        self.status.poll();
        if self.saving && !self.rows.busy() {
            self.saving = false;
            self.confirm = None;
            let warning = self.warning.lock().ok().and_then(|mut w| w.take());
            if let Some(text) = warning {
                // the change is stored and the list read again; only the apply failed
                self.form = None;
                self.rows.say(true, text);
            } else if arrived {
                self.form = None;
            } else if let (Some(form), Some((true, text))) = (&mut self.form, &self.rows.note) {
                form.error = Some(text.clone());
            }
        }
    }

    fn reload(&mut self, services: &Services, host: &str) {
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| read(gateway, &target, ask));
        self.rows
            .track(host, job, "reading the networks...", "up to date", true);
    }

    /// store `edit` in the settings, apply the docker and dns modules (the apply dialog shows
    /// how it goes) and read the networks again.
    fn change(&mut self, services: &Services, host: &str, edit: Edit, doing: &str, done: &str) {
        let target = host.to_string();
        let warning = self.warning.clone();
        let job = services.spawn(host, move |gateway, ask| {
            match gateway.change_networks(&target, &edit, ask) {
                Ok(()) => {}
                // stored but not applied: show the networks as they are now, and say why
                Err(e) if e.to_string().contains(SAVED_NOT_APPLIED) => {
                    let networks = read(gateway, &target, ask).map_err(|_| anyhow!("{e}"))?;
                    if let Ok(mut slot) = warning.lock() {
                        *slot = Some(e.to_string());
                    }
                    return Ok(networks);
                }
                Err(e) => return Err(e),
            }
            read(gateway, &target, ask)
        });
        self.saving = self.rows.track(host, job, doing, done, false);
    }

    /// may a network be added or changed now: the networks are read, and nothing is running.
    fn can_edit(&self, services: &Services, host: &str) -> bool {
        self.rows.belongs_to(host) && !self.rows.busy() && !services.busy(host)
    }

    /// the cards no network uses.
    fn unused_cards(&self) -> Vec<Iface> {
        self.rows.data.as_ref().map_or_else(Vec::new, |d| {
            networks::unused(&d.interfaces, &d.rows)
                .into_iter()
                .cloned()
                .collect()
        })
    }

    /// open the form for a new network on `card`, or on the card that suits best.
    fn open_new(&mut self, card: Option<Iface>) {
        let cards = self.unused_cards();
        let refs: Vec<&Iface> = cards.iter().collect();
        let card = card.or_else(|| networks::suggested_card(&refs).cloned());
        let Some(card) = card else { return };
        self.form = Some(NetForm {
            existing: None,
            draft: Draft::on_card(&card),
            original: Draft::default(),
            cards,
            touched: false,
            error: None,
        });
    }

    /// open the form for the lanlord network of the tab that is open.
    fn open_properties(&mut self) {
        let Some(row) = self.chosen().filter(|r| r.role == Role::Lanlord).cloned() else {
            self.details = self.chosen().cloned();
            return;
        };
        let Some(data) = self.rows.data.as_ref() else {
            return;
        };
        let network = data
            .lan
            .iter()
            .find(|n| n.name.eq_ignore_ascii_case(&row.name))
            .cloned()
            .unwrap_or_else(|| LanNetwork::new(&row.name, &row.interface, &row.address));
        let mut draft = Draft::of(&network);
        // settings from before the networks list leave these to be found
        if draft.interface.is_empty() {
            draft.interface = row.interface.clone();
        }
        if draft.address.is_empty() {
            draft.address = row.address.clone();
        }
        let mut cards = self.unused_cards();
        let own = data.interfaces.iter().find(|i| i.name == draft.interface);
        cards.insert(
            0,
            own.cloned().unwrap_or_else(|| Iface {
                name: draft.interface.clone(),
                mtu: row.mtu,
                state: String::new(),
                addresses: Vec::new(),
                default_route: false,
            }),
        );
        self.form = Some(NetForm {
            existing: Some(network.name),
            original: draft.clone(),
            draft,
            cards,
            touched: false,
            error: None,
        });
    }

    /// the edit the open form makes, or what is wrong with it.
    fn form_edit(&self) -> Result<Edit, Vec<String>> {
        let (Some(form), Some(data)) = (&self.form, &self.rows.data) else {
            return Err(Vec::new());
        };
        form.draft.edit(
            form.existing.as_deref().map(|n| (n, &form.original)),
            &data.lan,
            data.external.as_deref(),
        )
    }

    /// the cards of the interface list that the list shows, in its order.
    fn listed(&self, i: usize) -> Option<Iface> {
        let data = self.rows.data.as_ref()?;
        data.interfaces
            .iter()
            .filter(|i| !i.loopback())
            .nth(i)
            .cloned()
    }

    fn reload_status(&mut self, services: &Services, host: &str, now: f64) {
        let target = host.to_string();
        let job = services.quiet(move |gateway| gateway.network_status(&target));
        self.status_at = now;
        self.status.track(host, Some(job), "", "", true);
    }

    fn status_error(&self) -> Option<String> {
        self.status
            .note
            .as_ref()
            .filter(|(error, _)| *error)
            .map(|(_, text)| text.clone())
    }

    /// the status loads when the window opens, then again every few seconds while it is open
    /// and the last read worked; after a failure it waits for Refresh.
    fn refresh(&mut self, services: &Services, host: &str, now: f64) {
        self.poll();
        if self.rows.wants_load(host, false) {
            self.reload(services, host);
        }
        let stale = self.status.belongs_to(host)
            && self.status_error().is_none()
            && !self.status.busy()
            && now - self.status_at >= STATUS_EVERY;
        if self.status.wants_load(host, false) || stale {
            self.reload_status(services, host, now);
        }
    }

    fn chosen(&self) -> Option<&NetworkRow> {
        let rows = &self.rows.data.as_ref()?.rows;
        rows.get(self.selected.min(rows.len().saturating_sub(1)))
    }

    fn status_of(&self, row: &NetworkRow) -> Option<&InterfaceStatus> {
        self.status
            .data
            .as_ref()?
            .iter()
            .find(|s| s.name == row.interface)
    }

    fn reading(&self) -> bool {
        self.rows.busy() || self.status.busy()
    }

    pub fn view(&mut self, services: &Services, host: Option<&str>, now: f64) -> Vec<Node<Msg>> {
        let Some(host) = host else {
            return vec![Node::Text("No server yet. Sign in to one first.".into())];
        };
        self.refresh(services, host, now);
        let Networks {
            rows, interfaces, ..
        } = self.rows.data.clone().unwrap_or(Networks {
            rows: Vec::new(),
            interfaces: Vec::new(),
            lan: Vec::new(),
            external: None,
            used: Vec::new(),
        });
        let can_add =
            self.can_edit(services, host) && !networks::unused(&interfaces, &rows).is_empty();
        if rows.is_empty() {
            return self.without_networks(&interfaces, &rows, can_add);
        }
        self.selected = self.selected.min(rows.len() - 1);
        let titles = rows.iter().map(|r| r.name.clone()).collect();
        let error = self.status_error();
        let chosen = &rows[self.selected];
        let status = self.status_of(chosen);
        let lanlord = chosen.role == Role::Lanlord;
        let tabs = Node::Tabs {
            titles,
            current: self.selected,
            pick: Msg::Tab,
            body: page(chosen, status, error.as_deref()),
        };
        let buttons = Node::Buttons(vec![
            Button::new("Refresh", !self.reading(), Msg::Refresh),
            Button::new("Details...", status.is_some(), Msg::Details),
            Button::new("DNS & DHCP...", lanlord && !services.busy(host), Msg::Dns),
            Button::new("Properties...", true, Msg::Properties),
        ]);
        let add = Node::Buttons(vec![Button::new("New network...", can_add, Msg::New)]);
        vec![
            tabs,
            Node::Gap(6.0),
            buttons,
            Node::Gap(6.0),
            add,
            interface_list(&interfaces, &rows),
            Node::Gap(4.0),
            view::status(&self.rows),
        ]
    }

    /// the page of a server with no network of its own to show: its interfaces, when it has any,
    /// and the status.
    fn without_networks(
        &self,
        interfaces: &[Iface],
        rows: &[NetworkRow],
        can_add: bool,
    ) -> Vec<Node<Msg>> {
        let mut nodes = Vec::new();
        if interfaces.iter().any(|i| !i.loopback()) {
            nodes.push(Node::Buttons(vec![Button::new(
                "New network...",
                can_add,
                Msg::New,
            )]));
            nodes.push(Node::Gap(6.0));
            nodes.push(interface_list(interfaces, rows));
            nodes.push(Node::Gap(4.0));
        }
        nodes.push(view::status(&self.rows));
        nodes
    }

    pub fn update(&mut self, msg: Msg, services: &Services, host: &str, now: f64) -> Option<Ask> {
        match msg {
            Msg::Tab(i) => self.selected = i,
            Msg::Refresh if !self.reading() => {
                self.reload(services, host);
                self.reload_status(services, host, now);
            }
            Msg::Details => {
                self.connection = self.chosen().and_then(|r| self.status_of(r)).cloned();
            }
            Msg::Dns => {
                let lanlord = self.chosen().is_some_and(|r| r.role == Role::Lanlord);
                if lanlord && !services.busy(host) {
                    return Some(Ask::Dns);
                }
            }
            Msg::Properties => self.open_properties(),
            Msg::CloseProperties => self.details = None,
            Msg::CloseConnection => self.connection = None,
            Msg::New if self.can_edit(services, host) => self.open_new(None),
            Msg::PickInterface(i) if self.can_edit(services, host) => {
                let free = self.unused_cards();
                if let Some(card) = self.listed(i).filter(|c| free.contains(c)) {
                    self.open_new(Some(card));
                }
            }
            Msg::Edit(field, text) => self.edit_field(field, text),
            Msg::PickCard(i) => self.pick_card(i),
            Msg::Save if self.can_edit(services, host) => match self.form_edit() {
                Ok(edit) => {
                    let (doing, done) = match &edit {
                        Edit::Add(_) => ("adding the network and applying...", "network added"),
                        _ => ("saving the network and applying...", "network saved"),
                    };
                    self.change(services, host, edit, doing, done);
                }
                Err(errors) => {
                    if let Some(form) = &mut self.form {
                        form.touched = true;
                        form.error = errors.into_iter().next();
                    }
                }
            },
            Msg::CloseForm => self.form = None,
            Msg::Remove => {
                self.confirm = self.form.as_ref().and_then(|f| f.existing.clone());
            }
            Msg::ConfirmRemove if self.can_edit(services, host) => {
                if let Some(name) = self.confirm.clone() {
                    let doing = "removing the network and applying...";
                    self.change(services, host, Edit::Remove(name), doing, "network removed");
                }
            }
            Msg::CancelRemove => self.confirm = None,
            Msg::Refresh
            | Msg::Ignore
            | Msg::New
            | Msg::PickInterface(_)
            | Msg::Save
            | Msg::ConfirmRemove => {}
        }
        None
    }

    fn edit_field(&mut self, field: Field, text: String) {
        let Some(form) = &mut self.form else { return };
        let draft = &mut form.draft;
        match field {
            Field::Name => draft.name = text,
            Field::Address => draft.address = text,
            Field::RangeStart => draft.range_start = text,
            Field::RangeEnd => draft.range_end = text,
            Field::Lease => draft.lease = text,
            Field::Zone => draft.zone = text,
            Field::Mtu => draft.mtu = text,
        }
        form.touched = true;
        form.error = None;
    }

    fn pick_card(&mut self, i: usize) {
        let Some(form) = &mut self.form else { return };
        let Some(card) = form.cards.get(i) else {
            return;
        };
        if form.existing.is_some() {
            // a network that exists keeps its settings; only the card changes
            form.draft.interface = card.name.clone();
        } else {
            form.draft.pick_card(card);
        }
        form.touched = true;
        form.error = None;
    }

    /// the remove confirmation, the form of a network, the read-only properties box of the
    /// external network and the Network Connection Details list.
    pub fn dialog(&self) -> Option<Dialog<Msg>> {
        if let Some(status) = &self.connection {
            return Some(connection_dialog(status, self.time.unwrap_or(local_time)));
        }
        if let Some(name) = &self.confirm {
            return Some(self.remove_dialog(name));
        }
        if let Some(form) = &self.form {
            return Some(self.form_dialog(form));
        }
        self.details.as_ref().map(properties_dialog)
    }

    fn remove_dialog(&self, name: &str) -> Dialog<Msg> {
        let row = self.rows.data.as_ref().and_then(|d| {
            d.rows
                .iter()
                .find(|r| r.role == Role::Lanlord && r.name.eq_ignore_ascii_case(name))
        });
        let clients = row.and_then(|r| r.leases).map_or(String::new(), |n| {
            format!(" ({n} clients hold a lease now)")
        });
        let busy = self.rows.busy();
        let users = self
            .rows
            .data
            .as_ref()
            .map_or_else(Vec::new, |d| d.users_of(name));
        let blocked = if users.is_empty() {
            None
        } else {
            Some(format!(
                "{} still uses {name}; change that before removing the network.",
                users.join(" and ")
            ))
        };
        Dialog {
            id: "network-remove",
            title: "Remove network".into(),
            body: vec![
                Node::Alert(format!("Remove the network {name}?")),
                Node::Gap(4.0),
                Node::Text(format!(
                    "This stops and removes its dnsmasq (lanlord-dns-{name}), its DHCP range and \
                     its static leases{clients}. Clients on it lose their addresses and names. \
                     The card itself stays, and can be used for a new network."
                )),
                Node::Gap(6.0),
                Node::Warning(blocked.clone().unwrap_or_default()),
                Node::Buttons(vec![
                    Button::new("Cancel", !busy, Msg::CancelRemove),
                    Button::new(
                        "Remove network",
                        !busy && blocked.is_none(),
                        Msg::ConfirmRemove,
                    ),
                ]),
            ],
            escape: Some(Msg::CancelRemove),
        }
    }

    /// the static leases that saving the open form would drop: it clears the range of a network
    /// that has reservations.
    fn leases_lost(&self, form: &NetForm) -> usize {
        let cleared =
            form.draft.range_start.trim().is_empty() && form.draft.range_end.trim().is_empty();
        let name = form.existing.as_deref();
        match (cleared, name, &self.rows.data) {
            (true, Some(name), Some(data)) => data
                .lan
                .iter()
                .find(|n| n.name.eq_ignore_ascii_case(name))
                .and_then(|n| n.dhcp.as_ref())
                .map_or(0, |d| d.hosts.len()),
            _ => 0,
        }
    }

    fn form_dialog(&self, form: &NetForm) -> Dialog<Msg> {
        let problems = if form.touched {
            self.form_edit().err().unwrap_or_default()
        } else {
            Vec::new()
        };
        let busy = self.rows.busy();
        let input = |field: Field, value: &str, hint: &str| Node::Input {
            value: value.to_string(),
            size: Size::FillLess(10.0),
            focus: false,
            hint: hint.to_string(),
            edit: Box::new(move |text| Msg::Edit(field, text)),
            enter: None,
        };
        let draft = &form.draft;
        let name = match &form.existing {
            Some(name) => Node::Text(name.clone()),
            None => input(Field::Name, &draft.name, "like retronet"),
        };
        let card = Node::Choice {
            id: "network-card",
            current: draft.interface.clone(),
            width: 200.0,
            options: form
                .cards
                .iter()
                .enumerate()
                .map(|(i, c)| (i, c.name.clone()))
                .collect(),
            pick: Msg::PickCard,
        };
        let labelled = |label: &str, body: Node<Msg>| Node::Labelled {
            label: label.into(),
            body: vec![body],
        };
        let range = Node::Row(vec![
            input(Field::RangeStart, &draft.range_start, "start"),
            Node::Text("  to  ".into()),
            input(Field::RangeEnd, &draft.range_end, "end"),
        ]);
        let mut body = vec![
            Node::Text(
                if form.existing.is_some() {
                    "Change this network. Saving applies it to the server."
                } else {
                    "Add a network on a card no network uses. Saving applies it to the server."
                }
                .into(),
            ),
            Node::Gap(6.0),
            labelled("Name", name),
            labelled("Interface", card),
            labelled(
                "Address",
                input(Field::Address, &draft.address, "like 172.16.120.1/22"),
            ),
            labelled("DHCP range", range),
            labelled("Lease time", input(Field::Lease, &draft.lease, "like 7d")),
            labelled("DNS zone", input(Field::Zone, &draft.zone, "the name")),
            labelled("MTU", input(Field::Mtu, &draft.mtu, "the card's own")),
            Node::Text("Leave the DHCP range empty for a network without DHCP.".into()),
        ];
        let leases = self.leases_lost(form);
        if leases > 0 {
            body.push(Node::Warning(format!(
                "Clearing the DHCP range removes the {leases} static lease{} of this network.",
                if leases == 1 { "" } else { "s" }
            )));
        }
        let mut shown = problems;
        shown.extend(form.error.clone().filter(|e| !shown_has(&shown, e)));
        body.extend(shown.into_iter().map(Node::Warning));
        body.push(Node::Gap(6.0));
        let mut buttons = vec![
            Button::new("Cancel", !busy, Msg::CloseForm),
            Button::new("Save & apply", !busy && self.form_edit().is_ok(), Msg::Save),
        ];
        if form.existing.is_some() {
            buttons.push(Button::new("Remove network...", !busy, Msg::Remove));
        }
        body.push(Node::Buttons(buttons));
        Dialog {
            id: "network-form",
            title: match &form.existing {
                Some(name) => format!("{name} Properties"),
                None => "New network".into(),
            },
            body,
            escape: Some(Msg::CloseForm),
        }
    }
}

fn shown_has(shown: &[String], error: &str) -> bool {
    shown.iter().any(|s| s == error)
}

/// every interface but the loopback, with its address, MTU and link: the ones the tabs leave
/// out are here too. a card no network uses is marked, and clicking it starts a new network.
fn interface_list(interfaces: &[Iface], rows: &[NetworkRow]) -> Node<Msg> {
    let free = networks::unused(interfaces, rows);
    let items = interfaces
        .iter()
        .filter(|i| !i.loopback())
        .map(|iface| {
            let address = iface.addresses.first().map_or("no address", String::as_str);
            let mark = if free.contains(&iface) {
                "   not used"
            } else {
                ""
            };
            format!(
                "{}   {}   MTU {}   {}{}",
                iface.name,
                address,
                iface.mtu,
                iface.link(),
                mark
            )
        })
        .collect::<Vec<_>>();
    Node::Group {
        title: "Network interfaces".into(),
        body: vec![Node::List {
            id: "network-interfaces",
            min_height: 18.0 * items.len() as f32,
            items,
            selected: None,
            empty: "no network interfaces".into(),
            select: Msg::PickInterface,
        }],
    }
}

fn properties_dialog(row: &NetworkRow) -> Dialog<Msg> {
    let mut body: Vec<Node<Msg>> = row
        .details()
        .into_iter()
        .map(|(label, value)| Node::field(&label, value))
        .collect();
    body.push(Node::Gap(6.0));
    body.push(Node::Buttons(vec![Button::new(
        "OK",
        true,
        Msg::CloseProperties,
    )]));
    Dialog {
        id: "network-details",
        title: format!("{} Properties", row.name),
        body,
        escape: Some(Msg::CloseProperties),
    }
}

fn connection_dialog(status: &InterfaceStatus, time: fn(i64) -> String) -> Dialog<Msg> {
    let rows = status
        .details(time)
        .into_iter()
        .map(|(key, value)| vec![key, value])
        .collect();
    Dialog {
        id: "network-connection-details",
        title: "Network Connection Details".into(),
        body: vec![
            Node::Text("Network Connection Details:".into()),
            Node::Gap(4.0),
            Node::Table(Table {
                id: "network-details-list",
                headings: vec!["Property", "Value"],
                widths: vec![150.0, 230.0],
                rows,
                selected: None,
                bad: Vec::new(),
                height: 220.0,
                select: |_| Msg::Ignore,
            }),
            Node::Gap(6.0),
            Node::Buttons(vec![Button::new("Close", true, Msg::CloseConnection)]),
        ],
        escape: Some(Msg::CloseConnection),
    }
}

/// one network's tab: Connection and Activity like a connection's Status box, and the lanlord
/// network's dhcp.
fn page(row: &NetworkRow, status: Option<&InterfaceStatus>, error: Option<&str>) -> Vec<Node<Msg>> {
    let state = status.map_or(row.link.clone(), |s| s.connection.text().to_string());
    let connection = Node::Group {
        title: "Connection".into(),
        body: vec![
            Node::field("Status:", state),
            Node::field("Interface:", row.interface.clone()),
            Node::field(
                "Speed:",
                status.map_or("Unknown".to_string(), InterfaceStatus::speed),
            ),
            Node::field("Address:", row.address.clone()),
        ],
    };
    let mut nodes = Vec::new();
    match (status, error) {
        // the traffic sits beside the connection, so the page stays short enough to leave room
        // for the interface list on a small screen
        (Some(status), _) => nodes.push(Node::Columns(vec![
            (None, vec![connection]),
            (None, vec![activity(status)]),
        ])),
        (None, Some(error)) => {
            nodes.push(connection);
            nodes.push(Node::Gap(6.0));
            nodes.push(Node::Text(format!(
                "Could not read the connection status: {error}. Press Refresh to try again."
            )));
        }
        (None, None) => nodes.push(connection),
    }
    if row.role == Role::Lanlord {
        nodes.push(Node::Gap(6.0));
        nodes.push(lanlord_network(row));
    }
    nodes
}

fn activity(status: &InterfaceStatus) -> Node<Msg> {
    let c = status.counters;
    Node::Group {
        title: "Activity".into(),
        body: vec![Node::Table(Table {
            id: "network-activity",
            headings: vec!["", "Sent", "Received"],
            widths: vec![64.0, 96.0],
            rows: vec![
                vec![
                    "Bytes:".into(),
                    grouped(c.sent_bytes),
                    grouped(c.received_bytes),
                ],
                vec![
                    "Packets:".into(),
                    grouped(c.sent_packets),
                    grouped(c.received_packets),
                ],
            ],
            selected: None,
            bad: Vec::new(),
            height: 44.0,
            select: |_| Msg::Ignore,
        })],
    }
}

fn lanlord_network(row: &NetworkRow) -> Node<Msg> {
    let dhcp = row.dhcp.clone().unwrap_or_else(|| "not served".into());
    let clients = row.leases.map_or("-".to_string(), |n| n.to_string());
    Node::Group {
        title: "LANlord network".into(),
        body: vec![
            Node::field("DHCP range:", dhcp),
            Node::field("Clients with a lease:", clients),
        ],
    }
}

/// a count with thousands separators, the way Windows writes traffic.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// a unix time in UTC, in the same shape as `local_time`.
pub fn utc_time(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| t.format("%-m/%-d/%Y %-I:%M:%S %p").to_string())
        .unwrap_or_default()
}

/// a unix time in this computer's time zone, like `11/16/2026 4:05:09 PM`.
fn local_time(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%-m/%-d/%Y %-I:%M:%S %p")
                .to_string()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanlord_model::server::demo;

    #[test]
    fn a_server_with_no_networks_still_lists_its_interfaces() {
        let window = NetworksWindow::default();
        // nothing read yet: only the status
        assert_eq!(window.without_networks(&[], &[], false).len(), 1);
        let nodes = window.without_networks(&demo().interfaces, &[], true);
        assert!(matches!(&nodes[0], Node::Buttons(buttons) if buttons[0].enabled));
        assert!(matches!(&nodes[2], Node::Group { title, .. } if title == "Network interfaces"));
        assert!(matches!(nodes.last(), Some(Node::Status { .. })));
        // only the loopback: nothing to list
        let lo = &demo().interfaces[..1];
        assert_eq!(window.without_networks(lo, &[], true).len(), 1);
    }

    #[test]
    fn the_interface_list_has_every_interface_but_the_loopback() {
        let Node::Group { title, body } = interface_list(&demo().interfaces, &[]) else {
            panic!("a group");
        };
        assert_eq!(title, "Network interfaces");
        let Some(Node::List { items, .. }) = body.into_iter().next() else {
            panic!("a list");
        };
        // no network uses either card yet
        assert_eq!(
            items,
            [
                "eth0   192.0.2.10/24   MTU 1500   Connected   not used",
                "eth1   172.16.120.1/22   MTU 1450   Connected   not used"
            ]
        );
        let settings = Settings::parse(LAB).unwrap();
        let rows = networks::rows(&demo().interfaces, &settings, None);
        let Node::Group { body, .. } = interface_list(&demo().interfaces, &rows) else {
            panic!("a group");
        };
        let Some(Node::List { items, .. }) = body.into_iter().next() else {
            panic!("a list");
        };
        assert!(items.iter().all(|i| !i.ends_with("not used")), "{items:?}");
    }

    #[test]
    fn rows_name_one_tab_per_network() {
        let settings = Settings::parse(
            "docker:\n  internal_address: 172.16.120.1/22\ndns:\n  zone: retronet\n  dhcp:\n    range: [172.16.120.100, 172.16.120.200]\n    lease: 7d\n",
        )
        .unwrap();
        let rows = networks::rows(&demo().interfaces, &settings, Some(2));
        let names: Vec<_> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["External Network", "retronet"]);
        assert_eq!(
            rows[1].dhcp.as_deref(),
            Some("172.16.120.100 - 172.16.120.200")
        );
    }

    #[test]
    fn traffic_counts_are_grouped_in_thousands() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(913_442_071), "913,442,071");
    }

    #[test]
    fn a_bad_time_is_blank_not_a_panic() {
        assert_eq!(local_time(i64::MAX), "");
        assert!(!local_time(1_795_000_000).is_empty());
    }

    #[test]
    fn without_a_server_it_says_so_and_buttons_do_nothing() {
        let services = Services::new(
            std::sync::Arc::new(crate::gateway::Fake::new()),
            crate::wake::Wake::none(),
        );
        let mut window = NetworksWindow::default();
        let nodes = window.view(&services, None, 0.0);
        assert!(matches!(&nodes[..], [Node::Text(text)] if text.starts_with("No server")));
        assert!(window.update(Msg::Refresh, &services, "h", 0.0).is_none());
        assert!(window.update(Msg::Dns, &services, "h", 0.0).is_none());
        assert!(window.dialog().is_none());
        window.update(Msg::Properties, &services, "h", 0.0);
        window.update(Msg::CloseProperties, &services, "h", 0.0);
        assert!(window.dialog().is_none());
    }

    #[test]
    fn the_status_is_read_again_every_few_seconds_while_the_window_is_open() {
        let fake = std::sync::Arc::new(crate::gateway::Fake::new());
        let services = Services::new(fake.clone(), crate::wake::Wake::none());
        let mut window = NetworksWindow::default();
        let reads = |fake: &crate::gateway::Fake| {
            fake.calls()
                .iter()
                .filter(|c| c.starts_with("network-status"))
                .count()
        };
        let settle = |window: &mut NetworksWindow| {
            let start = std::time::Instant::now();
            while window.reading() || services.busy("gw") {
                window.poll();
                assert!(start.elapsed() < std::time::Duration::from_secs(5), "stuck");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            window.poll();
        };
        let _ = window.view(&services, Some("gw"), 100.0);
        settle(&mut window);
        assert_eq!(reads(&fake), 1);
        let _ = window.view(&services, Some("gw"), 100.0 + STATUS_EVERY - 0.5);
        settle(&mut window);
        assert_eq!(reads(&fake), 1, "too soon");
        let _ = window.view(&services, Some("gw"), 100.0 + STATUS_EVERY);
        settle(&mut window);
        assert_eq!(reads(&fake), 2);
        // a failure is not retried on the timer; Refresh does
        *fake.network_error.lock().unwrap() = Some("down".into());
        window.update(Msg::Refresh, &services, "gw", 200.0);
        settle(&mut window);
        let _ = window.view(&services, Some("gw"), 300.0);
        settle(&mut window);
        assert_eq!(reads(&fake), 3);
    }

    #[test]
    fn a_pinned_time_format_writes_the_lease_lines() {
        let window = NetworksWindow::default().with_time(utc_time);
        assert_eq!(utc_time(0), "1/1/1970 12:00:00 AM");
        assert!(window.dialog().is_none());
    }

    const LAB: &str = "networks:\n  - name: lab\n    interface: eth1\n    address: 172.16.120.1/22\n    dhcp:\n      range: [172.16.120.100, 172.16.120.200]\n";

    fn services_with(settings: &str) -> (std::sync::Arc<crate::gateway::Fake>, Services) {
        let fake = std::sync::Arc::new(crate::gateway::Fake::new());
        *fake.stored.lock().unwrap() = settings.into();
        let services = Services::new(fake.clone(), crate::wake::Wake::none());
        (fake, services)
    }

    /// the window once the networks are read, with nothing running.
    fn loaded(services: &Services) -> NetworksWindow {
        let mut window = NetworksWindow::default();
        let _ = window.view(services, Some("gw"), 0.0);
        settle(&mut window, services);
        window
    }

    fn settle(window: &mut NetworksWindow, services: &Services) {
        let start = std::time::Instant::now();
        while window.reading() || services.busy("gw") {
            window.poll();
            assert!(start.elapsed() < std::time::Duration::from_secs(5), "stuck");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        window.poll();
    }

    fn typed(window: &mut NetworksWindow, services: &Services, field: Field, text: &str) {
        window.update(Msg::Edit(field, text.into()), services, "gw", 0.0);
    }

    fn button(window: &mut NetworksWindow, services: &Services, label: &str) -> Option<bool> {
        let nodes = window.view(services, Some("gw"), 0.0);
        nodes.iter().find_map(|n| match n {
            Node::Buttons(buttons) => buttons.iter().find(|b| b.label == label).map(|b| b.enabled),
            _ => None,
        })
    }

    #[test]
    fn new_network_opens_on_an_unused_card_with_its_address_and_a_range() {
        let (_fake, services) = services_with("");
        let mut window = loaded(&services);
        assert_eq!(button(&mut window, &services, "New network..."), Some(true));
        assert!(!window.editing());
        window.update(Msg::New, &services, "gw", 0.0);
        assert!(window.editing());
        let form = window.form.as_ref().unwrap();
        assert_eq!(form.existing, None);
        // eth0 is the external network's, so eth1 is the only card to offer
        assert_eq!(
            form.cards
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["eth1"]
        );
        assert_eq!(form.draft.interface, "eth1");
        assert_eq!(form.draft.address, "172.16.120.1/22");
        assert_eq!(form.draft.range_start, "172.16.120.100");
        assert_eq!(form.draft.mtu, "1450");
        let dialog = window.dialog().unwrap();
        assert_eq!(dialog.id, "network-form");
        assert_eq!(dialog.title, "New network");
        assert_eq!(dialog.escape, Some(Msg::CloseForm));
        window.update(Msg::CloseForm, &services, "gw", 0.0);
        assert!(window.dialog().is_none());
    }

    #[test]
    fn selecting_an_unused_card_starts_a_new_network_and_a_used_one_does_nothing() {
        let (_fake, services) = services_with("");
        let mut window = loaded(&services);
        // row 0 is eth0, the external network's; row 1 is eth1
        window.update(Msg::PickInterface(0), &services, "gw", 0.0);
        assert!(!window.editing());
        window.update(Msg::PickInterface(1), &services, "gw", 0.0);
        assert_eq!(window.form.as_ref().unwrap().draft.interface, "eth1");
    }

    #[test]
    fn a_bad_form_cannot_be_saved_and_says_why() {
        let (fake, services) = services_with("");
        let mut window = loaded(&services);
        window.update(Msg::New, &services, "gw", 0.0);
        // no name yet
        assert!(!window.form_edit().unwrap_err().is_empty());
        window.update(Msg::Save, &services, "gw", 0.0);
        let form = window.form.as_ref().unwrap();
        assert!(form
            .error
            .as_deref()
            .is_some_and(|e| e.contains("plain name")));
        typed(&mut window, &services, Field::Name, "lab");
        typed(&mut window, &services, Field::Address, "172.16.120.1");
        assert!(window.form_edit().unwrap_err()[0].contains("must look like"));
        typed(&mut window, &services, Field::Address, "172.16.120.1/22");
        typed(&mut window, &services, Field::RangeEnd, "172.16.200.9");
        assert!(window.form_edit().unwrap_err()[0].contains("range"));
        window.update(Msg::Save, &services, "gw", 0.0);
        settle(&mut window, &services);
        assert!(window.editing());
        assert!(fake.calls().iter().all(|c| !c.starts_with("networks")));
    }

    #[test]
    fn saving_a_new_network_adds_it_applies_and_reads_again() {
        let (fake, services) = services_with("");
        let mut window = loaded(&services);
        window.update(Msg::New, &services, "gw", 0.0);
        typed(&mut window, &services, Field::Name, "lab");
        window.update(Msg::Save, &services, "gw", 0.0);
        settle(&mut window, &services);
        let calls = fake.calls();
        assert!(
            calls.contains(&"networks gw added the network lab on eth1 (172.16.120.1/22)".into()),
            "{calls:?}"
        );
        assert!(calls.contains(&"apply gw [docker,dns]".into()));
        assert!(!window.editing(), "the form closes once it worked");
        let rows = &window.rows().data.as_ref().unwrap().rows;
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["External Network", "lab"]
        );
        assert!(fake.stored.lock().unwrap().contains("name: lab"));
    }

    #[test]
    fn a_failed_save_keeps_the_form_open_with_the_reason() {
        let (fake, services) = services_with("");
        let mut window = loaded(&services);
        window.update(Msg::New, &services, "gw", 0.0);
        typed(&mut window, &services, Field::Name, "lab");
        *fake.fail.lock().unwrap() = Some("the server said no".into());
        window.update(Msg::Save, &services, "gw", 0.0);
        settle(&mut window, &services);
        let form = window.form.as_ref().unwrap();
        assert!(form.error.as_deref().is_some_and(|e| e.contains("said no")));
    }

    #[test]
    fn properties_of_a_lanlord_network_change_it_and_the_external_one_stays_read_only() {
        let (fake, services) = services_with(LAB);
        let mut window = loaded(&services);
        // the external network's tab: the read-only box
        window.update(Msg::Properties, &services, "gw", 0.0);
        assert_eq!(window.dialog().unwrap().id, "network-details");
        window.update(Msg::CloseProperties, &services, "gw", 0.0);
        window.update(Msg::Tab(1), &services, "gw", 0.0);
        window.update(Msg::Properties, &services, "gw", 0.0);
        let dialog = window.dialog().unwrap();
        assert_eq!(
            (dialog.id, dialog.title.as_str()),
            ("network-form", "lab Properties")
        );
        assert_eq!(
            window.form.as_ref().unwrap().draft.range_end,
            "172.16.120.200"
        );
        // nothing is changed yet, and the network's own name and card are no clash
        assert_eq!(window.form_edit().unwrap_err(), ["nothing was changed"]);
        typed(&mut window, &services, Field::Mtu, "1400");
        window.update(Msg::Save, &services, "gw", 0.0);
        settle(&mut window, &services);
        assert!(fake
            .calls()
            .contains(&"networks gw changed the network lab".into()));
        assert!(fake.stored.lock().unwrap().contains("mtu: 1400"));
        assert!(!window.editing());
    }

    #[test]
    fn removing_a_network_asks_first_and_names_what_goes() {
        let (fake, services) = services_with(LAB);
        let mut window = loaded(&services);
        window.update(Msg::Tab(1), &services, "gw", 0.0);
        window.update(Msg::Properties, &services, "gw", 0.0);
        window.update(Msg::Remove, &services, "gw", 0.0);
        let dialog = window.dialog().unwrap();
        assert_eq!(dialog.id, "network-remove");
        let text = dialog
            .body
            .iter()
            .filter_map(|n| match n {
                Node::Text(t) | Node::Alert(t) => Some(t.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert!(text.contains("lab") && text.contains("dnsmasq") && text.contains("static leases"));
        // cancelling goes back to the form and removes nothing
        window.update(Msg::CancelRemove, &services, "gw", 0.0);
        assert_eq!(window.dialog().unwrap().id, "network-form");
        assert!(fake.calls().iter().all(|c| !c.starts_with("networks")));
        window.update(Msg::Remove, &services, "gw", 0.0);
        window.update(Msg::ConfirmRemove, &services, "gw", 0.0);
        settle(&mut window, &services);
        assert!(fake
            .calls()
            .contains(&"networks gw removed the network lab".into()));
        assert!(window.dialog().is_none());
        assert_eq!(window.rows().data.as_ref().unwrap().rows.len(), 1);
    }

    #[test]
    fn an_apply_that_fails_after_the_save_reloads_the_list_and_says_so() {
        let (fake, services) = services_with("");
        let mut window = loaded(&services);
        window.update(Msg::New, &services, "gw", 0.0);
        typed(&mut window, &services, Field::Name, "lab");
        *fake.apply_error.lock().unwrap() = Some("dnsmasq would not start".into());
        window.update(Msg::Save, &services, "gw", 0.0);
        settle(&mut window, &services);
        // stored, and the list shows it, so a second Save cannot add it twice
        assert!(fake.stored.lock().unwrap().contains("name: lab"));
        assert!(!window.editing());
        assert_eq!(window.rows().data.as_ref().unwrap().lan.len(), 1);
        let (error, text) = window.rows().note.clone().unwrap();
        assert!(error);
        assert!(text.starts_with("Saved, but applying failed: dnsmasq would not start."));
        assert!(text.ends_with("Fix it and click Apply."));
    }

    #[test]
    fn a_network_that_is_still_used_cannot_be_removed() {
        let settings = format!("{LAB}nt4-domain:\n  network: lab\n");
        let (fake, services) = services_with(&settings);
        let mut window = loaded(&services);
        window.update(Msg::Tab(1), &services, "gw", 0.0);
        window.update(Msg::Properties, &services, "gw", 0.0);
        window.update(Msg::Remove, &services, "gw", 0.0);
        let dialog = window.dialog().unwrap();
        let said = dialog.body.iter().any(
            |n| matches!(n, Node::Warning(t) if t.contains("the nt4-domain network") && t.contains("lab")),
        );
        assert!(said);
        let enabled = dialog.body.iter().find_map(|n| match n {
            Node::Buttons(b) => b.iter().find(|b| b.label == "Remove network"),
            _ => None,
        });
        assert_eq!(enabled.map(|b| b.enabled), Some(false));
        window.update(Msg::ConfirmRemove, &services, "gw", 0.0);
        assert!(fake.calls().iter().all(|c| !c.starts_with("networks")));
    }

    #[test]
    fn clearing_the_range_of_a_network_with_reservations_warns_about_them() {
        let settings = LAB.replace(
            "172.16.120.200]\n",
            "172.16.120.200]\n      hosts:\n        - name: box\n          mac: 02:00:00:00:00:01\n          address: 172.16.120.10\n",
        );
        let (_fake, services) = services_with(&settings);
        let mut window = loaded(&services);
        window.update(Msg::Tab(1), &services, "gw", 0.0);
        window.update(Msg::Properties, &services, "gw", 0.0);
        let warns = |window: &NetworksWindow| {
            window.dialog().unwrap().body.iter().any(
                |n| matches!(n, Node::Warning(t) if t.contains("1 static lease") && t.contains("removes")),
            )
        };
        assert!(!warns(&window));
        typed(&mut window, &services, Field::RangeStart, "");
        typed(&mut window, &services, Field::RangeEnd, "");
        assert!(warns(&window));
    }

    #[test]
    fn nothing_is_offered_while_the_networks_are_unread_or_every_card_is_used() {
        let (_fake, services) = services_with(LAB);
        let mut window = NetworksWindow::default();
        window.update(Msg::New, &services, "gw", 0.0);
        window.update(Msg::PickInterface(1), &services, "gw", 0.0);
        assert!(!window.editing());
        let mut window = loaded(&services);
        // lab has eth1 and eth0 is the external network's
        assert_eq!(
            button(&mut window, &services, "New network..."),
            Some(false)
        );
        window.update(Msg::New, &services, "gw", 0.0);
        assert!(!window.editing());
    }
}
