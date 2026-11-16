//! the Networks window: the server's external network and the networks lanlord runs, in the
//! style of Network and Dial-up Connections. each network has a tab showing what Windows shows
//! in a connection's Status box (link, speed and traffic), and Details... opens the Network
//! Connection Details list. it only shows them in this version.

use eframe::egui::{self, Ui};
use lanlord_core::netstatus::InterfaceStatus;
use lanlord_core::networks::{self, NetworkRow, Role};
use lanlord_core::settings::Settings;

use crate::controls;
use crate::dialogs;
use crate::remote::Remote;
use crate::services::Services;
use crate::theme;
use crate::widgets::Table;

/// what the window asks the desktop to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Ask {
    /// open the DNS & DHCP window.
    Dns,
}

#[derive(Default)]
pub struct Networks {
    pub(crate) rows: Remote<Vec<NetworkRow>>,
    /// the status of every connection, matched to a row by its interface.
    pub(crate) status: Remote<Vec<InterfaceStatus>>,
    /// the tab that is open.
    pub(crate) selected: usize,
    /// the network whose properties are open.
    pub(crate) details: Option<NetworkRow>,
    /// the connection whose Network Connection Details are open.
    pub(crate) connection: Option<InterfaceStatus>,
    /// when the status was last asked for, in egui's seconds.
    status_at: f64,
}

/// how often an open window reads the status again, so the traffic counts move.
const STATUS_EVERY: f64 = 5.0;

impl Networks {
    #[cfg(test)]
    pub(crate) fn select(&mut self, index: usize) {
        self.selected = index;
    }

    fn reload(&mut self, services: &Services, host: &str) {
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| {
            let facts = gateway.server_facts(&target, ask)?;
            // networks still show without settings, or without the key to read them
            let settings = gateway
                .load_settings(&target, ask)
                .unwrap_or_else(|_| Settings::empty());
            let leases = gateway.leases(&target, ask).ok().map(|l| l.len());
            Ok(networks::rows(&facts.interfaces, &settings, leases))
        });
        self.rows
            .track(host, job, "reading the networks...", "up to date", true);
    }

    fn reload_status(&mut self, services: &Services, host: &str) {
        let target = host.to_string();
        let job = services.quiet(move |gateway| gateway.network_status(&target));
        self.status.track(host, Some(job), "", "", true);
    }

    pub fn show(&mut self, ui: &mut Ui, services: &Services, host: Option<&str>) -> Option<Ask> {
        let Some(host) = host else {
            ui.label("No server yet. Sign in to one first.");
            return None;
        };
        self.rows.poll();
        self.status.poll();
        if self.rows.wants_load(host, false) {
            self.reload(services, host);
        }
        // the status loads when the window opens, then again every few seconds while it is
        // open and the last read worked; after a failure it waits for Refresh
        let now = ui.input(|i| i.time);
        let status_failed = self.status.note.as_ref().is_some_and(|(error, _)| *error);
        let stale = self.status.belongs_to(host)
            && !status_failed
            && !self.status.busy()
            && now - self.status_at >= STATUS_EVERY;
        if self.status.wants_load(host, false) || stale {
            self.status_at = now;
            self.reload_status(services, host);
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(STATUS_EVERY));
        let status_error = self
            .status
            .note
            .as_ref()
            .filter(|(error, _)| *error)
            .map(|(_, text)| text.clone());
        let mut ask = None;
        let idle = !services.busy(host);
        let rows = self.rows.data.clone().unwrap_or_default();
        let status = self.status.data.clone().unwrap_or_default();
        if rows.is_empty() {
            self.rows.show_note(ui);
            return None;
        }
        self.selected = self.selected.min(rows.len() - 1);
        let labels: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        controls::tabs(ui, &labels, &mut self.selected, |ui, open| {
            let row = &rows[open];
            page(
                ui,
                row,
                status.iter().find(|s| s.name == row.interface),
                status_error.as_deref(),
            );
        });
        ui.add_space(6.0);
        let chosen = rows.get(self.selected).cloned();
        let chosen_status = chosen
            .as_ref()
            .and_then(|r| status.iter().find(|s| s.name == r.interface).cloned());
        let mut refresh = false;
        controls::buttons(ui, |ui| {
            let reading = self.rows.busy() || self.status.busy();
            refresh = theme::action(ui, "Refresh", !reading).clicked();
            if theme::action(ui, "Details...", chosen_status.is_some()).clicked() {
                self.connection = chosen_status.clone();
            }
            let lanlord = chosen.as_ref().is_some_and(|r| r.role == Role::Lanlord);
            if theme::action(ui, "DNS & DHCP...", lanlord && idle).clicked() {
                ask = Some(Ask::Dns);
            }
            if theme::action(ui, "Properties...", chosen.is_some()).clicked() {
                self.details = chosen.clone();
            }
        });
        if refresh {
            self.reload(services, host);
            self.status_at = now;
            self.reload_status(services, host);
        }
        ui.add_space(4.0);
        ui.label("Editing networks comes next. For now you can look at them, and open DNS & DHCP.");
        ui.add_space(4.0);
        self.rows.show_note(ui);
        ask
    }

    /// the read-only properties box and the Network Connection Details list.
    pub fn dialog(&mut self, ctx: &egui::Context) {
        self.connection_dialog(ctx);
        let Some(row) = self.details.clone() else {
            return;
        };
        let mut close = false;
        dialogs::modal(
            ctx,
            "network-details",
            &format!("{} Properties", row.name),
            |ui| {
                for (label, value) in row.details() {
                    controls::row(ui, &label, |ui| ui.label(value));
                }
                ui.add_space(6.0);
                controls::buttons(ui, |ui| {
                    close = theme::action(ui, "OK", true).clicked();
                });
            },
        );
        if close {
            self.details = None;
        }
    }
}

