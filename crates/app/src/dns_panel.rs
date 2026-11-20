//! the dns & dhcp window: zone, dhcp range and lease time, static leases, extra records and
//! the leases the gateway has handed out.

use std::net::Ipv4Addr;

use crate::ask::AskPassword;
use anyhow::Result;
use lanlord_model::leases::Lease;
use lanlord_model::network::Internal;
use lanlord_model::settings::{
    Dhcp, DnsSettings, Record, Settings, StaticLease, Subnet, ValidationError,
};

use crate::form::{Form, FormAnswer, FormMsg};
use crate::gateway::Gateway;
use crate::remote::Remote;
use crate::services::{update_settings, Services};
use crate::view::{self, Button, Dialog, Node, Size, Table};

const MODULE: &str = "dns";

pub struct DnsData {
    pub settings: DnsSettings,
    /// the retronet the gateway is on, or why that is not known.
    pub internal: Result<Internal, String>,
    pub leases: Result<Vec<Lease>, String>,
}

fn load(gateway: &dyn Gateway, host: &str, ask: AskPassword) -> Result<DnsData> {
    let settings = DnsSettings::from_settings(&gateway.load_settings(host, ask)?)?;
    Ok(DnsData {
        settings,
        internal: gateway.probe(host).map_err(|e| e.to_string()),
        leases: gateway.leases(host, ask).map_err(|e| e.to_string()),
    })
}

/// when a lease ends, as `Nov 14 18:30`, or `never`.
pub fn expiry_text(expires: u64) -> String {
    if expires == 0 {
        return "never".into();
    }
    i64::try_from(expires)
        .ok()
        .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
        .map_or_else(
            || expires.to_string(),
            |time| {
                time.with_timezone(&chrono::Local)
                    .format("%b %-d %H:%M")
                    .to_string()
            },
        )
}

enum Ask {
    /// a static lease: the row being edited, if any.
    Lease(Option<usize>),
    Record(Option<usize>),
}

/// what a click or a keystroke in the panel means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    Tab(usize),
    Zone(String),
    ToggleDhcp,
    RangeStart(String),
    RangeEnd(String),
    Lease(String),
    PickLease(usize),
    AddLease,
    EditLease,
    RemoveLease,
    PickRecord(usize),
    AddRecord,
    EditRecord,
    RemoveRecord,
    PickCurrent(usize),
    MakeStatic,
    Reload,
    SaveAndApply,
    Save,
}

#[derive(Default)]
pub struct DnsPanel {
    remote: Remote<DnsData>,
    pub draft: DnsSettings,
    /// what the dhcp fields held when the checkbox was last switched off.
    stash: Dhcp,
    lease_row: Option<usize>,
    record_row: Option<usize>,
    current_row: Option<usize>,
    form: Option<(Ask, Form)>,
    /// the open tab: settings, static leases, extra records or current leases.
    pub tab: usize,
}

impl DnsPanel {
    pub fn poll(&mut self) {
        if self.remote.poll() {
            if let Some(data) = &self.remote.data {
                self.draft = data.settings.clone();
            }
            (self.lease_row, self.record_row, self.current_row) = (None, None, None);
        }
    }

    pub fn dirty(&self) -> bool {
        self.remote
            .data
            .as_ref()
            .is_some_and(|d| d.settings != self.draft)
    }

    fn errors(&self) -> Vec<ValidationError> {
        let (subnet, gateway) = match self.remote.data.as_ref().map(|d| &d.internal) {
            Some(Ok(internal)) => (internal.subnet(), internal.address),
            _ => (Subnet::new(Ipv4Addr::UNSPECIFIED, 0), Ipv4Addr::UNSPECIFIED),
        };
        self.draft.validate(&subnet, gateway)
    }

