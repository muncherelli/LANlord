//! the glue every tab of the nt4 window shares: a panel that lives in `lanlord_app` is wrapped,
//! drawn with egui, and its box is drawn on top of everything.

/// `$name`, a wrapper around the app crate's `$panel` with the methods the window calls: `key`,
/// `show` and `dialog`. `$box_doc` says what the box is for.
macro_rules! panel_glue {
    ($name:ident, $panel:ty, $box_doc:literal) => {
        #[derive(Default)]
        pub struct $name($panel);

        impl ::std::ops::Deref for $name {
            type Target = $panel;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl ::std::ops::DerefMut for $name {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }

        impl $name {
            /// stands for what the page is made of, as `lanlord_app::view::fingerprint` has it.
            pub fn key(
                &self,
                services: &::lanlord_app::services::Services,
                host: Option<&str>,
            ) -> u64 {
                ::lanlord_app::view::fingerprint(&self.0.preview(services, host))
            }

            pub fn show(
                &mut self,
                ui: &mut ::eframe::egui::Ui,
                services: &::lanlord_app::services::Services,
                host: Option<&str>,
            ) {
                // a page being measured is drawn unseen: it asks for nothing and answers nothing
                let measuring = !ui.is_visible();
                let nodes = if measuring {
                    self.0.preview(services, host)
                } else {
                    self.0.view(services, host)
                };
                if let (Some(msg), Some(host)) = (crate::view::show(ui, &nodes), host) {
                    if !measuring {
                        self.0.update(msg, services, host);
                    }
                }
            }

            #[doc = $box_doc]
            pub fn dialog(
                &mut self,
                ctx: &::eframe::egui::Context,
                services: &::lanlord_app::services::Services,
                host: Option<&str>,
            ) {
                let Some(dialog) = self.0.dialog() else {
                    return;
                };
                if let Some(msg) = crate::view::dialog(ctx, &dialog) {
                    self.0.update_dialog(msg, services, host);
                }
            }
        }
    };
}
