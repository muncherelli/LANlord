//! the data every LANlord front end shares: settings, modules, servers, users and the rest,
//! with their parsing and validation. nothing here touches the network, files or processes,
//! so it also builds for the browser.

pub mod apply;
pub mod bridge;
pub mod clock;
pub mod compat;
pub mod components;
pub mod doctor;
pub mod lan_networks;
pub mod leases;
pub mod log;
pub mod logon;
pub mod modules;
pub mod netstatus;
pub mod network;
pub mod networks;
pub mod nt4;
pub mod ops;
pub mod passwords;
pub mod power;
pub mod routes;
pub mod secret;
pub mod server;
pub mod settings;
pub mod signin;
pub mod update;
pub mod users;