    fn reload(&mut self, services: &Services, host: &str) {
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| load(gateway, &target, ask));
        self.remote
            .track(host, job, "loading...", "loaded the dns settings", true);
    }

    fn save(&mut self, services: &Services, host: &str, apply: bool) {
        let (target, draft) = (host.to_string(), self.draft.clone());
        let modules = if apply {
            vec![MODULE.to_string()]
        } else {
            vec![]
        };
        let job = services.spawn(host, move |gateway, ask| {
            let edit = |settings: &mut Settings| {
                draft.apply_to(settings);
                Ok(())
            };
            update_settings(gateway, &target, &[], edit, &modules, ask)?;
            load(gateway, &target, ask)
        });
        let (doing, done) = if apply {
            ("saving and applying...", "saved and applied")
        } else {
            ("saving...", "saved")
        };
        self.remote.track(host, job, doing, done, false);
    }

    /// the settings may be saved: nothing runs, they pass their checks and are `host`'s own.
    pub fn can_save(&self, services: &Services, host: &str) -> bool {
        !services.busy(host)
            && !self.remote.busy()
            && self.errors().is_empty()
            && self.remote.belongs_to(host)
    }

    /// the panel for `host`; loads the settings when it shows another host's.
    pub fn view(&mut self, services: &Services, host: Option<&str>) -> Vec<Node<Msg>> {
        let Some(host) = host else {
            return vec![Node::Text(
                "No host yet. Add one in the Hosts window first.".into(),
            )];
        };
        if self.remote.wants_load(host, self.dirty()) {
            self.reload(services, host);
        }
        let idle = !services.busy(host) && !self.remote.busy();
        let errors = self.errors();
        let mut pages = vec![
            (0, self.settings_page(&errors)),
            (1, self.leases_page(&errors)),
            (2, self.records_page(&errors)),
            (3, self.current_page()),
        ];
        let at = pages
            .iter()
            .position(|(tab, _)| *tab == self.tab)
            .unwrap_or(0);
        let (_, page) = pages.remove(at);
        let ok = self.can_save(services, host);
        let mut nodes = vec![
            Node::Tabs {
                titles: view::titles(&[
                    "Settings",
                    "Static leases",
                    "Extra records",
                    "Current leases",
                ]),
                current: self.tab,
                pick: Msg::Tab,
                body: page,
                others: pages,
            },
            Node::Gap(6.0),
            Node::Buttons(vec![
                Button::new("Reload DNS", idle, Msg::Reload),
                Button::new("Save DNS & apply", ok, Msg::SaveAndApply),
                Button::new("Save DNS", ok, Msg::Save),
            ]),
        ];
        if !errors.is_empty() {
            nodes.push(Node::Text(format!(
                "{} problem(s) to fix before saving.",
                errors.len()
            )));
        }
        nodes.push(Node::Gap(4.0));
        nodes.push(view::status(&self.remote));
        nodes
    }

    fn settings_page(&self, errors: &[ValidationError]) -> Vec<Node<Msg>> {
        let network = match self.remote.data.as_ref().map(|d| &d.internal) {
            Some(Ok(internal)) => Node::Text(format!("Retronet: {}", internal.subnet())),
            Some(Err(why)) => Node::Warning(format!(
                "Retronet: unknown ({why}); addresses are not checked against it"
            )),
            None => Node::Text("Retronet: not loaded yet".into()),
        };
        let edit = |label: &str, value: &str, msg: fn(String) -> Msg| Node::Labelled {
            label: label.into(),
            body: vec![Node::Input {
                value: value.into(),
                size: Size::Points(170.0),
                hint: String::new(),
                focus: false,
                edit: Box::new(msg),
                enter: None,
            }],
        };
        let mut names = vec![edit("Zone", &self.draft.zone, Msg::Zone)];
        names.extend(view::field_errors(errors, "dns.zone"));
        let mut dhcp = vec![Node::Check {
            label: "DHCP server on".into(),
            checked: self.draft.dhcp.is_some(),
            enabled: true,
            toggle: Msg::ToggleDhcp,
        }];
        if let Some(d) = &self.draft.dhcp {
            dhcp.push(Node::Gap(2.0));
            dhcp.push(edit("Range start", &d.range_start, Msg::RangeStart));
            dhcp.extend(view::field_errors(errors, "dns.dhcp.range[0]"));
            dhcp.push(edit("Range end", &d.range_end, Msg::RangeEnd));
            dhcp.extend(view::field_errors(errors, "dns.dhcp.range[1]"));
            dhcp.extend(view::field_errors(errors, "dns.dhcp.range"));
            dhcp.push(edit("Lease time", &d.lease, Msg::Lease));
            dhcp.extend(view::field_errors(errors, "dns.dhcp.lease"));
        }
        vec![
            network,
            Node::Gap(4.0),
            Node::Group {
                title: "Names".into(),
                body: names,
            },
            Node::Gap(4.0),
            Node::Group {
                title: "DHCP".into(),
                body: dhcp,
            },
        ]
    }

    fn hosts(&self) -> Vec<StaticLease> {
        self.draft
            .dhcp
            .as_ref()
            .map(|d| d.hosts.clone())
            .unwrap_or_default()
    }

    fn leases_page(&self, errors: &[ValidationError]) -> Vec<Node<Msg>> {
        let hosts = self.hosts();
        let rows: Vec<Vec<String>> = hosts
            .iter()
            .map(|h| vec![h.name.clone(), h.mac.clone(), h.address.clone()])
            .collect();
        let picked = self.lease_row.filter(|i| *i < rows.len()).is_some();
        let free = self.form.is_none();
        let mut nodes = vec![Node::Table(Table {
            id: "dns-leases",
            headings: vec!["Name", "MAC", "Address"],
            widths: vec![120.0, 140.0],
            bad: view::bad_rows(errors, "dns.dhcp.hosts", rows.len()),
            rows,
            selected: self.lease_row,
            height: 190.0,
            select: Msg::PickLease,
        })];
        nodes.extend(problems(errors, "dns.dhcp.hosts"));
        nodes.push(Node::Toolbar(vec![
            Button::new(
                "Add lease...",
                self.draft.dhcp.is_some() && free,
                Msg::AddLease,
            ),
            Button::new("Edit lease...", picked && free, Msg::EditLease),
            Button::new("Remove lease", picked, Msg::RemoveLease),
        ]));
        nodes
    }

    fn records_page(&self, errors: &[ValidationError]) -> Vec<Node<Msg>> {
        let rows: Vec<Vec<String>> = self
            .draft
            .records
            .iter()
            .map(|r| vec![r.name.clone(), r.address.clone()])
            .collect();
        let picked = self.record_row.filter(|i| *i < rows.len()).is_some();
        let free = self.form.is_none();
        let mut nodes = vec![Node::Table(Table {
            id: "dns-records",
            headings: vec!["Name", "Address"],
            widths: vec![160.0],
            bad: view::bad_rows(errors, "dns.records", rows.len()),
            rows,
            selected: self.record_row,
            height: 190.0,
            select: Msg::PickRecord,
        })];
        nodes.extend(problems(errors, "dns.records"));
        nodes.push(Node::Toolbar(vec![
            Button::new("Add record...", free, Msg::AddRecord),
            Button::new("Edit record...", picked && free, Msg::EditRecord),
            Button::new("Remove record", picked, Msg::RemoveRecord),
        ]));
        nodes
    }

    fn current(&self) -> Vec<Lease> {
        match self.remote.data.as_ref().map(|d| &d.leases) {
            Some(Ok(leases)) => leases.clone(),
            _ => Vec::new(),
        }
    }

    fn current_page(&self) -> Vec<Node<Msg>> {
        let mut nodes = vec![Node::Text("Read from the gateway:".into())];
        if let Some(Err(why)) = self.remote.data.as_ref().map(|d| &d.leases) {
            nodes.push(Node::Warning(format!("could not read them: {why}")));
        }
        let leases = self.current();
        let rows = leases
            .iter()
            .map(|l| {
                vec![
                    expiry_text(l.expires),
                    l.mac.clone(),
                    l.address.clone(),
                    l.name.clone(),
                ]
            })
            .collect();
        let picked = self.current_row.and_then(|i| leases.get(i)).is_some();
        let ready = picked && self.draft.dhcp.is_some() && self.form.is_none();
        nodes.push(Node::Table(Table {
            id: "dns-current",
            headings: vec!["Expires", "MAC", "Address", "Name"],
            widths: vec![110.0, 130.0, 100.0],
            rows,
            selected: self.current_row,
            bad: Vec::new(),
            height: 190.0,
            select: Msg::PickCurrent,
        }));
        nodes.push(Node::Button(Button::new(
            "Make static",
            ready,
            Msg::MakeStatic,
        )));
        nodes
    }

    /// act on a click or keystroke for `host`.
    pub fn update(&mut self, msg: Msg, services: &Services, host: &str) {
        if !self.allowed(&msg, services, host) {
            return;
        }
        match msg {
            Msg::Tab(tab) => self.tab = tab,
            Msg::Zone(text) => self.draft.zone = text,
            Msg::ToggleDhcp => match self.draft.dhcp.take() {
                Some(dhcp) => self.stash = dhcp,
                None => self.draft.dhcp = Some(self.stash.clone()),
            },
            Msg::RangeStart(text) => self.edit_dhcp(|d| d.range_start = text),
            Msg::RangeEnd(text) => self.edit_dhcp(|d| d.range_end = text),
            Msg::Lease(text) => self.edit_dhcp(|d| d.lease = text),
            Msg::PickLease(i) => self.lease_row = Some(i),
            Msg::AddLease => self.open_lease(
                None,
                StaticLease {
                    name: String::new(),
                    mac: String::new(),
                    address: String::new(),
                },
            ),
            Msg::EditLease => self.edit_lease(),
            Msg::RemoveLease => self.remove_lease(),
            Msg::PickRecord(i) => self.record_row = Some(i),
            Msg::AddRecord => self.open_record(
                None,
                Record {
                    name: String::new(),
                    address: String::new(),
                },
            ),
            Msg::EditRecord => self.edit_record(),
            Msg::RemoveRecord => self.remove_record(),
            Msg::PickCurrent(i) => self.current_row = Some(i),
            Msg::MakeStatic => self.make_static(),
            Msg::Reload => self.reload(services, host),
            Msg::SaveAndApply => self.save(services, host, true),
            Msg::Save => self.save(services, host, false),
        }
    }

    /// the same rules as the buttons: a front end may send any message.
    fn allowed(&self, msg: &Msg, services: &Services, host: &str) -> bool {
        let free = self.form.is_none();
        match msg {
            Msg::AddLease | Msg::MakeStatic => free && self.draft.dhcp.is_some(),
            Msg::EditLease | Msg::AddRecord | Msg::EditRecord => free,
            Msg::Reload => !services.busy(host) && !self.remote.busy(),
            Msg::Save | Msg::SaveAndApply => self.can_save(services, host),
            _ => true,
        }
    }

    fn edit_dhcp(&mut self, edit: impl FnOnce(&mut Dhcp)) {
        if let Some(dhcp) = self.draft.dhcp.as_mut() {
            edit(dhcp);
        }
    }

    fn edit_lease(&mut self) {
        let hosts = self.hosts();
        if let Some(i) = self.lease_row.filter(|i| *i < hosts.len()) {
            self.open_lease(Some(i), hosts[i].clone());
        }
    }

    fn remove_lease(&mut self) {
        let picked = self.lease_row;
        if let (Some(i), Some(d)) = (picked, &mut self.draft.dhcp) {
            if i < d.hosts.len() {
                d.hosts.remove(i);
                self.lease_row = None;
            }
        }
    }

    fn edit_record(&mut self) {
        if let Some(i) = self.record_row.filter(|i| *i < self.draft.records.len()) {
            self.open_record(Some(i), self.draft.records[i].clone());
        }
    }

    fn remove_record(&mut self) {
        if let Some(i) = self.record_row.filter(|i| *i < self.draft.records.len()) {
            self.draft.records.remove(i);
            self.record_row = None;
        }
    }

    /// open a box to turn the picked current lease into a static one.
    fn make_static(&mut self) {
        let leases = self.current();
        let Some(lease) = self.current_row.and_then(|i| leases.get(i)) else {
            return;
        };
        let name = if lanlord_model::settings::valid_label(&lease.name) {
            lease.name.clone()
        } else {
            String::new()
        };
        self.open_lease(
            None,
            StaticLease {
                name,
                mac: lease.mac.clone(),
                address: lease.address.clone(),
            },
        );
    }

    /// the box for a lease or a record, while one is open.
    pub fn dialog(&mut self) -> Option<Dialog<FormMsg>> {
        self.form.as_mut().map(|(_, form)| form.view())
    }

    /// act on a message from the box.
    pub fn update_dialog(&mut self, msg: FormMsg) {
        let Some((ask, mut form)) = self.form.take() else {
            return;
        };
        match form.update(msg) {
            None => self.form = Some((ask, form)),
            Some(FormAnswer::Cancel) => {}
            Some(FormAnswer::Ok) => {
                if let Err(error) = self.accept(&ask, &form) {
                    form.error = Some(error);
                    self.form = Some((ask, form));
                }
            }
        }
    }

    fn open_lease(&mut self, row: Option<usize>, lease: StaticLease) {
        let title = if row.is_some() {
            "Edit a lease"
        } else {
            "Add a lease"
        };
        self.form = Some((
            Ask::Lease(row),
            Form::new("dns-lease", title, "")
                .text("Name", &lease.name)
                .text("MAC address (aa:bb:cc:dd:ee:ff)", &lease.mac)
                .text("IP address", &lease.address),
        ));
    }

    fn open_record(&mut self, row: Option<usize>, record: Record) {
        let title = if row.is_some() {
            "Edit a record"
        } else {
            "Add a record"
        };
        self.form = Some((
            Ask::Record(row),
            Form::new("dns-record", title, "")
                .text("Name", &record.name)
                .text("IP address", &record.address),
        ));
    }

    fn accept(&mut self, ask: &Ask, form: &Form) -> Result<(), String> {
        if (0..3).all(|i| form.value(i).is_empty()) {
            return Err("fill something in, or Cancel".into());
        }
        match *ask {
            Ask::Lease(row) => {
                let lease = StaticLease {
                    name: form.value(0).into(),
                    mac: form.value(1).to_ascii_lowercase(),
                    address: form.value(2).into(),
                };
                let dhcp = self.draft.dhcp.as_mut().ok_or("DHCP is switched off")?;
                put(&mut dhcp.hosts, row, lease);
            }
            Ask::Record(row) => {
                let record = Record {
                    name: form.value(0).into(),
                    address: form.value(1).into(),
                };
                put(&mut self.draft.records, row, record);
            }
        }
        Ok(())
    }

    /// the open box, for tests that fill it in.
    pub fn form_mut(&mut self) -> Option<&mut Form> {
        self.form.as_mut().map(|(_, form)| form)
    }

    pub fn belongs_to(&self, host: &str) -> bool {
        self.remote.belongs_to(host)
    }

    pub fn select_current(&mut self, row: usize) {
        self.current_row = Some(row);
    }

    pub fn select_lease(&mut self, row: usize) {
        self.lease_row = Some(row);
    }

    pub fn select_record(&mut self, row: usize) {
        self.record_row = Some(row);
    }

    pub fn problems(&self) -> Vec<ValidationError> {
        self.errors()
    }
}

