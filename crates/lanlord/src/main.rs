use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use lanlord_core::bootstrap::{bootstrap, AnsibleMode, Options};
use lanlord_core::exec::{exec, Target};
use lanlord_core::{crypto, paths};

#[derive(Parser)]
#[command(name = "lanlord", version, about = "all your LAN are belong to you")]
struct Cli {
    /// print debug lines about what lanlord is doing
    #[arg(short, long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Clone, Copy, ValueEnum)]
enum AnsibleChoice {
    Auto,
    Local,
    Remote,
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// encrypt a settings file with your key and store it on the gateway
    Push { host: String, file: PathBuf },
    /// fetch the gateway's settings and print them decrypted
    Show { host: String },
}

#[derive(Subcommand)]
enum Command {
    /// open the native window
    Gui,
    /// run the web server
    Webui,
    /// install the gateway payload on a host
    Bootstrap {
        host: String,
        /// where ansible runs: auto picks local when ansible is installed here
        #[arg(long, value_enum, default_value = "auto")]
        ansible: AnsibleChoice,
        /// let the ssh user run sudo without a password
        #[arg(long)]
        passwordless_sudo: bool,
    },
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
    /// create the key that encrypts your settings
    Keygen,
    /// store and read the encrypted settings on a gateway
    #[command(subcommand)]
    Config(ConfigCommand),
    /// add LANlord to the start menu (windows), applications (macos) or the app menu (linux)
    InstallLauncher,
    /// remove the launcher added by install-launcher
    UninstallLauncher,
}

/// ask for a password on the terminal.
fn ask(what: &str) -> Result<String> {
    Ok(rpassword::prompt_password(format!("{what}: "))?)
}

fn identity_path() -> Result<PathBuf> {
    paths::identity_path().ok_or_else(|| anyhow::anyhow!("could not find a config directory"))
}

fn keygen() -> Result<()> {
    let path = identity_path()?;
    let recipient = crypto::generate(&path)?;
    println!("created {}", path.display());
    println!("public key: {recipient}");
    println!("back this file up: without it your settings cannot be read");
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    lanlord_core::log::set_verbose(cli.verbose);
    match cli.command {
        None => {
            if !std::io::stdout().is_terminal() {
                Cli::command().print_help()?;
                return Ok(());
            }
            lanlord_tui::run()
        }
        #[cfg(feature = "gui")]
        Some(Command::Gui) => lanlord_ui::run(),
        #[cfg(not(feature = "gui"))]
        Some(Command::Gui) => bail!("this build has no gui; install the desktop build"),
        Some(Command::Webui) => bail!("webui is not implemented yet"),
        Some(Command::Bootstrap {
            host,
            ansible,
            passwordless_sudo,
        }) => bootstrap(
            &Options {
                host,
                ansible: match ansible {
                    AnsibleChoice::Auto => AnsibleMode::detect(),
                    AnsibleChoice::Local => AnsibleMode::Local,
                    AnsibleChoice::Remote => AnsibleMode::Remote,
                },
                passwordless_sudo,
            },
            &mut ask,
        ),
        Some(Command::Apply { host }) => {
            // no key yet just means no settings; a damaged key is an error
            let identity = match identity_path()? {
                path if path.exists() => Some(crypto::load(&path)?),
                _ => None,
            };
            lanlord_core::apply::apply(&host, identity.as_ref(), &mut ask)
        }
        Some(Command::Keygen) => keygen(),
        Some(Command::Config(ConfigCommand::Push { host, file })) => {
            let identity = crypto::load(&identity_path()?)?;
            let settings = std::fs::read(&file)?;
            lanlord_core::config::push(&host, &identity, &settings, &mut ask)
        }
        Some(Command::Config(ConfigCommand::Show { host })) => {
            let identity = crypto::load(&identity_path()?)?;
            match lanlord_core::config::pull(&host, &identity, &mut ask)? {
                Some(settings) => Ok(std::io::stdout().write_all(&settings)?),
                None => bail!("no settings on {host}; run lanlord config push first"),
            }
        }
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
