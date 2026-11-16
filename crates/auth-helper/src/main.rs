//! lanlord-auth-helper checks one web ui login against pam and the `admins` group.
//!
//! it runs as root, one process per connection, started by systemd from the lanlord-auth socket
//! that only the web ui's account may open. the web ui runs unprivileged, and an unprivileged
//! process cannot check another user's password with pam_unix, so this small helper does just
//! that and nothing else. see the protocol module for what goes over the socket.

// only the linux build answers logins; elsewhere these are compiled for their tests
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod check;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod protocol;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod pam;

use std::process::ExitCode;

#[cfg(target_os = "linux")]
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ExitCode::from(linux::run(&args).exit_code())
}

// the web ui only runs on a linux gateway; elsewhere the helper just says so
#[cfg(not(target_os = "linux"))]
fn main() -> ExitCode {
    let verdict = protocol::Verdict::Error("unsupported");
    println!("{}", verdict.line());
    ExitCode::from(verdict.exit_code())
}
