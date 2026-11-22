//! the computers tab of the nt4 domain: the panel lives in `lanlord_app::computers_panel`; this
//! draws it and its box with egui.

panel_glue!(
    ComputersPanel,
    lanlord_app::computers_panel::ComputersPanel,
    "the box for a command or an iis preset, drawn on top of everything."
);
