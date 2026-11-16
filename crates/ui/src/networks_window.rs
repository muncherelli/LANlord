//! the Networks window: the server's external network and the networks lanlord runs, in the
//! style of Network and Dial-up Connections. it only shows them in this version.

use eframe::egui::{self, Ui};
use lanlord_model::networks::{self, NetworkRow, Role};
use lanlord_model::settings::Settings;

use crate::controls;
use crate::dialogs;
use crate::remote::{Remote, ShowNote};
use crate::theme;
use crate::widgets::Table;
use lanlord_app::services::Services;

/// what the window asks the desktop to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Ask {
    /// open the DNS & DHCP window.
    Dns,
}

#[derive(Default)]
pub struct Networks {
    pub(crate) rows: Remote<Vec<NetworkRow>>,
    pub(crate) selected: usize,
    /// the network whose properties are open.
    pub(crate) details: Option<NetworkRow>,
}

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

    pub fn show(&mut self, ui: &mut Ui, services: &Services, host: Option<&str>) -> Option<Ask> {
        let Some(host) = host else {
            ui.label("No server yet. Sign in to one first.");
            return None;
        };
        self.rows.poll();
        if self.rows.wants_load(host, false) {
            self.reload(services, host);
        }
        let mut ask = None;
        let idle = !services.busy(host);
        let rows = self.rows.data.clone().unwrap_or_default();
        controls::group(ui, "Networks on this server", |ui| {
            let cells: Vec<Vec<String>> = rows.iter().map(cells).collect();
            let clicked = Table {
                id: "networks-list",
                headings: &[
                    "Name",
                    "Interface",
                    "Address",
                    "MTU",
                    "Link",
                    "DHCP range",
                    "Clients",
                ],
                widths: &[125.0, 70.0, 125.0, 48.0, 90.0, 210.0],
                rows: &cells,
                selected: Some(self.selected),
                bad: &[],
                height: 120.0,
            }
            .show(ui);
            if let Some(i) = clicked {
                self.selected = i;
            }
        });
        ui.add_space(6.0);
        let chosen = rows.get(self.selected).cloned();
        controls::buttons(ui, |ui| {
            let lanlord = chosen.as_ref().is_some_and(|r| r.role == Role::Lanlord);
            if theme::action(ui, "DNS & DHCP...", lanlord && idle).clicked() {
                ask = Some(Ask::Dns);
            }
            if theme::action(ui, "Properties...", chosen.is_some()).clicked() {
                self.details = chosen.clone();
            }
        });
        ui.add_space(4.0);
        ui.label("Editing networks comes next. For now you can look at them, and open DNS & DHCP.");
        ui.add_space(4.0);
        self.rows.show_note(ui);
        ask
    }

    /// the read-only properties box.
    pub fn dialog(&mut self, ctx: &egui::Context) {
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

fn cells(row: &NetworkRow) -> Vec<String> {
    vec![
        row.name.clone(),
        row.interface.clone(),
        row.address.clone(),
        row.mtu.to_string(),
        row.link.clone(),
        row.dhcp.clone().unwrap_or_default(),
        row.leases.map(|n| n.to_string()).unwrap_or_default(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanlord_model::server::demo;

    #[test]
    fn a_row_becomes_one_cell_per_column() {
        let settings = Settings::parse(
            "docker:\n  internal_address: 172.16.120.1/22\ndns:\n  zone: retronet\n  dhcp:\n    range: [172.16.120.100, 172.16.120.200]\n    lease: 7d\n",
        )
        .unwrap();
        let rows = networks::rows(&demo().interfaces, &settings, Some(2));
        assert_eq!(
            cells(&rows[1]),
            [
                "retronet",
                "eth1",
                "172.16.120.1/22",
                "1450",
                "Connected",
                "172.16.120.100 - 172.16.120.200",
                "2"
            ]
        );
        assert_eq!(cells(&rows[0])[5], "");
    }
}
