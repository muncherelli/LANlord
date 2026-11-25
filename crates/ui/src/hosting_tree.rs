//! the hosting window: the panel lives in `lanlord_app::hosting_tree`; this draws it with egui.

panel_glue!(
    HostingTree,
    lanlord_app::hosting_tree::HostingTree,
    "the hosting window has no box: this does nothing."
);
