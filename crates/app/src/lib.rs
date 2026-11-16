//! the LANlord app: everything the gui, the tui and the web ui share. it owns the state,
//! the layout (a renderer-independent description of the desktop, menus and windows) and
//! the actions; each front end only draws that description and turns input back into
//! actions.

pub mod ask;
pub mod demo_clock;
pub mod desktop;
pub mod gateway;
pub mod hostlist;
pub mod log;
pub mod menu;
pub mod my_server;
pub mod remote;
pub mod services;
pub mod tasks;
pub mod view;
pub mod wake;
