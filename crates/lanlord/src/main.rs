use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use lanlord_core::bootstrap::{bootstrap, AnsibleMode, Options};
use lanlord_core::exec::{exec, Target};
use lanlord_core::ops::{execute, Op, Reply, Response};
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

#[derive(Clone, Copy, ValueEnum)]
enum PowerChoice {
    Reboot,
    Shutdown,
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
    /// let a login write to the software share without being a domain admin, or stop
    Software {
        name: String,
        #[arg(value_enum)]
        state: Switch,
    },
    /// change a login's password
    Passwd { name: String },
    /// remove a login
    Remove { name: String },
}

#[derive(Clone, Copy, ValueEnum)]
enum Switch {
    On,
    Off,
}

#[derive(Subcommand)]
enum ClockCommand {
    /// show the server's time, time zone and internet time
    Show,
    /// set the time zone, for example Europe/Berlin
    SetTimezone { zone: String },
    /// set the time, as 'YYYY-MM-DD HH:MM:SS' (internet time must be off)
    SetTime { time: String },
    /// turn internet time on or off, optionally with the servers to use
    Ntp {
        #[arg(value_enum)]
        state: Switch,
        servers: Vec<String>,
    },
    /// synchronize with the time servers now
    Update,
}

#[derive(Subcommand)]
enum MachinesCommand {
    /// list the machine accounts in your settings
    List,
    /// create an account in advance so the machine can join without creating one
    Add { name: String },
    /// remove a machine from your settings (its account stays in the domain)
    Remove { name: String },
    /// set a machine's account back to its first password, so it can join the domain again
    Reset { name: String },
}

#[derive(Subcommand)]
enum LogonCommand {
    /// list the logon scripts in the netlogon share
    List,
    /// print a script (logon for everyone, a login's name for one login, lanlord for the generated one)
    Show { name: String },
    /// store a script read from standard input
    Set { name: String },
}

#[derive(Subcommand)]
enum AdminCommand {
    /// list the members of admins on the gateway
    List,
    /// add an existing gateway user to admins
    Add { user: String },
    /// remove a user from admins; the last member stays
    Remove { user: String },
    /// create a new administrator account (a login, in admins and sudo); asks for its password
    Create {
        name: String,
        /// a public key file to authorize for ssh, or - to read it from standard input
        #[arg(long, value_name = "FILE")]
        ssh_key: Option<String>,
        /// let the new account use sudo without a password
        #[arg(long)]
        passwordless_sudo: bool,
    },
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
    /// manage the machine accounts of the nt4-domain module
    Machines {
        host: String,
        #[command(subcommand)]
        action: MachinesCommand,
    },
    /// edit the logon scripts every domain logon runs
    Logon {
        host: String,
        #[command(subcommand)]
        action: LogonCommand,
    },
    /// show or change a server's time, time zone and internet time
    Clock {
        host: String,
        #[command(subcommand)]
        action: ClockCommand,
    },
    /// remove lanlord from a server: containers, files, service account and sudo rules
    Purge {
        host: String,
        /// also remove the administrator accounts (needs a root login)
        #[arg(long)]
        remove_admins: bool,
        /// skip the prompt by repeating the host name (for scripts)
        #[arg(long, value_name = "HOST")]
        confirm: Option<String>,
    },
    /// list the dhcp leases the gateway has handed out
    Leases { host: String },
    /// reboot or shut down a server
    Power {
        host: String,
        #[arg(value_enum)]
        action: PowerChoice,
        /// skip the prompt by repeating the host name (for scripts)
        #[arg(long, value_name = "HOST")]
        confirm: Option<String>,
    },
    /// show what a server is: its system, uptime, memory and network interfaces
    Facts { host: String },
    /// show the status of each network connection on a server: link, speed, addresses,
    /// gateway, dns, the dhcp lease and traffic
    Networks {
        host: String,
        /// print the status as json
        #[arg(long)]
        json: bool,
    },
    /// read one operation as json on stdin, run it and print the reply as json
    #[command(hide = true)]
    Op { host: String },
    /// turn the nt4-domain software share (S: at logon) on or off, and apply it
    Software {
        host: String,
        #[arg(value_enum)]
        state: Switch,
    },
    /// manage who may administer lanlord on a gateway (the admins group)
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
                match user.roles().join(", ") {
                    roles if roles.is_empty() => println!("{}", user.name),
                    roles => println!("{} ({roles})", user.name),
                }
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
        UsersCommand::Software { name, state } => Change::SetSoftwareAdmin {
            name,
            software_admin: matches!(state, Switch::On),
        },
        UsersCommand::Remove { name } => Change::Remove { name },
    };
    lanlord_core::users::change(host, &identity, &change, &mut ask).map(|_| ())
}

