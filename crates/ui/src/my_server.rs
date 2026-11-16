//! the My Server window: what the signed-in server is (General) and the one dangerous thing you
//! can do to it (Advanced): take lanlord back off.

use eframe::egui::{self, Frame, Margin, RichText, ScrollArea, Stroke, Ui};
use lanlord_core::log::{emit, Kind};
use lanlord_core::server::ServerFacts;

use crate::controls;
use crate::dialogs;
use crate::remote::{Remote, RED};
use crate::services::Services;
use crate::theme::{self, WHITE};

const GENERAL: usize = 0;

/// the confirmation box for a purge: what would go, who may type the name, the running job.
pub(crate) struct PurgeBox {
    host: String,
    pub(crate) preview: Remote<Vec<String>>,
    running: Remote<Vec<String>>,
    remove_admins: bool,
    /// what you typed to confirm; it must equal the host name.
    pub(crate) typed: String,
}

impl PurgeBox {
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
    pub(crate) tab: usize,
    pub(crate) facts: Remote<ServerFacts>,
    pub(crate) purge: Option<PurgeBox>,
    /// a purge finished on this host; the desktop logs off and forgets it.
    purged: Option<String>,
}

enum Act {
    None,
    Cancel,
    Look,
    Remove,
}

impl MyServer {
    /// the host whose purge just finished, once.
    pub fn take_purged(&mut self) -> Option<String> {
        self.purged.take()
    }

    /// is the purge box open (or running)?
    #[cfg(test)]
    pub fn purging(&self) -> bool {
        self.purge.is_some()
    }

    /// open the box that asks before a purge.
    pub fn ask_to_purge(&mut self, services: &Services, host: &str) {
        if self.purge.is_none() {
            self.purge = Some(PurgeBox::new(host, services));
        }
    }

    fn reload(&mut self, services: &Services, host: &str) {
        let target = host.to_string();
        let job = services.spawn(host, move |gateway, ask| gateway.server_facts(&target, ask));
        self.facts
            .track(host, job, "reading the server...", "up to date", true);
    }

    pub fn show(&mut self, ui: &mut Ui, services: &Services, host: Option<&str>) {
        let Some(host) = host else {
            ui.label("No server yet. Sign in to one first.");
            return;
        };
        self.facts.poll();
        if self.facts.wants_load(host, false) {
            self.reload(services, host);
        }
        let idle = !services.busy(host);
        let installed = self.facts.data.as_ref().map(|f| f.lanlord_installed);
        let mut tab = self.tab;
        controls::tabs(ui, &["General", "Advanced"], &mut tab, |ui, chosen| {
            if chosen == GENERAL {
                self.general(ui);
            } else {
                self.advanced(ui, services, host, idle, installed);
            }
        });
        self.tab = tab;
        ui.add_space(6.0);
        self.facts.show_note(ui);
    }

    fn general(&self, ui: &mut Ui) {
        let Some(facts) = &self.facts.data else {
            ui.label("Reading the server...");
            return;
        };
        controls::group(ui, "Computer", |ui| {
            let user = facts.user.clone();
            let memory = format!(
                "{} MB, {} MB free",
                facts.mem_total_mb, facts.mem_available_mb
            );
            let lanlord = match (&facts.lanlord_installed, &facts.lanlord_version) {
                (false, _) => "Not installed".to_string(),
                (true, Some(version)) => format!("Installed, version {version}"),
                (true, None) => "Installed".to_string(),
            };
            let rows = [
                ("Server", facts.hostname.clone()),
                ("Signed in as", user),
                ("System", facts.os.clone()),
                ("Kernel", facts.kernel.clone()),
                ("Up for", facts.uptime()),
                ("Processors", facts.cpus.to_string()),
                ("Memory", memory),
                ("LANlord", lanlord),
            ];
            for (label, value) in rows {
                controls::row(ui, label, |ui| ui.label(value));
            }
            if let Some(applied) = &facts.last_applied {
                controls::row(ui, "Last applied", |ui| ui.label(applied));
            }
        });
        ui.add_space(4.0);
        controls::group(ui, "Network interfaces", |ui| {
            for iface in facts.interfaces.iter().filter(|i| !i.loopback()) {
                let address = iface.addresses.first().map_or("no address", String::as_str);
                ui.label(format!(
                    "{}   {}   MTU {}   {}",
                    iface.name,
                    address,
                    iface.mtu,
                    iface.link()
                ));
            }
        });
        self.environment(ui);
    }

