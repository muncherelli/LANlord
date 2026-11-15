//! the nt4 domain window: its settings, its logins and what it is.

use eframe::egui::{self, Ui};

use crate::logon_panel::LogonPanel;
use crate::nt4_panel::Nt4Panel;
use crate::services::Services;
use crate::users_panel::UsersPanel;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Settings,
    Logins,
    Logon,
}

#[derive(Default)]
pub struct Nt4Window {
    pub tab: Tab,
    pub settings: Nt4Panel,
    pub users: UsersPanel,
    pub logon: LogonPanel,
}

impl Nt4Window {
    pub fn poll(&mut self) {
        self.settings.poll();
        self.users.poll();
        self.logon.poll();
    }

    /// draw the tab strip and the open tab.
    pub fn show(&mut self, ui: &mut Ui, services: &Services, host: Option<&str>) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tab, Tab::Settings, "Settings");
            ui.selectable_value(&mut self.tab, Tab::Logins, "Logins");
            ui.selectable_value(&mut self.tab, Tab::Logon, "Logon scripts");
        });
        ui.separator();
        match self.tab {
            Tab::Settings => self.settings.show(ui, services, host),
            Tab::Logins => self.users.show(ui, services, host),
            Tab::Logon => self.logon.show(ui, services, host),
        }
    }

    pub fn dialog(&mut self, ctx: &egui::Context, services: &Services, host: Option<&str>) {
        self.settings.dialog(ctx, services, host);
        self.users.dialog(ctx, services, host);
        self.logon.dialog(ctx);
    }
}