fn machines(host: &str, action: MachinesCommand) -> Result<()> {
    use lanlord_core::nt4::{change_machine, machines_on, MachineChange};
    use lanlord_core::ssh::Session;
    let identity = crypto::load(&identity_path()?)?;
    let change = match action {
        MachinesCommand::List => {
            for name in machines_on(&Session::new(host)?, host, &identity, &mut ask)? {
                println!("{name}");
            }
            return Ok(());
        }
        MachinesCommand::Add { name } => MachineChange::Add(name),
        MachinesCommand::Remove { name } => MachineChange::Remove(name),
        MachinesCommand::Reset { name } => MachineChange::Reset(name),
    };
    change_machine(&Session::new(host)?, host, &identity, &change, &mut ask)
}

fn logon(host: &str, action: LogonCommand) -> Result<()> {
    use lanlord_core::logon::{list, read, read_generated, write, GENERATED};
    let session = lanlord_core::ssh::Session::new(host)?;
    match action {
        LogonCommand::List => {
            for name in list(&session, &mut ask)? {
                println!("{name}");
            }
            Ok(())
        }
        LogonCommand::Show { name } => {
            let found = if name == GENERATED {
                read_generated(&session, &mut ask)?
            } else {
                read(&session, &name, &mut ask)?
            };
            match found {
                Some(text) => Ok(std::io::stdout().write_all(text.as_bytes())?),
                None => bail!("no script named {name} on {host}"),
            }
        }
        LogonCommand::Set { name } => {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
            write(&session, &name, &text, &mut ask)
        }
    }
}

fn clock(host: &str, action: ClockCommand) -> Result<()> {
    use lanlord_core::clock::{self, Change, NtpChange};
    let session = lanlord_core::ssh::Session::new(host)?;
    let change = match action {
        ClockCommand::Show => {
            let state = clock::read(&session)?;
            println!("{}", clock::describe(&state));
            return Ok(());
        }
        ClockCommand::Update => {
            for line in clock::update_now(&session, &mut ask)? {
                println!("{line}");
            }
            return Ok(());
        }
        ClockCommand::SetTimezone { zone } => Change {
            timezone: Some(zone),
            ..Change::default()
        },
        ClockCommand::SetTime { time } => Change {
            time: Some(clock::valid_time(&time)?),
            ..Change::default()
        },
        ClockCommand::Ntp { state, servers } => Change {
            ntp: Some(NtpChange {
                enabled: matches!(state, Switch::On),
                servers: clock::parse_servers(&servers.join(" "))?,
            }),
            ..Change::default()
        },
    };
    let identity = crypto::load_default()?;
    let (said, stored) = clock::apply_stored(&session, host, identity.as_ref(), &change, &mut ask)?;
    for line in said {
        println!("{line}");
    }
    if stored == clock::Stored::NoKey {
        eprintln!(
            "note: this change is not kept in your settings (no key yet), so an apply may undo it"
        );
    }
    Ok(())
}

fn purge(host: &str, remove_admins: bool, confirm: Option<String>) -> Result<()> {
    use lanlord_core::purge::{preview, purge, PurgeOptions};
    let session = lanlord_core::ssh::Session::new(host)?;
    let options = PurgeOptions { remove_admins };
    println!("this would remove from {host}:");
    for line in preview(&session, options, &mut ask)? {
        println!("  {line}");
    }
    let typed = confirmed("type the host name to remove lanlord from it: ", confirm)?;
    if typed != host {
        bail!("the host name does not match; nothing was removed");
    }
    for line in purge(&session, host, options, &mut ask)? {
        println!("{line}");
    }
    if let Some(path) = lanlord_core::hosts::config_path() {
        let _ = lanlord_core::hosts::remove(&path, host);
    }
    Ok(())
}

