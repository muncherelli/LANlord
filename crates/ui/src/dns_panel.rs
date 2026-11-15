//! the dns & dhcp window: zone, dhcp range and lease time, static leases,
//! extra records and the leases the gateway has handed out.

use std::net::Ipv4Addr;

use anyhow::Result;
use eframe::egui::{self, ScrollArea, Ui};
use lanlord_core::bootstrap::AskPassword;
use lanlord_core::leases::Lease;
use lanlord_core::network::Internal;
use lanlord_core::settings::{
    Dhcp, DnsSettings, Record, Settings, StaticLease, Subnet, ValidationError,
};

use crate::form::{Form, FormAnswer};
use crate::gateway::Gateway;
use crate::remote::Remote;
use crate::services::{update_settings, Services};
use crate::theme;
use crate::widgets::{bad_rows, field_error, labelled_edit, messages, Table};

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

    pub fn show(&mut self, ui: &mut Ui, services: &Services, host: Option<&str>) {
        let Some(host) = host else {
            ui.label("No host yet. Add one in the Hosts window first.");
            return;
        };
        if self.remote.wants_load(host, self.dirty()) {
            self.reload(services, host);
        }
        let idle = !services.busy(host) && !self.remote.busy();
        let errors = self.errors();
        ScrollArea::vertical().id_salt("dns-scroll").show(ui, |ui| {
            self.network_line(ui);
            self.fields(ui, &errors);
            self.leases_table(ui, &errors);
            self.records_table(ui, &errors);
            self.current_table(ui);
            self.buttons(ui, services, host, idle, &errors);
            self.remote.show_note(ui);
        });
    }

    fn network_line(&self, ui: &mut Ui) {
        match self.remote.data.as_ref().map(|d| &d.internal) {
            Some(Ok(internal)) => {
                ui.label(format!("Retronet: {}", internal.subnet()));
            }
            Some(Err(why)) => {
                ui.colored_label(
                    crate::remote::RED,
                    format!("Retronet: unknown ({why}); addresses are not checked against it"),
                );
            }
            None => {
                ui.label("Retronet: not loaded yet");
            }
        }
    }

    fn fields(&mut self, ui: &mut Ui, errors: &[ValidationError]) {
        labelled_edit(ui, "Zone", &mut self.draft.zone, 160.0);
        field_error(ui, errors, "dns.zone");
        let mut on = self.draft.dhcp.is_some();
        if ui.checkbox(&mut on, "DHCP server on").changed() {
            if on {
                self.draft.dhcp = Some(self.stash.clone());
            } else if let Some(dhcp) = self.draft.dhcp.take() {
                self.stash = dhcp;
            }
        }
        if let Some(dhcp) = &mut self.draft.dhcp {
            labelled_edit(ui, "Range start", &mut dhcp.range_start, 160.0);
            field_error(ui, errors, "dns.dhcp.range[0]");
            labelled_edit(ui, "Range end", &mut dhcp.range_end, 160.0);
            field_error(ui, errors, "dns.dhcp.range[1]");
            field_error(ui, errors, "dns.dhcp.range");
            labelled_edit(ui, "Lease time", &mut dhcp.lease, 160.0);
            field_error(ui, errors, "dns.dhcp.lease");
        }
    }

    fn leases_table(&mut self, ui: &mut Ui, errors: &[ValidationError]) {
        ui.label("Static leases:");
        let hosts: Vec<StaticLease> = self
            .draft
            .dhcp
            .as_ref()
            .map(|d| d.hosts.clone())
            .unwrap_or_default();
        let rows: Vec<Vec<String>> = hosts
            .iter()
            .map(|h| vec![h.name.clone(), h.mac.clone(), h.address.clone()])
            .collect();
        let bad = bad_rows(errors, "dns.dhcp.hosts", rows.len());
        let table = Table {
            id: "dns-leases",
            headings: &["Name", "MAC", "Address"],
            widths: &[120.0, 140.0],
            rows: &rows,
            selected: self.lease_row,
            bad: &bad,
            height: 70.0,
        };
        if let Some(i) = table.show(ui) {
            self.lease_row = Some(i);
        }
        list_problems(ui, errors, "dns.dhcp.hosts");
        let (on, picked) = (
            self.draft.dhcp.is_some(),
            self.lease_row.filter(|i| *i < rows.len()),
        );
        ui.horizontal(|ui| {
            if theme::action(ui, "Add lease...", on && self.form.is_none()).clicked() {
                let blank = StaticLease {
                    name: String::new(),
                    mac: String::new(),
                    address: String::new(),
                };
                self.open_lease(None, blank);
            }
            if theme::action(ui, "Edit lease...", picked.is_some() && self.form.is_none()).clicked()
            {
                if let Some(i) = picked {
                    self.open_lease(Some(i), hosts[i].clone());
                }
            }
            if theme::action(ui, "Remove lease", picked.is_some()).clicked() {
                if let (Some(i), Some(dhcp)) = (picked, &mut self.draft.dhcp) {
                    dhcp.hosts.remove(i);
                    self.lease_row = None;
                }
            }
        });
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

    fn records_table(&mut self, ui: &mut Ui, errors: &[ValidationError]) {
        ui.label("Extra DNS records:");
        let rows: Vec<Vec<String>> = self
            .draft
            .records
            .iter()
            .map(|r| vec![r.name.clone(), r.address.clone()])
            .collect();
        let bad = bad_rows(errors, "dns.records", rows.len());
        let table = Table {
            id: "dns-records",
            headings: &["Name", "Address"],
            widths: &[160.0],
            rows: &rows,
            selected: self.record_row,
            bad: &bad,
            height: 60.0,
        };
        if let Some(i) = table.show(ui) {
            self.record_row = Some(i);
        }
        list_problems(ui, errors, "dns.records");
        let picked = self.record_row.filter(|i| *i < rows.len());
        ui.horizontal(|ui| {
            if theme::action(ui, "Add record...", self.form.is_none()).clicked() {
                self.open_record(
                    None,
                    Record {
                        name: String::new(),
                        address: String::new(),
                    },
                );
            }
            if theme::action(
                ui,
                "Edit record...",
                picked.is_some() && self.form.is_none(),
            )
            .clicked()
            {
                if let Some(i) = picked {
                    self.open_record(Some(i), self.draft.records[i].clone());
                }
            }
            if theme::action(ui, "Remove record", picked.is_some()).clicked() {
                if let Some(i) = picked {
                    self.draft.records.remove(i);
                    self.record_row = None;
                }
            }
        });
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

    fn current_table(&mut self, ui: &mut Ui) {
        ui.label("Current leases (read from the gateway):");
        let leases = match self.remote.data.as_ref().map(|d| &d.leases) {
            Some(Ok(leases)) => leases.clone(),
            Some(Err(why)) => {
                ui.colored_label(crate::remote::RED, format!("could not read them: {why}"));
                Vec::new()
            }
            None => Vec::new(),
        };
        let rows: Vec<Vec<String>> = leases
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
        let table = Table {
            id: "dns-current",
            headings: &["Expires", "MAC", "Address", "Name"],
            widths: &[110.0, 130.0, 100.0],
            rows: &rows,
            selected: self.current_row,
            bad: &[],
            height: 70.0,
        };
        if let Some(i) = table.show(ui) {
            self.current_row = Some(i);
        }
        let picked = self.current_row.and_then(|i| leases.get(i));
        let ready = picked.is_some() && self.draft.dhcp.is_some() && self.form.is_none();
        if theme::action(ui, "Make static", ready).clicked() {
            if let Some(lease) = picked {
                let name = if lanlord_core::settings::valid_label(&lease.name) {
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
        }
    }

    fn buttons(
        &mut self,
        ui: &mut Ui,
        services: &Services,
        host: &str,
        idle: bool,
        errors: &[ValidationError],
    ) {
        ui.separator();
        ui.horizontal(|ui| {
            let ok = idle && errors.is_empty();
            if theme::action(ui, "Save DNS", ok).clicked() {
                self.save(services, host, false);
            }
            if theme::action(ui, "Save DNS & apply", ok).clicked() {
                self.save(services, host, true);
            }
            if theme::action(ui, "Reload DNS", idle).clicked() {
                self.reload(services, host);
            }
        });
        if !errors.is_empty() {
            ui.label(format!("{} problem(s) to fix before saving.", errors.len()));
        }
    }

    /// the box for a lease or a record, drawn on top of everything.
    pub fn dialog(&mut self, ctx: &egui::Context) {
        let Some((ask, mut form)) = self.form.take() else {
            return;
        };
        match form.show(ctx) {
            None => self.form = Some((ask, form)),
            Some(FormAnswer::Cancel) => {}
            Some(FormAnswer::Ok) => match self.accept(&ask, &form) {
                Ok(()) => {}
                Err(error) => {
                    form.error = Some(error);
                    self.form = Some((ask, form));
                }
            },
        }
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

    #[cfg(test)]
    pub fn form_mut(&mut self) -> Option<&mut Form> {
        self.form.as_mut().map(|(_, form)| form)
    }

    #[cfg(test)]
    pub fn belongs_to(&self, host: &str) -> bool {
        self.remote.belongs_to(host)
    }

    #[cfg(test)]
    pub fn select_current(&mut self, row: usize) {
        self.current_row = Some(row);
    }

    #[cfg(test)]
    pub fn select_lease(&mut self, row: usize) {
        self.lease_row = Some(row);
    }

    #[cfg(test)]
    pub fn select_record(&mut self, row: usize) {
        self.record_row = Some(row);
    }

    #[cfg(test)]
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

/// the messages of every error under `prefix`, one line each.
fn list_problems(ui: &mut Ui, errors: &[ValidationError], prefix: &str) {
    for error in errors
        .iter()
        .filter(|e| !messages(std::slice::from_ref(e), prefix).is_empty())
    {
        ui.label(
            egui::RichText::new(error.to_string())
                .color(crate::remote::RED)
                .small(),
        );
    }
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
    fn put_replaces_or_appends() {
        let mut rows = vec![1, 2];
        put(&mut rows, Some(0), 9);
        put(&mut rows, Some(7), 5);
        put(&mut rows, None, 6);
        assert_eq!(rows, [9, 2, 5, 6]);
    }
}
