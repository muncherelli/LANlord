//! the Networks window: the server's external network and the networks lanlord runs, in the
//! style of Network and Dial-up Connections. each network has a tab showing what Windows shows
//! in a connection's Status box (link, speed and traffic), and Details... opens the Network
//! Connection Details list. New network... adds a lanlord network on a card that no network uses,
//! and Properties of a lanlord network changes or removes it, the way `lanlord networks add`,
//! `set` and `remove` do; the external network is only shown.

use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use lanlord_model::clock;
use lanlord_model::internet::host_line;
use lanlord_model::internet::{parse_duration, parse_until_local, Expiry};
use lanlord_model::lan_edit::{external_interface, network_users, Edit};
use lanlord_model::lan_networks::{self, LanNetwork};
use lanlord_model::lockout::{self, Lockout};
use lanlord_model::netstatus::InterfaceStatus;
use lanlord_model::networks::{self, Draft, NetworkRow, Role};
use lanlord_model::server::Iface;
use lanlord_model::settings::Settings;
use lanlord_model::ssh_access::{Change as SshChange, SshAccess};
use lanlord_model::validate::{cidr_partial, ipv4_partial, mac_partial};

use crate::ask::AskPassword;
use crate::gateway::Gateway;
use crate::remote::Remote;
use crate::services::Services;
use crate::view::{self, Button, Dialog, HeldFor, Node, Size, Table};

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

/// a text field of the form that allows a computer to ssh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SshField {
    Mac,
    Name,
    When,
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
    /// switch ssh to the server on or off for the network of the tab that is open.
    SetSsh(bool),
    /// pick a computer of the list allowed to ssh on its own.
    SshPick(usize),
    /// open the form that allows one more computer.
    SshAdd,
    /// a field of that form was edited.
    SshEdit(SshField, String),
    /// the form's "for how long" choice: 0 always, 1 for a while, 2 until a time.
    SshHow(usize),
    /// check the form, store the grant and apply.
    SshSave,
    SshClose,
    /// take the picked computer's grant away.
    SshRemove,
    /// go on with the ssh change that would cut this connection off.
    ConfirmLockout,
    /// leave the settings as they are.
    CancelLockout,
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
    /// the server's time zone, as its name and its offset from utc in seconds: leases are the
    /// gateway's, so their times are written in its zone. `None` when its clock was not read.
    pub zone: Option<(String, i32)>,
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

/// the form that allows one computer to ssh to the server, for good or for a while.
struct SshForm {
    /// the network the computer is on.
    network: String,
    mac: String,
    name: String,
    /// 0 always, 1 for a while (`when` is a length like 1h), 2 until a time (`when` is
    /// like 2030-01-31 09:00).
    how: usize,
    when: String,
    touched: bool,
    error: Option<String>,
}

impl SshForm {
    /// the change this form asks for, or why it cannot be made. `now` is unix seconds.
    fn change(&self, now: i64) -> Result<SshChange, String> {
        let expiry = match self.how {
            0 => Expiry::Never,
            1 => Expiry::At(now + parse_duration(&self.when).map_err(|e| e.to_string())?),
            _ => Expiry::At(parse_until_local(&self.when).map_err(|e| e.to_string())?),
        };
        let name = self.name.trim();
        let change = SshChange::Allow {
            network: self.network.clone(),
            mac: self.mac.trim().to_string(),
            name: (!name.is_empty()).then(|| name.to_string()),
            expiry,
        };
        change
            .check(now)
            .map_err(|(field, why)| format!("{field} {why}"))?;
        Ok(change)
    }
}

/// an ssh change that would cut this connection off, held until the person says yes.
#[derive(Clone)]
struct Held {
    host: String,
    lockout: Lockout,
    change: SshChange,
    doing: String,
    done: String,
}

/// seconds since 1970 on this computer's clock.
pub(crate) fn unix_now() -> i64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// read the networks, the interfaces and the lanlord networks of the settings.
fn read(gateway: &dyn Gateway, target: &str, ask: AskPassword) -> Result<Networks> {
    let facts = gateway.server_facts(target, ask)?;
    // networks still show without settings, or without the key to read them
    let settings = gateway
        .load_settings(target, ask)
        .unwrap_or_else(|_| Settings::empty());
    let leases = gateway.leases(target, ask).ok().map(|l| l.len());
    let zone = gateway
        .clock_state(target)
        .ok()
        .map(|c| (c.timezone, c.offset_secs));
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
        zone,
    })
}

/// the width of the Connection box beside the traffic.
const CONNECTION_WIDTH: f32 = 250.0;

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
    /// the form that allows a computer to ssh.
    ssh_form: Option<SshForm>,
    /// the computer picked in the list allowed to ssh, of the tab that is open.
    ssh_pick: Option<usize>,
    /// the ssh change a confirmation is open for, because it would cut this connection off.
    held: Option<Held>,
    /// set by the job of an ssh change that was held instead of made.
    cut_off: Arc<Mutex<Option<Held>>>,
    /// a save or a removal is on its way, so its form stays open until it is known how it went.
    saving: bool,
    /// a change was stored since `take_saved` was last asked.
    saved_now: bool,
    /// set by the job of a save that was stored but could not be applied.
    warning: Arc<Mutex<Option<String>>>,
    /// when the status was last asked for, in seconds on the caller's clock.
    status_at: f64,
    /// writes a unix time for the lease lines; the server's time zone when `None`.
    time: Option<fn(i64) -> String>,
}