/// the host name typed at a prompt, or the one given with --confirm.
fn confirmed(question: &str, confirm: Option<String>) -> Result<String> {
    if let Some(typed) = confirm {
        return Ok(typed);
    }
    print!("{question}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

fn leases(host: &str) -> Result<()> {
    let Reply::Leases(leases) = execute(host, &Op::Leases, &mut ask)? else {
        bail!("the server answered something other than leases");
    };
    if leases.is_empty() {
        println!("no leases on {host}");
        return Ok(());
    }
    for lease in leases {
        let until = match lease.expires {
            0 => "never".to_string(),
            secs => format!(
                "{} UTC",
                lanlord_core::clock::local_text(i64::try_from(secs).unwrap_or(i64::MAX), 0)
            ),
        };
        let name = if lease.name.is_empty() {
            "-"
        } else {
            &lease.name
        };
        println!(
            "{:<15}  {:<17}  {:<20}  expires {until}",
            lease.address, lease.mac, name
        );
    }
    Ok(())
}

fn networks(host: &str, json: bool) -> Result<()> {
    let Reply::Interfaces(all) = execute(host, &Op::NetworkStatus, &mut ask)? else {
        bail!("the server answered something other than its network status");
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&all)?);
        return Ok(());
    }
    let utc = |secs: i64| format!("{} UTC", lanlord_core::clock::local_text(secs, 0));
    for (i, iface) in all.iter().enumerate() {
        if i > 0 {
            println!();
        }
        let role = if iface.default_route {
            " (external network)"
        } else {
            ""
        };
        println!("{}{role}", iface.name);
        println!("  {:<22}{}", "status:", iface.connection.text());
        println!("  {:<22}{}", "speed:", iface.speed());
        println!(
            "  {:<22}{} bytes, {} packets",
            "sent:", iface.counters.sent_bytes, iface.counters.sent_packets
        );
        println!(
            "  {:<22}{} bytes, {} packets",
            "received:", iface.counters.received_bytes, iface.counters.received_packets
        );
        for (label, value) in iface.details(utc) {
            let label = if label.is_empty() {
                String::new()
            } else {
                format!("{}:", label.to_lowercase())
            };
            println!("  {label:<22}{value}");
        }
    }
    Ok(())
}

fn power(host: &str, action: PowerChoice, confirm: Option<String>) -> Result<()> {
    use lanlord_core::power::{Outcome, Power};
    let (power, verb) = match action {
        PowerChoice::Reboot => (Power::Reboot, "reboot"),
        PowerChoice::Shutdown => (Power::PowerOff, "shut down"),
    };
    let typed = confirmed(&format!("type the host name to {verb} it: "), confirm)?;
    if typed != host {
        bail!("the host name does not match; nothing was done");
    }
    match execute(host, &Op::Power { power }, &mut ask)? {
        Reply::Outcome(Outcome::Done) => println!("{host} is {}", power.doing()),
        Reply::Outcome(Outcome::Dropped) => println!(
            "{host} dropped the connection, which is what {} looks like",
            power.doing()
        ),
        _ => bail!("the server answered something other than a power outcome"),
    }
    Ok(())
}

fn facts(host: &str) -> Result<()> {
    let Reply::Facts(facts) = execute(host, &Op::ServerFacts, &mut ask)? else {
        bail!("the server answered something other than its facts");
    };
    println!("name:          {}", facts.hostname);
    println!("system:        {}", facts.os);
    println!("kernel:        {}", facts.kernel);
    println!("up for:        {}", facts.uptime());
    let root = if facts.is_root { " (root)" } else { "" };
    println!("signed in as:  {}{root}", facts.user);
    println!("processors:    {}", facts.cpus);
    println!(
        "memory:        {} MB free of {} MB",
        facts.mem_available_mb, facts.mem_total_mb
    );
    let lanlord = match (facts.lanlord_installed, &facts.lanlord_version) {
        (false, _) => "not installed".to_string(),
        (true, Some(version)) => format!("installed, version {version}"),
        (true, None) => "installed".to_string(),
    };
    println!("lanlord:       {lanlord}");
    if let Some(when) = &facts.last_applied {
        println!("last applied:  {when}");
    }
    for iface in &facts.interfaces {
        let route = if iface.default_route {
            ", default route"
        } else {
            ""
        };
        println!(
            "interface:     {} {} ({}{route}) {}",
            iface.name,
            iface.link(),
            iface.state,
            iface.addresses.join(" ")
        );
    }
    Ok(())
}

