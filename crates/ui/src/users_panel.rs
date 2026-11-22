//! the logins of the nt4 domain: the panel lives in `lanlord_app::users_panel`; this draws it
//! and its box with egui.

panel_glue!(
    UsersPanel,
    lanlord_app::users_panel::UsersPanel,
    "the box for adding, changing or deleting a login, drawn on top of everything."
);
