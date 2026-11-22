//! the nt4 domain's settings tab: the panel lives in `lanlord_app::nt4_panel`; this draws it
//! and its box with egui.

panel_glue!(
    Nt4Panel,
    lanlord_app::nt4_panel::Nt4Panel,
    "the box for a machine name or the administrator password, drawn on top of everything."
);
