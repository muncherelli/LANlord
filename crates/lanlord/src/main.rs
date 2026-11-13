use std::io::IsTerminal;

use anyhow::{bail, Result};
use clap::{CommandFactory, Parser, Subcommand};

#[derive(Parser)]
#[command(name = "lanlord", version, about = "all your LAN are belong to you")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Open the native window
    Gui,
    /// Run the web server
    Webui,
    /// Install the gateway payload on a host
    Bootstrap { host: String },
    /// Push the encrypted config to a host and apply it
    Apply { host: String },
    /// Add LANlord to the desktop app menu
    InstallLauncher,
    /// Remove LANlord from the desktop app menu
    UninstallLauncher,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None => {
            if !std::io::stdout().is_terminal() {
                Cli::command().print_help()?;
                return Ok(());
            }
            bail!("tui is not implemented yet")
        }
        Some(Command::Gui) => bail!("gui is not implemented yet"),
        Some(Command::Webui) => bail!("webui is not implemented yet"),
        Some(Command::Bootstrap { host }) => lanlord_core::bootstrap::bootstrap(&host),
        Some(Command::Apply { host }) => lanlord_core::apply::apply(&host),
        Some(Command::InstallLauncher | Command::UninstallLauncher) => {
            bail!("launcher setup is not implemented yet")
        }
    }
}
