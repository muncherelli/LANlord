//! the screen root lands on after logging on: create the first administrator. its contents
//! live in `lanlord_app::admin_setup`; this draws them in a box in the middle of the screen.

use std::ops::{Deref, DerefMut};

use eframe::egui::{self, Align2, Area, Frame, Id, Margin, Order, Vec2};

pub use lanlord_app::admin_setup::Outcome;
use lanlord_app::services::Services;

use crate::theme::{self, GRAY, NAVY, WHITE};
use crate::view;

pub struct AdminSetup(lanlord_app::admin_setup::AdminSetup);

impl Deref for AdminSetup {
    type Target = lanlord_app::admin_setup::AdminSetup;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for AdminSetup {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl AdminSetup {
    pub fn new() -> Self {
        Self(lanlord_app::admin_setup::AdminSetup::new())
    }

    #[cfg(test)]
    pub fn with_test_keys(keys: &[&str]) -> Self {
        Self(lanlord_app::admin_setup::AdminSetup::with_test_keys(keys))
    }

    /// draw the screen; `Some` once it is over. `host` is the signed-in server.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        services: &Services,
        host: &str,
    ) -> Option<Outcome> {
        if let Some(done) = self.0.poll() {
            return Some(done);
        }
        let mut outcome = None;
        Area::new(Id::unique("admin-setup"))
            .order(Order::Middle)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                let frame = Frame::new().fill(GRAY).inner_margin(Margin::same(3));
                let shown = frame.show(ui, |ui| {
                    ui.set_width(500.0);
                    let (bar, _) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), 20.0),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect_filled(bar, 0.0, NAVY);
                    ui.painter().text(
                        bar.left_center() + Vec2::new(6.0, 0.0),
                        Align2::LEFT_CENTER,
                        self.0.title(),
                        theme::font(),
                        WHITE,
                    );
                    ui.add_space(6.0);
                    Frame::new()
                        .inner_margin(Margin::symmetric(10, 4))
                        .show(ui, |ui| {
                            let nodes = self.0.view();
                            if let Some(msg) = view::show(ui, &nodes) {
                                outcome = self.0.update(msg, services, host);
                            }
                        });
                });
                theme::bevel(ui.painter(), shown.response.rect, true);
            });
        if let Some(dialog) = self.0.warning() {
            if let Some(button) = view::dialog(ctx, &dialog) {
                self.0.answer_warning(button);
            }
        }
        outcome
    }
}
