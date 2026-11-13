use std::io::IsTerminal;

use anyhow::{bail, Result};
use clap::{CommandFactory, Parser, Subcommand};
use lanlord_core::exec::{exec, Target};

#[derive(Parser)]
#[command(name = "lanlord", version, about = "all your LAN are belong to you")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// open the native window
    Gui,
    /// run the web server
    Webui,
    /// install the gateway payload on a host
    Bootstrap { host: String },
    /// push the encrypted config to a host and apply it
    Apply { host: String },
    /// run a command on the gateway, or on a lan machine through it with --via
    Run {
        /// the gateway, or the lan machine when --via is given
        target: String,
        /// reach the target through this gateway
        #[arg(long)]
        via: Option<String>,
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
    /// add LANlord to the start menu (windows), applications (macos) or the app menu (linux)
    InstallLauncher,
    /// remove the launcher added by install-launcher
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
        Some(Command::Run {
            target,
            via,
            command,
        }) => {
            let target = match via {
                Some(gateway) => Target::Machine {
                    gateway,
                    name: target,
                },
                None => Target::Gateway { host: target },
            };
            std::process::exit(exec(&target, &command)?);
        }
        Some(Command::InstallLauncher | Command::UninstallLauncher) => {
            bail!("launcher setup is not implemented yet")
        }
    }
}
