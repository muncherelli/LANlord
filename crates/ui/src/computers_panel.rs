//! the computers tab of the nt4 domain: the panel lives in `lanlord_app::computers_panel`; this
//! draws it and its box with egui.

panel_glue!(
    ComputersPanel,
    lanlord_app::computers_panel::ComputersPanel,
    "the box for a command or an iis preset, drawn on top of everything."
);

impl ComputersPanel {
    /// the panel of the hosting window, which runs the iis presets.
    pub fn hosting() -> Self {
        let mut panel = Self::default();
        panel.set_mode(lanlord_app::computers_panel::Mode::Hosting);
        panel
    }
}