impl NetworksWindow {
    /// write lease times with `time` rather than in the server's time zone, so a test or a
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
        self.form.is_some()
            || self.confirm.is_some()
            || self.ssh_form.is_some()
            || self.held.is_some()
    }

    pub fn poll(&mut self) {
        let arrived = self.rows.poll();
        self.status.poll();
        if self.saving && !self.rows.busy() {
            self.saving = false;
            self.confirm = None;
            self.ssh_pick = None;
            let warning = self.warning.lock().ok().and_then(|mut w| w.take());
            let cut_off = self.cut_off.lock().ok().and_then(|mut slot| slot.take());
            if let Some(held) = cut_off {
                // nothing was changed: the person is asked first
                self.held = Some(held);
                self.rows.say(false, "waiting for your answer");
            } else if let Some(text) = warning {
                // the change is stored and the list read again; only the apply failed
                self.form = None;
                self.saved_now = true;
                self.rows.say(true, text);
            } else if arrived {
                self.form = None;
                self.ssh_form = None;
                self.saved_now = true;
            } else if let (Some(form), Some((true, text))) = (&mut self.form, &self.rows.note) {
                form.error = Some(text.clone());
            } else if let (Some(form), Some((true, text))) = (&mut self.ssh_form, &self.rows.note) {
                form.error = Some(text.clone());
            }
        }
    }

    /// a network was added, changed or removed, once: the settings others cached are old.
    pub fn take_saved(&mut self) -> bool {
        std::mem::take(&mut self.saved_now)
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

    /// store the ssh switch of the network of the open tab, apply the docker and dns modules (the
    /// firewall) and read the networks again.
    fn switch_ssh(&mut self, services: &Services, host: &str, on: bool) {
        let Some(row) = self.chosen().filter(|r| r.role == Role::Lanlord) else {
            return;
        };
        let change = SshChange::Network {
            network: row.name.clone(),
            on,
        };
        let (doing, done) = if on {
            (
                "allowing ssh to the server and applying...",
                "ssh to this server is on",
            )
        } else {
            (
                "stopping ssh to the server and applying...",
                "ssh to this server is off",
            )
        };
        self.change_ssh(services, host, change, doing, done);
    }

    /// store one ssh change, apply the docker and dns modules and read the networks again. a
    /// change that takes access away from the network this connection comes from is held first.
    fn change_ssh(
        &mut self,
        services: &Services,
        host: &str,
        change: SshChange,
        doing: &str,
        done: &str,
    ) {
        self.send_ssh(services, host, change, doing, done, false);
    }

    fn send_ssh(
        &mut self,
        services: &Services,
        host: &str,
        change: SshChange,
        doing: &str,
        done: &str,
        confirmed: bool,
    ) {
        let target = host.to_string();
        let cut_off = self.cut_off.clone();
        let (said_doing, said_done) = (doing.to_string(), done.to_string());
        let job = services.spawn(host, move |gateway, ask| {
            if !confirmed && lockout::takes_access(&change) {
                // no ssh connection at all (the browser) is nothing to cut; one that cannot be
                // placed is asked about, like one that would be cut
                let held = match gateway.client_spot(&target) {
                    Ok(None) => None,
                    Ok(Some(spot)) => {
                        let networks = read(gateway, &target, ask)?;
                        lockout::after(&spot, &networks.lan, &change, unix_now())
                            .ok()
                            .flatten()
                            .map(|held| (held, networks))
                    }
                    Err(_) => Some((Lockout::unknown(None), read(gateway, &target, ask)?)),
                };
                if let Some((lockout, networks)) = held {
                    if let Ok(mut slot) = cut_off.lock() {
                        *slot = Some(Held {
                            host: target.clone(),
                            lockout,
                            change: change.clone(),
                            doing: said_doing.clone(),
                            done: said_done.clone(),
                        });
                        return Ok(networks);
                    }
                }
            }
            gateway.change_ssh_access(&target, &change, ask)?;
            read(gateway, &target, ask)
        });
        self.saving = self.rows.track(host, job, doing, done, false);
    }

    /// the computers of the open tab that are allowed to ssh on their own.
    fn ssh_hosts(&self) -> Vec<lanlord_model::internet::AllowedHost> {
        self.chosen()
            .and_then(|r| r.ssh.as_ref())
            .map_or_else(Vec::new, |s| s.allow.clone())
    }

    fn ssh_update(&mut self, msg: Msg, services: &Services, host: &str) {
        match msg {
            Msg::SshPick(i) => self.ssh_pick = Some(i).filter(|i| *i < self.ssh_hosts().len()),
            Msg::SshAdd if self.can_edit(services, host) => {
                let network = self
                    .chosen()
                    .filter(|r| r.role == Role::Lanlord)
                    .map(|r| r.name.clone());
                if let Some(network) = network {
                    self.ssh_form = Some(SshForm {
                        network,
                        mac: String::new(),
                        name: String::new(),
                        how: 0,
                        when: String::new(),
                        touched: false,
                        error: None,
                    });
                }
            }
            Msg::SshEdit(field, text) => {
                if let Some(form) = &mut self.ssh_form {
                    match field {
                        SshField::Mac => form.mac = text,
                        SshField::Name => form.name = text,
                        SshField::When => form.when = text,
                    }
                    form.touched = true;
                    form.error = None;
                }
            }
            Msg::SshHow(how) => {
                if let Some(form) = &mut self.ssh_form {
                    form.how = how.min(2);
                    form.when.clear();
                    form.error = None;
                }
            }
            Msg::SshSave if self.can_edit(services, host) => {
                let made = self.ssh_form.as_ref().map(|f| f.change(unix_now()));
                match made {
                    Some(Ok(change)) => self.change_ssh(
                        services,
                        host,
                        change,
                        "allowing the computer and applying...",
                        "computer allowed to ssh",
                    ),
                    Some(Err(why)) => {
                        if let Some(form) = &mut self.ssh_form {
                            form.touched = true;
                            form.error = Some(why);
                        }
                    }
                    None => {}
                }
            }
            Msg::SshClose => self.ssh_form = None,
            Msg::SshRemove if self.can_edit(services, host) => {
                let picked = self.ssh_pick.and_then(|i| self.ssh_hosts().get(i).cloned());
                let network = self.chosen().map(|r| r.name.clone());
                if let (Some(entry), Some(network)) = (picked, network) {
                    let change = SshChange::Deny {
                        network,
                        mac: entry.mac,
                    };
                    self.change_ssh(
                        services,
                        host,
                        change,
                        "taking the computer's ssh access away and applying...",
                        "computer no longer allowed to ssh",
                    );
                }
            }
            _ => {}
        }
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
            zone: None,
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
        let ssh_free = self.can_edit(services, host);
        let pick = self.ssh_pick;
        let body = sized(chosen, status, error.as_deref(), ssh_free, pick);
        let others: Vec<(usize, Vec<Node<Msg>>)> = rows
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != self.selected)
            .map(|(i, r)| {
                (
                    i,
                    sized(r, self.status_of(r), error.as_deref(), ssh_free, pick),
                )
            })
            .collect();
        let tabs = Node::Tabs {
            titles,
            key: view::pages_fingerprint(self.selected, &body, &others),
            current: self.selected,
            pick: Msg::Tab,
            body,
            others,
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
            Msg::Tab(i) => {
                self.selected = i;
                self.ssh_pick = None;
            }
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
            Msg::SetSsh(on) if self.can_edit(services, host) => self.switch_ssh(services, host, on),
            Msg::SshPick(_)
            | Msg::SshAdd
            | Msg::SshEdit(..)
            | Msg::SshHow(_)
            | Msg::SshSave
            | Msg::SshClose
            | Msg::SshRemove => self.ssh_update(msg, services, host),
            Msg::ConfirmLockout if self.can_edit(services, host) => {
                if let Some(held) = self.held.take() {
                    self.send_ssh(services, host, held.change, &held.doing, &held.done, true);
                }
            }
            Msg::CancelLockout => {
                self.held = None;
                self.rows.say(false, "nothing was changed");
            }
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
            | Msg::SetSsh(_)
            | Msg::ConfirmLockout
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
            let zone = self.rows.data.as_ref().and_then(|n| n.zone.clone());
            return Some(match self.time {
                Some(time) => connection_dialog(status, &time),
                None => connection_dialog(status, &|secs| zoned_time(secs, zone.as_ref())),
            });
        }
        if let Some(held) = &self.held {
            return Some(self.lockout_dialog(held));
        }
        if let Some(form) = &self.ssh_form {
            return Some(self.ssh_dialog(form));
        }
        if let Some(name) = &self.confirm {
            return Some(self.remove_dialog(name));
        }
        if let Some(form) = &self.form {
            return Some(self.form_dialog(form));
        }
        self.details.as_ref().map(properties_dialog)
    }

    /// the question before an ssh change that would end this connection: what happens, how to
    /// keep the connection, and the choice to go on or leave things as they are.
    fn lockout_dialog(&self, held: &Held) -> Dialog<Msg> {
        let busy = self.rows.busy();
        let mut sentence = held.lockout.sentence();
        if let Some(first) = sentence.get_mut(..1) {
            first.make_ascii_uppercase();
        }
        let mut body = vec![Node::Alert(match &held.lockout.network {
            Some(network) => format!("Cut off this connection ({network})?"),
            None => "Can't tell where you are connected from".into(),
        })];
        body.push(Node::Gap(4.0));
        body.push(Node::Text(format!("{sentence}.")));
        body.push(Node::Gap(4.0));
        body.push(Node::Text(if held.lockout.is_unknown() {
            "Go on only if you can still reach the server another way.".into()
        } else {
            "To keep it, allow this computer (or the whole network) first; the command line \
             does the same with:"
                .into()
        }));
        let keep = held
            .lockout
            .commands(&held.host)
            .into_iter()
            .map(Node::Text)
            .collect::<Vec<_>>();
        body.extend(keep);
        body.push(Node::Gap(6.0));
        body.push(Node::Buttons(vec![
            Button::new("Cancel", !busy, Msg::CancelLockout),
            Button::new("Go on anyway", !busy, Msg::ConfirmLockout),
        ]));
        Dialog {
            id: "ssh-lockout",
            title: "Ssh to this server".into(),
            body,
            escape: Some(Msg::CancelLockout),
        }
    }

    /// the form that allows one computer to ssh to the server: its mac, a name for people and
    /// how long, like the command line's `ssh NAME allow MAC`.
    fn ssh_dialog(&self, form: &SshForm) -> Dialog<Msg> {
        let busy = self.rows.busy();
        let input = |field: SshField, value: &str, hint: &str| Node::Input {
            value: value.to_string(),
            size: Size::FillLess(10.0),
            focus: false,
            hint: hint.to_string(),
            edit: Box::new(move |text| Msg::SshEdit(field, text)),
            enter: None,
        };
        let labelled = |label: &str, body: Node<Msg>| Node::Labelled {
            label: label.into(),
            body: vec![body],
        };
        let how = Node::Choice {
            id: "ssh-how",
            current: ["Always", "For a while", "Until a time"][form.how.min(2)].into(),
            width: 160.0,
            options: vec![
                (0, "Always".into()),
                (1, "For a while".into()),
                (2, "Until a time".into()),
            ],
            pick: Msg::SshHow,
        };
        let mut body = vec![
            Node::Text(format!(
                "Let one computer on {} reach this server over ssh, even while the network is off.",
                form.network
            )),
            Node::Gap(6.0),
            labelled(
                "MAC address:",
                input(SshField::Mac, &form.mac, "like 02:00:00:00:00:0a"),
            ),
            labelled("Name:", input(SshField::Name, &form.name, "optional")),
            labelled("Allowed:", how),
        ];
        match form.how {
            1 => body.push(labelled(
                "For:",
                input(SshField::When, &form.when, "like 30m, 1h or 1d12h"),
            )),
            2 => body.push(labelled(
                "Until:",
                input(SshField::When, &form.when, "like 2030-01-31 09:00"),
            )),
            _ => {}
        }
        body.push(Node::Gap(6.0));
        let problem = form.error.clone().or_else(|| {
            form.touched
                .then(|| form.change(unix_now()).err())
                .flatten()
        });
        // a mac address still being typed is not called wrong until the focus leaves it: the
        // line is about the first of the text fields, the name and the time come after it
        let after_mac = if matches!(form.how, 1 | 2) { 2 } else { 1 };
        body.push(match &problem {
            Some(text) if text.starts_with("mac ") => Node::HeldError {
                text: text.clone(),
                partial: mac_partial(form.mac.trim()),
                field: HeldFor::Above(after_mac),
            },
            _ => Node::Warning(problem.clone().unwrap_or_default()),
        });
        body.push(Node::Buttons(vec![
            Button::new("Cancel", !busy, Msg::SshClose),
            Button::new(
                "Allow",
                !busy && form.touched && problem.is_none(),
                Msg::SshSave,
            ),
        ]));
        Dialog {
            id: "ssh-allow",
            title: "Allow a computer to ssh".into(),
            body,
            escape: Some(Msg::SshClose),
        }
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
        // two boxes and the "to" between them, each wide enough for the longest dotted quad
        let half = |field: Field, value: &str, hint: &str| match input(field, value, hint) {
            Node::Input {
                value,
                focus,
                hint,
                edit,
                enter,
                ..
            } => Node::Input {
                value,
                size: Size::Text(LONGEST_ADDRESS),
                focus,
                hint,
                edit,
                enter,
            },
            other => other,
        };
        let range = Node::Row(vec![
            half(Field::RangeStart, &draft.range_start, "start"),
            Node::Text(" to ".into()),
            half(Field::RangeEnd, &draft.range_end, "end"),
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
        ];
        // what is typed into an address field is not called wrong while it could still become
        // one: each such problem sits under its own field and waits for the focus to leave it
        let (mut address_lines, mut start_lines, mut end_lines, mut shown) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for problem in problems {
            let (lines, partial, above) = if problem.contains(".address:") {
                (&mut address_lines, cidr_partial(draft.address.trim()), 0)
            } else if problem.contains(".dhcp.range[0]:") {
                (&mut start_lines, ipv4_partial(draft.range_start.trim()), 1)
            } else if problem.contains(".dhcp.range[1]:") {
                (&mut end_lines, ipv4_partial(draft.range_end.trim()), 0)
            } else {
                shown.push(problem);
                continue;
            };
            lines.push(Node::HeldError {
                text: problem,
                partial,
                field: HeldFor::Above(above),
            });
        }
        body.push(labelled(
            "Address",
            input(Field::Address, &draft.address, "like 172.16.120.1/22"),
        ));
        body.extend(address_lines);
        body.push(labelled("DHCP range", range));
        body.extend(start_lines);
        body.extend(end_lines);
        body.extend([
            labelled("Lease time", input(Field::Lease, &draft.lease, "like 7d")),
            labelled("DNS zone", input(Field::Zone, &draft.zone, "the name")),
            labelled("MTU", input(Field::Mtu, &draft.mtu, "the card's own")),
            Node::Text("Leave the DHCP range empty for a network without DHCP.".into()),
        ]);
        let leases = self.leases_lost(form);
        if leases > 0 {
            body.push(Node::Warning(format!(
                "Clearing the DHCP range removes the {leases} static lease{} of this network.",
                if leases == 1 { "" } else { "s" }
            )));
        }
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

fn connection_dialog(status: &InterfaceStatus, time: &dyn Fn(i64) -> String) -> Dialog<Msg> {
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

/// a network's page, as tall as the page of a lanlord network would be, so the window keeps its
/// size from one tab to the next and when a network is added or removed, as a property sheet does.
fn sized(
    row: &NetworkRow,
    status: Option<&InterfaceStatus>,
    error: Option<&str>,
    ssh_free: bool,
    pick: Option<usize>,
) -> Vec<Node<Msg>> {
    let lanlord = NetworkRow {
        role: Role::Lanlord,
        // the switch is on every LANlord page, so the window has room for it on each tab
        ssh: row.ssh.clone().or_else(|| Some(SshAccess::default())),
        ..row.clone()
    };
    // what the page shows: the row, the status text and the error text
    let key = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (row.role as u8, row.details(), &row.link).hash(&mut hasher);
        status
            .map(|s| (s.connection.text(), s.speed()))
            .hash(&mut hasher);
        error.hash(&mut hasher);
        hasher.finish()
    };
    vec![Node::SameSize {
        key,
        body: page(row, status, error, ssh_free, pick),
        others: vec![page(&lanlord, status, error, ssh_free, pick)],
    }]
}

/// one network's tab: Connection and Activity like a connection's Status box, and the lanlord
/// network's dhcp.
fn page(
    row: &NetworkRow,
    status: Option<&InterfaceStatus>,
    error: Option<&str>,
    ssh_free: bool,
    pick: Option<usize>,
) -> Vec<Node<Msg>> {
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
            (Some(CONNECTION_WIDTH), vec![connection]),
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
        nodes.push(lanlord_network(row, ssh_free, pick));
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

/// the text of the longest address a DHCP range box has to show.
const LONGEST_ADDRESS: &str = "255.255.255.255";

fn lanlord_network(row: &NetworkRow, ssh_free: bool, pick: Option<usize>) -> Node<Msg> {
    let dhcp = row.dhcp.clone().unwrap_or_else(|| "not served".into());
    let clients = row.leases.map_or("-".to_string(), |n| n.to_string());
    let mut body = vec![
        Node::field("DHCP range:", dhcp),
        Node::field("Clients with a lease:", clients),
    ];
    if let Some(ssh) = &row.ssh {
        // the switch for the whole network, and below it the computers allowed on their own
        body.push(Node::Gap(4.0));
        body.push(Node::Check {
            label: "Allow ssh to this server".into(),
            checked: ssh.enabled,
            enabled: ssh_free,
            toggle: Msg::SetSsh(!ssh.enabled),
        });
        // the computers allowed on their own, a list with Add and Remove under it
        body.push(Node::Text("Computers allowed to ssh on their own:".into()));
        body.push(Node::List {
            id: "ssh-allowed",
            min_height: 18.0 * 3.0,
            items: ssh.allow.iter().map(host_line).collect(),
            selected: pick.filter(|i| *i < ssh.allow.len()),
            empty: "no computers".into(),
            select: Msg::SshPick,
        });
        body.push(Node::Toolbar(vec![
            Button::new("Add...", ssh_free, Msg::SshAdd),
            Button::new(
                "Remove",
                ssh_free && pick.is_some_and(|i| i < ssh.allow.len()),
                Msg::SshRemove,
            ),
        ]));
    }
    Node::Group {
        title: "LANlord network".into(),
        body,
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

/// a unix time in UTC, written the way `zoned_time` writes it.
pub fn utc_time(secs: i64) -> String {
    zoned_time(secs, None)
}

/// a unix time in the server's `zone` (its name and its offset from utc in seconds), with the
/// offset written out: `2026-11-20 14:03:09 UTC-05:00`. with no zone known it is in UTC and says
/// so. the name stays out: the box is narrow, and the offset says which time it is.
fn zoned_time(secs: i64, zone: Option<&(String, i32)>) -> String {
    let offset = zone.map_or(0, |(_, offset)| *offset);
    let text = clock::local_text(secs, offset);
    // a time that can't be written is blank, not a bare zone
    if text.is_empty() {
        return text;
    }
    match zone {
        Some(_) => format!("{text} UTC{}", clock::offset_text(offset)),
        None => format!("{text} UTC"),
    }
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
        let zone = ("Etc/UTC".to_string(), 3600);
        assert_eq!(zoned_time(i64::MAX, Some(&zone)), "");
        assert_eq!(zoned_time(i64::MAX, None), "");
        assert_eq!(zoned_time(i64::MIN, Some(&("x".to_string(), -3600))), "");
    }

    #[test]
    fn a_lease_time_names_whose_zone_it_is_in() {
        let zone = ("America/New_York".to_string(), -5 * 3600);
        assert_eq!(
            zoned_time(1_795_000_000, Some(&zone)),
            "2026-11-18 06:06:40 UTC-05:00"
        );
        assert_eq!(zoned_time(0, None), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc_time(0), "1970-01-01 00:00:00 UTC");
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
        assert!(window.take_saved(), "the other windows are told");
        assert!(!window.take_saved(), "only once");
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
    fn the_ssh_switch_of_a_network_is_stored_applied_and_shown() {
        let allowed =
            format!("{LAB}    ssh_allow:\n      - {{mac: \"02:00:00:00:00:0a\", name: dana-pc}}\n");
        let (fake, services) = services_with(&allowed);
        let mut window = loaded(&services);
        window.update(Msg::Tab(1), &services, "gw", 0.0);
        // the switch (as the toggle it would send) and the computers allowed on their own
        let shown = |window: &NetworksWindow| {
            let Node::Group { body, .. } = lanlord_network(window.chosen().unwrap(), true, None)
            else {
                panic!("a group");
            };
            let toggle = body.iter().find_map(|n| match n {
                Node::Check { label, toggle, .. } if label == "Allow ssh to this server" => {
                    Some(toggle.clone())
                }
                _ => None,
            });
            (toggle, body.len())
        };
        let (toggle, rows) = shown(&window);
        assert_eq!(toggle, Some(Msg::SetSsh(true)));
        // dhcp, clients, the gap, the switch, the list's heading, the list and its buttons
        assert_eq!(rows, 7);
        window.update(Msg::SetSsh(true), &services, "gw", 0.0);
        settle(&mut window, &services);
        assert!(fake
            .calls()
            .iter()
            .any(|c| c.starts_with("ssh-access gw turned ssh")));
        assert!(fake.stored.lock().unwrap().contains("ssh: true"));
        assert_eq!(shown(&window).0, Some(Msg::SetSsh(false)));
    }

    /// say the connection to the gateway comes from the lab network, from this mac.
    fn connected_from(fake: &crate::gateway::Fake, interface: &str, mac: Option<&str>) {
        *fake.spot.lock().unwrap() = Some(lanlord_model::lockout::Spot {
            address: "192.0.2.50".into(),
            interface: interface.into(),
            mac: mac.map(str::to_string),
            routed: false,
        });
    }

    fn asked(fake: &crate::gateway::Fake) -> bool {
        fake.calls().iter().any(|c| c.starts_with("ssh-access"))
    }

    #[test]
    fn turning_ssh_off_on_the_network_this_connection_is_on_asks_first_and_names_it() {
        let (fake, services) = services_with(&format!("{LAB}    ssh: true\n"));
        connected_from(&fake, "eth1", Some("02:00:00:00:00:0a"));
        let mut window = loaded(&services);
        window.update(Msg::Tab(1), &services, "gw", 0.0);
        window.update(Msg::SetSsh(false), &services, "gw", 0.0);
        settle(&mut window, &services);
        // nothing was stored or applied yet
        assert!(!asked(&fake));
        assert!(fake.stored.lock().unwrap().contains("ssh: true"));
        assert!(window.editing());
        let dialog = window.dialog().unwrap();
        assert_eq!(dialog.id, "ssh-lockout");
        let names_lab = dialog
            .body
            .iter()
            .any(|n| matches!(n, Node::Alert(t) if t.contains("lab")));
        assert!(names_lab);
        let says_mac = dialog
            .body
            .iter()
            .any(|n| matches!(n, Node::Text(t) if t.contains("02:00:00:00:00:0a")));
        assert!(says_mac);
        // yes: the change is made
        window.update(Msg::ConfirmLockout, &services, "gw", 0.0);
        settle(&mut window, &services);
        assert!(asked(&fake));
        assert!(!fake.stored.lock().unwrap().contains("ssh: true"));
        assert!(window.dialog().is_none());
    }

    #[test]
    fn a_connection_that_cannot_be_placed_asks_first_in_other_words() {
        let places: [fn(&crate::gateway::Fake); 2] = [
            // the gateway found an address but no interface (an fe80 client, say)
            |fake| connected_from(fake, "", Some("02:00:00:00:00:0a")),
            // or could not be asked at all
            |fake| *fake.spot_fails.lock().unwrap() = true,
        ];
        for place in places {
            let (fake, services) = services_with(&format!("{LAB}    ssh: true\n"));
            place(&fake);
            let mut window = loaded(&services);
            window.update(Msg::Tab(1), &services, "gw", 0.0);
            window.update(Msg::SetSsh(false), &services, "gw", 0.0);
            settle(&mut window, &services);
            assert!(!asked(&fake));
            let dialog = window.dialog().unwrap();
            assert_eq!(dialog.id, "ssh-lockout");
            let says = |text: &str| {
                dialog
                    .body
                    .iter()
                    .any(|n| matches!(n, Node::Alert(t) | Node::Text(t) if t.contains(text)))
            };
            assert!(says("Can't tell where you are connected from"));
            assert!(says("an't tell which network"));
            // no command is offered: none would be the right one
            assert!(!says("lanlord networks"));
            window.update(Msg::ConfirmLockout, &services, "gw", 0.0);
            settle(&mut window, &services);
            assert!(asked(&fake));
        }
    }

    #[test]
    fn cancelling_the_question_leaves_the_settings_alone() {
        let (fake, services) = services_with(&format!("{LAB}    ssh: true\n"));
        connected_from(&fake, "eth1", None);
        let mut window = loaded(&services);
        window.update(Msg::Tab(1), &services, "gw", 0.0);
        window.update(Msg::SetSsh(false), &services, "gw", 0.0);
        settle(&mut window, &services);
        assert!(window.dialog().is_some());
        window.update(Msg::CancelLockout, &services, "gw", 0.0);
        assert!(window.dialog().is_none());
        assert!(!window.editing());
        assert!(!asked(&fake));
        assert!(fake.stored.lock().unwrap().contains("ssh: true"));
        // a confirmation with nothing held does nothing
        window.update(Msg::ConfirmLockout, &services, "gw", 0.0);
        settle(&mut window, &services);
        assert!(!asked(&fake));
    }

    #[test]
    fn denying_the_computer_this_connection_is_from_asks_first_but_another_one_does_not() {
        let allowed =
            format!("{LAB}    ssh_allow:\n      - {{mac: \"02:00:00:00:00:0a\", name: dana-pc}}\n      - {{mac: \"02:00:00:00:00:0b\"}}\n");
        let (fake, services) = services_with(&allowed);
        connected_from(&fake, "eth1", Some("02:00:00:00:00:0a"));
        let mut window = loaded(&services);
        window.update(Msg::Tab(1), &services, "gw", 0.0);
        // the second computer is not this one
        window.update(Msg::SshPick(1), &services, "gw", 0.0);
        window.update(Msg::SshRemove, &services, "gw", 0.0);
        settle(&mut window, &services);
        assert!(asked(&fake));
        assert!(window.dialog().is_none());
        // the first is
        let sent = |fake: &crate::gateway::Fake| {
            fake.calls()
                .iter()
                .filter(|c| c.starts_with("ssh-access"))
                .count()
        };
        let before = sent(&fake);
        window.update(Msg::SshPick(0), &services, "gw", 0.0);
        window.update(Msg::SshRemove, &services, "gw", 0.0);
        settle(&mut window, &services);
        assert_eq!(sent(&fake), before, "nothing was sent");
        assert_eq!(window.dialog().map(|d| d.id), Some("ssh-lockout"));
    }

    #[test]
    fn no_question_without_a_known_place_or_on_another_network_or_for_a_change_that_gives_access() {
        for spot in [None, Some("eth9")] {
            let (fake, services) = services_with(&format!("{LAB}    ssh: true\n"));
            if let Some(interface) = spot {
                connected_from(&fake, interface, Some("02:00:00:00:00:0a"));
            }
            let mut window = loaded(&services);
            window.update(Msg::Tab(1), &services, "gw", 0.0);
            window.update(Msg::SetSsh(false), &services, "gw", 0.0);
            settle(&mut window, &services);
            assert!(asked(&fake), "{spot:?}");
            assert!(window.dialog().is_none());
        }
        let (fake, services) = services_with(LAB);
        connected_from(&fake, "eth1", Some("02:00:00:00:00:0a"));
        let mut window = loaded(&services);
        window.update(Msg::Tab(1), &services, "gw", 0.0);
        window.update(Msg::SetSsh(true), &services, "gw", 0.0);
        settle(&mut window, &services);
        assert!(asked(&fake));
        assert!(window.dialog().is_none());
    }

    fn type_ssh(window: &mut NetworksWindow, services: &Services, mac: &str, name: &str) {
        window.update(Msg::SshAdd, services, "gw", 0.0);
        window.update(Msg::SshEdit(SshField::Mac, mac.into()), services, "gw", 0.0);
        window.update(
            Msg::SshEdit(SshField::Name, name.into()),
            services,
            "gw",
            0.0,
        );
    }

    #[test]
    fn a_computer_is_allowed_to_ssh_from_the_form_and_taken_away_again() {
        let (fake, services) = services_with(LAB);
        let mut window = loaded(&services);
        window.update(Msg::Tab(1), &services, "gw", 0.0);
        type_ssh(&mut window, &services, "02:00:00:00:00:0b", "dana-pc");
        assert_eq!(window.dialog().unwrap().id, "ssh-allow");
        window.update(Msg::SshSave, &services, "gw", 0.0);
        settle(&mut window, &services);
        assert!(window.dialog().is_none());
        assert!(fake.calls().iter().any(|c| c.starts_with("ssh-access gw")));
        let stored = fake.stored.lock().unwrap().clone();
        assert!(stored.contains("02:00:00:00:00:0b") && stored.contains("dana-pc"));
        assert_eq!(window.ssh_hosts().len(), 1);
        window.update(Msg::SshPick(0), &services, "gw", 0.0);
        window.update(Msg::SshRemove, &services, "gw", 0.0);
        settle(&mut window, &services);
        assert!(window.ssh_hosts().is_empty());
        assert!(!fake.stored.lock().unwrap().contains("02:00:00:00:00:0b"));
    }

    #[test]
    fn the_ssh_form_refuses_a_bad_address_and_a_bad_time_and_can_be_cancelled() {
        let (fake, services) = services_with(LAB);
        let mut window = loaded(&services);
        window.update(Msg::Tab(1), &services, "gw", 0.0);
        type_ssh(&mut window, &services, "not-a-mac", "");
        window.update(Msg::SshSave, &services, "gw", 0.0);
        assert_eq!(window.dialog().unwrap().id, "ssh-allow");
        assert!(window.ssh_form.as_ref().unwrap().error.is_some());
        window.update(
            Msg::SshEdit(SshField::Mac, "02:00:00:00:00:0b".into()),
            &services,
            "gw",
            0.0,
        );
        window.update(Msg::SshHow(1), &services, "gw", 0.0);
        window.update(
            Msg::SshEdit(SshField::When, "soon".into()),
            &services,
            "gw",
            0.0,
        );
        window.update(Msg::SshSave, &services, "gw", 0.0);
        assert!(window.ssh_form.as_ref().unwrap().error.is_some());
        window.update(
            Msg::SshEdit(SshField::When, "2h".into()),
            &services,
            "gw",
            0.0,
        );
        assert!(window.ssh_form.as_ref().unwrap().change(1000).is_ok());
        window.update(Msg::SshClose, &services, "gw", 0.0);
        assert!(window.dialog().is_none());
        assert!(fake.calls().iter().all(|c| !c.starts_with("ssh-access")));
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

    /// the problem lines of the open dialog: (held and could still become valid, text) and the
    /// plain warnings that name `word`.
    fn lines(window: &NetworksWindow, word: &str) -> (Vec<(bool, HeldFor)>, usize) {
        let nodes = window.dialog().unwrap().body;
        let held = nodes
            .iter()
            .filter_map(|n| match n {
                Node::HeldError {
                    partial,
                    field,
                    text,
                } if text.contains(word) => Some((*partial, *field)),
                _ => None,
            })
            .collect();
        let warnings = nodes
            .iter()
            .filter(|n| matches!(n, Node::Warning(t) if t.contains(word)))
            .count();
        (held, warnings)
    }

    #[test]
    fn an_address_typed_into_the_network_form_is_held_until_it_is_whole() {
        let (_fake, services) = services_with("");
        let mut window = loaded(&services);
        window.update(Msg::New, &services, "gw", 0.0);
        typed(&mut window, &services, Field::Name, "lab");
        for (field, whole, key, above) in [
            (Field::Address, "172.16.120.1/22", "address", 0),
            (Field::RangeStart, "172.16.120.100", "range[0]", 1),
            (Field::RangeEnd, "172.16.120.200", "range[1]", 0),
        ] {
            let mut text = String::new();
            for ch in whole.chars() {
                text.push(ch);
                typed(&mut window, &services, field, &text);
                let (held, warnings) = lines(&window, key);
                // never a plain warning for this field while it is typed in
                assert_eq!(warnings, 0, "{text:?}");
                if text != whole {
                    for (partial, at) in &held {
                        assert!(*partial && *at == HeldFor::Above(above), "{text:?}");
                    }
                }
            }
            // a value that can never be valid is not held
            typed(&mut window, &services, field, &format!("{whole}x"));
            let (held, _) = lines(&window, key);
            assert!(held.iter().any(|(partial, _)| !partial), "{whole}x");
            typed(&mut window, &services, field, whole);
        }
    }

    #[test]
    fn a_mac_address_typed_into_the_ssh_form_is_held_until_it_is_whole() {
        let (_fake, services) = services_with(LAB);
        let mut window = loaded(&services);
        window.update(Msg::Tab(1), &services, "gw", 0.0);
        window.update(Msg::SshAdd, &services, "gw", 0.0);
        let mut text = String::new();
        for ch in "02:00:00:00:00:0a".chars() {
            text.push(ch);
            window.update(
                Msg::SshEdit(SshField::Mac, text.clone()),
                &services,
                "gw",
                0.0,
            );
            let (held, warnings) = lines(&window, "mac");
            assert_eq!(warnings, 0, "{text:?}");
            // the mac is the first of two text fields above the line (the name follows)
            assert!(held
                .iter()
                .all(|(partial, at)| *partial && *at == HeldFor::Above(1)));
        }
        window.update(
            Msg::SshEdit(SshField::Mac, "not-a-mac".into()),
            &services,
            "gw",
            0.0,
        );
        let (held, _) = lines(&window, "mac");
        assert!(held.iter().any(|(partial, _)| !partial));
    }
}