    /// the environment check (container, nesting, keyctl) goes here once core::env lands:
    /// one group box with its OK, WARN and FAIL lines. nothing is shown until then.
    fn environment(&self, _ui: &mut Ui) {}

    fn advanced(
        &mut self,
        ui: &mut Ui,
        services: &Services,
        host: &str,
        idle: bool,
        installed: Option<bool>,
    ) {
        Frame::new()
            .stroke(Stroke::new(1.0, RED))
            .inner_margin(Margin::same(3))
            .show(ui, |ui| {
                controls::group(ui, "Danger zone", |ui| {
                    ui.add_space(4.0);
                    ui.label(
                        "Removes the containers, files, service account and sudo rules that \
                         LANlord put on this server. Your own login stays.",
                    );
                    ui.add_space(6.0);
                    let ready = installed == Some(true) && idle && self.purge.is_none();
                    if theme::action(ui, "Remove LANlord from this server...", ready).clicked() {
                        self.ask_to_purge(services, host);
                    }
                    if installed == Some(false) {
                        ui.add_space(4.0);
                        ui.label("LANlord is not installed on this server.");
                    }
                });
            });
    }

    /// the confirmation box, drawn over everything while a purge is being asked for or run.
    pub fn dialog(&mut self, ctx: &egui::Context, services: &Services) {
        let Some(mut purge) = self.purge.take() else {
            return;
        };
        purge.preview.poll();
        if purge.running.poll() {
            let lines = purge.running.data.take().unwrap_or_default();
            for line in lines {
                emit(Kind::Info, line);
            }
            emit(
                Kind::Info,
                format!("lanlord was removed from {}", purge.host),
            );
            self.purged = Some(purge.host);
            return;
        }
        let is_root = self.facts.data.as_ref().is_some_and(|f| f.is_root);
        let mut act = Act::None;
        dialogs::modal(ctx, "purge-box", "Remove LANlord", |ui| {
            act = Self::purge_body(ui, services, &mut purge, is_root);
        });
        match act {
            Act::Cancel => return,
            Act::Look => purge.look(services),
            Act::Remove => purge.remove(services),
            Act::None => {}
        }
        self.purge = Some(purge);
    }

    fn purge_body(ui: &mut Ui, services: &Services, purge: &mut PurgeBox, is_root: bool) -> Act {
        let mut act = Act::None;
        let working = purge.running.busy();
        ui.label(RichText::new(format!("Remove LANlord from {}?", purge.host)).strong());
        ui.label("This cannot be undone. It would remove:");
        ui.add_space(4.0);
        theme::sunken(ui, WHITE, |ui| {
            ui.set_width(ui.available_width());
            ScrollArea::vertical()
                .id_salt("purge-lines")
                .max_height(150.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height(150.0);
                    for line in purge.preview.data.iter().flatten() {
                        ui.label(RichText::new(line).monospace().size(11.0));
                    }
                });
        });
        ui.add_space(4.0);
        let mut admins = purge.remove_admins;
        ui.add_enabled_ui(is_root && !working && !purge.preview.busy(), |ui| {
            controls::check(ui, &mut admins, "Also remove the administrator accounts");
        });
        if !is_root {
            ui.label("(Removing the accounts needs a root login.)");
        }
        if admins != purge.remove_admins {
            purge.remove_admins = admins;
            act = Act::Look;
        }
        ui.add_space(4.0);
        ui.label(format!("Type {} to confirm:", purge.host));
        controls::edit(ui, &mut purge.typed, 240.0);
        ui.add_space(6.0);
        let confirmed = purge.confirmed(services);
        controls::buttons(ui, |ui| {
            if theme::action(ui, "Cancel", !working).clicked() {
                act = Act::Cancel;
            }
            if theme::action(ui, "Remove", confirmed).clicked() {
                act = Act::Remove;
            }
        });
        let note = purge
            .running
            .note
            .clone()
            .or_else(|| purge.preview.note.clone());
        if let Some((error, text)) = note {
            controls::status_bar(ui, &text, error);
        }
        act
    }
}
