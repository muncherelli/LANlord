use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use lanlord_core::bootstrap::{bootstrap, AnsibleMode, Options};
use lanlord_core::exec::{exec, Target};
use lanlord_core::users::Change;
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
enum UsersCommand {
    /// list the logins in your settings
    List,
    /// add a login; asks for its password
    Add {
        name: String,
        /// also make the login a domain admin
        #[arg(long)]
        admin: bool,
    },
    /// make an existing login a domain admin
    Promote { name: String },
    /// take domain admin away from a login
    Demote { name: String },
    /// change a login's password
    Passwd { name: String },
    /// remove a login
    Remove { name: String },
}

#[derive(Subcommand)]
enum AdminCommand {
    /// list the members of lanlord-admins on the gateway
    List,
    /// add an existing gateway user to lanlord-admins
    Add { user: String },
    /// remove a user from lanlord-admins; the last member stays
    Remove { user: String },
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
    Apply {
        host: String,
        /// run only this module's tasks (repeatable); see the tui desktop
        #[arg(long = "module", value_name = "ID")]
        modules: Vec<String>,
    },
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
    /// manage the logins of the nt4-domain module
    Users {
        host: String,
        #[command(subcommand)]
        action: UsersCommand,
    },
    /// manage who may administer lanlord on a gateway (the lanlord-admins group)
    Admin {
        host: String,
        #[command(subcommand)]
        action: AdminCommand,
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

/// ask for a new password twice.
fn new_password(name: &str) -> Result<String> {
    let first = rpassword::prompt_password(format!("new password for {name}: "))?;
    let again = rpassword::prompt_password("again: ")?;
    if first != again {
        bail!("the passwords do not match");
    }
    Ok(first)
}

fn users(host: &str, action: UsersCommand) -> Result<()> {
    let identity = crypto::load(&identity_path()?)?;
    let change = match action {
        UsersCommand::List => {
            for user in lanlord_core::users::list_on(host, &identity, &mut ask)? {
                println!("{}", user.name);
            }
            return Ok(());
        }
        UsersCommand::Add { name, admin } => {
            let password = new_password(&name)?;
            let mut changes = vec![Change::SetPassword {
                name: name.clone(),
                password,
            }];
            if admin {
                changes.push(Change::SetAdmin { name, admin: true });
            }
            return lanlord_core::users::change_all(host, &identity, &changes, &mut ask)
                .map(|_| ());
        }
        UsersCommand::Passwd { name } => {
            let password = new_password(&name)?;
            Change::SetPassword { name, password }
        }
        UsersCommand::Promote { name } => Change::SetAdmin { name, admin: true },
        UsersCommand::Demote { name } => Change::SetAdmin { name, admin: false },
        UsersCommand::Remove { name } => Change::Remove { name },
    };
    lanlord_core::users::change(host, &identity, &change, &mut ask).map(|_| ())
}

fn admin(host: &str, action: AdminCommand) -> Result<()> {
    use lanlord_core::admins::{change, list, Action};
    let action = match action {
        AdminCommand::List => {
            for member in list(host, &mut ask)? {
                println!("{member}");
            }
            return Ok(());
        }
        AdminCommand::Add { user } => Action::Add(user),
        AdminCommand::Remove { user } => Action::Remove(user),
    };
    change(host, &action, &mut ask)
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
        Some(Command::Apply { host, modules }) => {
            // no key yet just means no settings; a damaged key is an error
            let identity = crypto::load_default()?;
            lanlord_core::apply::apply(&host, identity.as_ref(), &modules, &mut ask)
        }
        Some(Command::Users { host, action }) => users(&host, action),
        Some(Command::Admin { host, action }) => admin(&host, action),
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
