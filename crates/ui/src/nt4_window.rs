//! the nt4 domain window: its settings, its logins, its logon scripts and its computers.

use eframe::egui::{self, Ui};

use crate::computers_panel::ComputersPanel;
use crate::controls;
use crate::logon_panel::LogonPanel;
use crate::nt4_panel::Nt4Panel;
use crate::users_panel::UsersPanel;
use lanlord_app::services::Services;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Settings,
    Logins,
    Logon,
    Computers,
}

#[derive(Default)]
pub struct Nt4Window {
    pub tab: Tab,
    pub settings: Nt4Panel,
    pub users: UsersPanel,
    pub logon: LogonPanel,
    pub computers: ComputersPanel,
}

impl Nt4Window {
    pub fn poll(&mut self) {
        self.settings.poll();
        self.users.poll();
        self.logon.poll();
        self.computers.poll();
    }

    /// draw the tab strip and the open tab.
    pub fn show(&mut self, ui: &mut Ui, services: &Services, host: Option<&str>) {
        let mut chosen = match self.tab {
            Tab::Settings => 0,
            Tab::Logins => 1,
            Tab::Logon => 2,
            Tab::Computers => 3,
        };
        // the pages are measured again when what any of them shows changes
        let key = [
            self.settings.key(services, host),
            self.users.key(services, host),
            self.logon.key(services, host),
            self.computers.key(services, host),
        ]
        .iter()
        .fold(0u64, |k, n| k.rotate_left(21) ^ n);
        controls::tabs(
            ui,
            &["Settings", "Logins", "Logon scripts", "Computers"],
            key,
            &mut chosen,
            |ui, open| match chosen_tab(open) {
                Tab::Settings => self.settings.show(ui, services, host),
                Tab::Logins => self.users.show(ui, services, host),
                Tab::Logon => self.logon.show(ui, services, host),
                Tab::Computers => self.computers.show(ui, services, host),
            },
        );
        self.tab = chosen_tab(chosen);
    }

    pub fn dialog(&mut self, ctx: &egui::Context, services: &Services, host: Option<&str>) {
        self.settings.dialog(ctx, services, host);
        self.users.dialog(ctx, services, host);
        self.logon.dialog(ctx);
        // the host can change from another tab, while the Computers tab draws nothing
        self.computers.close_stale_box(host);
        self.computers.dialog(ctx, services, host);
    }
}

fn chosen_tab(index: usize) -> Tab {
    match index {
        1 => Tab::Logins,
        2 => Tab::Logon,
        3 => Tab::Computers,
        _ => Tab::Settings,
    }
}
