//! the LANlord app: everything the gui, the tui and the web ui share. it owns the state,
//! the layout (a renderer-independent description of the desktop, menus and windows) and
//! the actions; each front end only draws that description and turns input back into
//! actions.

pub mod account_window;
pub mod admin_setup;
pub mod appearance;
pub mod apply_progress;
pub mod ask;
#[cfg(target_arch = "wasm32")]
pub mod browser;
pub mod changelog;
pub mod clock_window;
pub mod command_panel;
pub mod compat;
pub mod components;
pub mod computers_panel;
pub mod console;
pub mod demo_clock;
pub mod demo_monitor;
pub mod desktop;
#[cfg(feature = "fake")]
pub mod devtools;
pub mod dns_panel;
pub mod form;
pub mod gateway;
pub mod hosting_tree;
pub mod hostlist;
pub mod hosts_window;
pub mod log;
pub mod login;
pub mod logon_panel;
pub mod menu;
pub mod message;
pub mod module_window;
pub mod monitor;
pub mod my_server;
pub mod networks_window;
pub mod nt4_panel;
pub mod permissions_window;
pub mod pin;
pub mod prompt;
#[cfg(feature = "native")]
pub mod rebuild;
pub mod remote;
pub mod replay;
pub mod services;
pub mod settings_window;
pub mod tasks;
pub mod update_box;
#[cfg(feature = "native")]
pub mod updates;
pub mod users_panel;
pub mod view;
pub mod view_wizard;
pub mod wake;
pub mod welcome;