impl Networks {
    fn connection_dialog(&mut self, ctx: &egui::Context) {
        let Some(status) = self.connection.clone() else {
            return;
        };
        let mut close = false;
        dialogs::modal(
            ctx,
            "network-connection-details",
            "Network Connection Details",
            |ui| {
                ui.label("Network Connection Details:");
                ui.add_space(4.0);
                let cells: Vec<Vec<String>> = status
                    .details(local_time)
                    .into_iter()
                    .map(|(k, v)| vec![k, v])
                    .collect();
                Table {
                    id: "network-details-list",
                    headings: &["Property", "Value"],
                    widths: &[150.0, 230.0],
                    rows: &cells,
                    selected: None,
                    bad: &[],
                    height: 220.0,
                }
                .show(ui);
                ui.add_space(6.0);
                controls::buttons(ui, |ui| {
                    close = theme::action(ui, "Close", true).clicked();
                });
            },
        );
        if close {
            self.connection = None;
        }
    }
}

/// one network's tab: Connection and Activity like a connection's Status box, and the lanlord
/// network's dhcp.
fn page(ui: &mut Ui, row: &NetworkRow, status: Option<&InterfaceStatus>, error: Option<&str>) {
    controls::group(ui, "Connection", |ui| {
        let state = status.map_or(row.link.clone(), |s| s.connection.text().to_string());
        controls::row(ui, "Status:", |ui| ui.label(state));
        controls::row(ui, "Interface:", |ui| ui.label(&row.interface));
        controls::row(ui, "Speed:", |ui| {
            ui.label(status.map_or("Unknown".to_string(), InterfaceStatus::speed))
        });
        controls::row(ui, "Address:", |ui| ui.label(&row.address));
    });
    if let (None, Some(error)) = (status, error) {
        ui.add_space(6.0);
        ui.label(format!(
            "Could not read the connection status: {error}. Press Refresh to try again."
        ));
    }
    if let Some(status) = status {
        ui.add_space(6.0);
        controls::group(ui, "Activity", |ui| {
            let c = status.counters;
            Table {
                id: "network-activity",
                headings: &["", "Sent", "Received"],
                widths: &[80.0, 160.0],
                rows: &[
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
                bad: &[],
                height: 44.0,
            }
            .show(ui);
        });
    }
    if row.role == Role::Lanlord {
        ui.add_space(6.0);
        controls::group(ui, "LANlord network", |ui| {
            let dhcp = row.dhcp.clone().unwrap_or_else(|| "not served".into());
            controls::row(ui, "DHCP range:", |ui| ui.label(dhcp));
            let clients = row.leases.map_or("-".to_string(), |n| n.to_string());
            controls::row(ui, "Clients with a lease:", |ui| ui.label(clients));
        });
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
    use lanlord_core::server::demo;

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
}