/// replace row `at`, or add at the end.
fn put<T>(rows: &mut Vec<T>, at: Option<usize>, item: T) {
    match at.and_then(|i| rows.get_mut(i)) {
        Some(slot) => *slot = item,
        None => rows.push(item),
    }
}

/// the messages of every error under `prefix`, one small red line each.
fn problems(errors: &[ValidationError], prefix: &str) -> Vec<Node<Msg>> {
    errors
        .iter()
        .filter(|e| !view::messages(std::slice::from_ref(e), prefix).is_empty())
        .map(|e| Node::FieldError(e.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_is_never_for_zero_and_a_date_otherwise() {
        assert_eq!(expiry_text(0), "never");
        let text = expiry_text(1_700_000_000);
        assert!(text.contains("Nov") && text.contains(':'), "{text}");
        assert_eq!(expiry_text(u64::MAX), u64::MAX.to_string());
    }

    #[test]
    fn unloaded_settings_are_not_saved_and_leases_need_dhcp() {
        use std::sync::Arc;

        use crate::gateway::Fake;
        use crate::wake::Wake;

        let fake = Arc::new(Fake::new());
        let services = Services::new(fake.clone(), Wake::none());
        let mut panel = DnsPanel::default();
        panel.update(Msg::Save, &services, "a@b");
        panel.update(Msg::SaveAndApply, &services, "a@b");
        assert!(fake.calls().is_empty(), "the draft is not a@b's");
        panel.draft.dhcp = None;
        panel.update(Msg::AddLease, &services, "a@b");
        assert!(panel.form_mut().is_none(), "no lease box with dhcp off");
    }

    #[test]
    fn put_replaces_or_appends() {
        let mut rows = vec![1, 2];
        put(&mut rows, Some(0), 9);
        put(&mut rows, Some(7), 5);
        put(&mut rows, None, 6);
        assert_eq!(rows, [9, 2, 5, 6]);
    }
}
