//! the LANlord app: everything the gui, the tui and the web ui share. it owns the state,
//! the layout (a renderer-independent description of the desktop, menus and windows) and
//! the actions; each front end only draws that description and turns input back into
//! actions.

pub mod console;
pub mod demo_clock;
pub mod desktop;
pub mod form;
pub mod gateway;
pub mod hostlist;
pub mod hosts_window;
pub mod logon_panel;
pub mod menu;
pub mod module_window;
pub mod my_server;
pub mod prompt;
pub mod remote;
pub mod services;
pub mod settings_window;
pub mod tasks;
pub mod users_panel;
pub mod view;
pub mod wake;