/// one operation in, one reply out, both as json; what the web ui backend calls.
fn op(host: &str) -> Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let response: Response = match serde_json::from_str::<Op>(&input) {
        Ok(op) => execute(host, &op, &mut ask).into(),
        Err(e) => Response::Error(format!("not an operation: {e}")),
    };
    println!("{}", serde_json::to_string(&response)?);
    if matches!(response, Response::Error(_)) {
        std::process::exit(1);
    }
    Ok(())
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
        AdminCommand::Create {
            name,
            ssh_key,
            passwordless_sudo,
        } => {
            let key = ssh_key
                .as_deref()
                .map(lanlord_core::account::read_key)
                .transpose()?;
            let password = new_password(&name)?;
            let session = lanlord_core::ssh::Session::new(host)?;
            return lanlord_core::account::create_admin(
                &session,
                host,
                &name,
                &password,
                key.as_deref(),
                passwordless_sudo,
                &mut ask,
            );
        }
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
    // ssh starts this program to ask for a password during a gui logon
    if let Some(code) = lanlord_core::signin::askpass_main() {
        std::process::exit(code);
    }
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
        Some(Command::Machines { host, action }) => machines(&host, action),
        Some(Command::Logon { host, action }) => logon(&host, action),
        Some(Command::Clock { host, action }) => clock(&host, action),
        Some(Command::Purge {
            host,
            remove_admins,
            confirm,
        }) => purge(&host, remove_admins, confirm),
        Some(Command::Leases { host }) => leases(&host),
        Some(Command::Power {
            host,
            action,
            confirm,
        }) => power(&host, action, confirm),
        Some(Command::Facts { host }) => facts(&host),
        Some(Command::Networks { host, json }) => networks(&host, json),
        Some(Command::Op { host }) => op(&host),
        Some(Command::Software { host, state }) => {
            let identity = crypto::load(&identity_path()?)?;
            let session = lanlord_core::ssh::Session::new(&host)?;
            let on = matches!(state, Switch::On);
            lanlord_core::nt4::set_software(&session, &host, &identity, on, &mut ask)
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Command {
        let mut all = vec!["lanlord"];
        all.extend_from_slice(args);
        Cli::try_parse_from(all)
            .expect("parses")
            .command
            .expect("a command")
    }

    #[test]
    fn leases_and_facts_take_a_host() {
        assert!(matches!(parse(&["leases", "gw"]), Command::Leases { host } if host == "gw"));
        assert!(matches!(parse(&["facts", "gw"]), Command::Facts { host } if host == "gw"));
        assert!(matches!(
            parse(&["networks", "gw", "--json"]),
            Command::Networks { host, json: true } if host == "gw"
        ));
    }

    #[test]
    fn power_takes_an_action_and_an_optional_confirmation() {
        assert!(matches!(
            parse(&["power", "gw", "reboot"]),
            Command::Power { host, action: PowerChoice::Reboot, confirm: None } if host == "gw"
        ));
        assert!(matches!(
            parse(&["power", "gw", "shutdown", "--confirm", "gw"]),
            Command::Power { action: PowerChoice::Shutdown, confirm: Some(c), .. } if c == "gw"
        ));
        let mut wrong = vec!["lanlord", "power", "gw", "explode"];
        assert!(Cli::try_parse_from(&wrong).is_err());
        wrong.truncate(3);
        assert!(
            Cli::try_parse_from(&wrong).is_err(),
            "the action is required"
        );
    }

    #[test]
    fn op_is_hidden_from_help() {
        assert!(matches!(parse(&["op", "gw"]), Command::Op { host } if host == "gw"));
        let help = Cli::command().render_help().to_string();
        assert!(help.contains("leases") && help.contains("facts") && help.contains("power"));
        assert!(
            !help.lines().any(|l| l.trim_start().starts_with("op ")),
            "{help}"
        );
    }
}
