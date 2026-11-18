mod launcher;
#[cfg(feature = "webui")]
mod operations_service;
#[cfg(feature = "webui")]
mod webui;

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use lanlord_core::bootstrap::AskPassword;
use lanlord_core::bootstrap::{bootstrap, AnsibleMode, Options};
use lanlord_core::exec::{exec, Target};
use lanlord_core::operations::{execute, Operation, Reply, Request, Response};
use lanlord_core::paths;
use lanlord_core::users::Change;

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
enum NetworksCommand {
    /// show the status of each network connection on a server: link, speed, addresses,
    /// gateway, dns, the dhcp lease and traffic
    Status {
        /// print the status as json
        #[arg(long)]
        json: bool,
    },
    /// list the lanlord networks in your settings
    List,
    /// add a lanlord network on an interface, with its own dnsmasq
    Add {
        name: String,
        /// the interface it is on, like eth2
        #[arg(long)]
        interface: String,
        /// the server's address there, like 192.168.50.1/24
        #[arg(long)]
        address: String,
        #[arg(long)]
        mtu: Option<u32>,
        /// give it a dhcp server, like 192.168.50.100-192.168.50.200
        #[arg(long, value_name = "START-END")]
        dhcp_range: Option<String>,
        /// the lease time, like 12h or 7d (needs a dhcp range)
        #[arg(long)]
        lease: Option<String>,
        /// the dns zone; default is the network's name
        #[arg(long)]
        zone: Option<String>,
    },
    /// change a network
    Set {
        name: String,
        #[arg(long)]
        interface: Option<String>,
        #[arg(long)]
        address: Option<String>,
        #[arg(long)]
        mtu: Option<u32>,
        /// take the mtu setting away
        #[arg(long)]
        no_mtu: bool,
        #[arg(long)]
        zone: Option<String>,
        #[arg(long, value_name = "START-END")]
        dhcp_range: Option<String>,
        #[arg(long)]
        lease: Option<String>,
        /// turn the dhcp server off
        #[arg(long)]
        no_dhcp: bool,
    },
    /// remove a network and its dnsmasq
    Remove { name: String },
    /// add or remove a static dhcp lease on a network
    Lease {
        name: String,
        #[command(subcommand)]
        action: LeaseCommand,
    },
}

#[derive(Subcommand)]
enum LeaseCommand {
    /// give a machine a fixed address
    Add {
        #[arg(long)]
        host: String,
        #[arg(long)]
        mac: String,
        #[arg(long)]
        address: String,
    },
    /// take a machine's fixed address away
    Remove {
        #[arg(long)]
        host: String,
    },
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

/// where the web ui files of an install come from: a local build, or this version's release.
#[derive(clap::Args)]
struct WebFiles {
    /// the linux server binary to install (a headless build with the webui feature); give it
    /// with --web-dist and --auth-helper, or leave all three out to download this version's release
    #[arg(long, value_name = "PATH")]
    server_binary: Option<PathBuf>,
    /// the browser build to install (a folder with index.html, from tools/dev.sh web)
    #[arg(long, value_name = "DIR")]
    web_dist: Option<PathBuf>,
    /// the lanlord-auth-helper binary for the web ui login
    #[arg(long, value_name = "PATH")]
    auth_helper: Option<PathBuf>,
}

impl WebFiles {
    fn source(self) -> Result<lanlord_core::webinstall::WebSource> {
        lanlord_core::webinstall::WebSource::choose(
            self.server_binary,
            self.web_dist,
            self.auth_helper,
        )
    }
}

#[derive(Subcommand)]
enum BridgeMode {
    /// answer requests until stdin ends (the default)
    Serve,
    /// greet and exit, to time the round trip
    Hello,
}

#[derive(Subcommand)]
enum Command {
    /// open the native window
    Gui,
    /// run the web server on a gateway (caddy serves it over https on port 8443)
    Webui {
        /// the unix socket to listen on
        #[arg(long, value_name = "PATH", default_value = "/run/lanlord-web/web.sock")]
        socket: PathBuf,
        /// listen on this tcp address instead of the socket, to try the server out
        #[arg(long, value_name = "ADDRESS")]
        listen: Option<std::net::SocketAddr>,
        /// the browser build to serve (default: the web folder beside the binary's folder)
        #[arg(long = "static", value_name = "DIR")]
        static_dir: Option<PathBuf>,
        /// the operations service's unix socket
        #[arg(
            long,
            value_name = "PATH",
            default_value = "/run/lanlord-operations/operations.sock"
        )]
        operations_socket: PathBuf,
    },
    /// run the root service that carries out the operations the web server forwards
    OperationsService {
        /// the unix socket to listen on
        #[arg(
            long,
            value_name = "PATH",
            default_value = "/run/lanlord-operations/operations.sock"
        )]
        socket: PathBuf,
        /// the unix socket the accounts of this machine connect to, each as itself
        #[arg(
            long,
            value_name = "PATH",
            default_value = "/run/lanlord-operations/local.sock"
        )]
        local_socket: PathBuf,
        /// the uid of the web server's account; only it and root may connect
        #[arg(long, value_name = "UID")]
        allowed_uid: u32,
        /// the gateway's settings key, made here on first start (root only)
        #[arg(
            long,
            value_name = "PATH",
            default_value = "/var/lib/lanlord-operations/identity"
        )]
        identity: PathBuf,
    },
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
    /// manage the lanlord networks (interfaces, addresses, dhcp and static leases) or show the
    /// status of every network connection
    Networks {
        host: String,
        #[command(subcommand)]
        action: NetworksCommand,
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
    /// install lanlord on a server, resuming an install that was interrupted
    Install {
        host: String,
        #[command(flatten)]
        web: WebFiles,
    },
    /// upgrade lanlord on an installed server in place: the server binary, the browser build
    /// and the playbooks, then restart the services that changed; nothing is removed
    Upgrade {
        host: String,
        #[command(flatten)]
        web: WebFiles,
        /// skip the prompt
        #[arg(long)]
        yes: bool,
        /// go ahead even when the services are already not answering (nothing is rolled back)
        #[arg(long)]
        even_if_down: bool,
    },
    /// purge a server and install lanlord on it again
    Reinstall {
        host: String,
        /// repeat the host name to confirm that everything on it is removed
        #[arg(long, value_name = "HOST")]
        confirm: String,
        #[command(flatten)]
        web: WebFiles,
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
    /// check what a server can do for lanlord (docker, networks, tunnels) and how to fix the rest
    #[command(alias = "check")]
    Doctor {
        host: String,
        /// print the report as json
        #[arg(long)]
        json: bool,
    },
    /// read one operation as json on stdin, run it and print the reply as json
    #[command(name = "op", hide = true)]
    Operation { host: String },
    /// run typed operations on this gateway for a controller; json frames, one per line, on
    /// stdin and stdout (what lanlord starts over ssh; sudo for the ones that change things)
    #[command(hide = true)]
    Bridge {
        #[command(subcommand)]
        mode: Option<BridgeMode>,
    },
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
    /// seal your settings to the gateway's own key, once, when an older version of this
    /// program sealed them to a key kept on this machine
    MoveSecrets {
        /// the server's address
        host: String,
    },
    /// carry out one operation as the account that ran sudo; the gateway's own door for the
    /// operations that need its key (not meant to be run by hand)
    #[command(hide = true)]
    Operate,
    /// store and read the encrypted settings on a gateway
    #[command(subcommand)]
    Config(ConfigCommand),
    /// add LANlord to the start menu (windows), applications (macos) or the app menu (linux)
    InstallLauncher {
        /// show what would be written without writing anything
        #[arg(long)]
        dry_run: bool,
        /// replace launcher files that install-launcher didn't write
        #[arg(long)]
        force: bool,
    },
    /// remove the launcher added by install-launcher
    UninstallLauncher {
        /// show what would be removed without removing anything
        #[arg(long)]
        dry_run: bool,
    },
}

/// ask for a password on the terminal.
fn ask(what: &str) -> Result<String> {
    Ok(rpassword::prompt_password(format!("{what}: "))?)
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
    let change = match action {
        UsersCommand::List => {
            for user in lanlord_core::remote::users_on(host, &mut ask)? {
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
            return lanlord_core::remote::change_users(host, &changes, &mut ask).map(|_| ());
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
    lanlord_core::remote::change_users(host, &[change], &mut ask).map(|_| ())
}

fn machines(host: &str, action: MachinesCommand) -> Result<()> {
    use lanlord_core::nt4::{edit_machines, valid_machine, MachineChange, Nt4Settings};
    use lanlord_core::operations::{execute, Operation, Reply};
    let change = match action {
        MachinesCommand::List => {
            let document = lanlord_core::remote::document(host, &mut ask)?;
            let settings = lanlord_core::settings::Settings::parse(&document.plain)?;
            for name in Nt4Settings::from_settings(&settings)?.machines {
                println!("{name}");
            }
            return Ok(());
        }
        MachinesCommand::Add { name } => MachineChange::Add(name),
        MachinesCommand::Remove { name } => MachineChange::Remove(name),
        MachinesCommand::Reset { name } => {
            if !valid_machine(&name) {
                bail!("{name} is not a valid machine name (up to 15 letters, digits or -)");
            }
            return match execute(host, &Operation::ResetMachine { name }, &mut ask)? {
                Reply::Done => Ok(()),
                _ => bail!("unexpected answer"),
            };
        }
    };
    let modules = ["nt4-domain".to_string()];
    lanlord_core::remote::edit(
        host,
        &modules,
        |plain| edit_machines(plain, &change),
        &mut ask,
    )
}

fn networks(host: &str, action: NetworksCommand) -> Result<()> {
    match action {
        NetworksCommand::Status { json } => network_status(host, json),
        edit => change_networks(host, edit),
    }
}

fn change_networks(host: &str, action: NetworksCommand) -> Result<()> {
    use lanlord_core::lan::{
        add_edit, edit_settings, read_networks, render_list, set_edit, AddArgs, Edit, SetArgs,
        MODULES,
    };
    use lanlord_core::settings::StaticLease;
    let edit = match action {
        NetworksCommand::Status { .. } => unreachable!("shown by `networks`"),
        NetworksCommand::List => {
            let document = lanlord_core::remote::document(host, &mut ask)?;
            for line in render_list(&read_networks(&document.plain)?) {
                println!("{line}");
            }
            return Ok(());
        }
        NetworksCommand::Add {
            name,
            interface,
            address,
            mtu,
            dhcp_range,
            lease,
            zone,
        } => add_edit(&AddArgs {
            name,
            interface,
            address,
            mtu,
            dhcp_range,
            lease,
            zone,
        })?,
        NetworksCommand::Set {
            name,
            interface,
            address,
            mtu,
            no_mtu,
            zone,
            dhcp_range,
            lease,
            no_dhcp,
        } => set_edit(
            &name,
            &SetArgs {
                interface,
                address,
                mtu,
                no_mtu,
                zone,
                dhcp_range,
                lease,
                no_dhcp,
            },
        )?,
        NetworksCommand::Remove { name } => Edit::Remove(name),
        NetworksCommand::Lease { name, action } => match action {
            LeaseCommand::Add { host, mac, address } => Edit::LeaseAdd {
                network: name,
                lease: StaticLease {
                    name: host,
                    mac,
                    address,
                },
            },
            LeaseCommand::Remove { host } => Edit::LeaseRemove {
                network: name,
                host,
            },
        },
    };
    let modules = MODULES.map(String::from);
    lanlord_core::remote::edit(
        host,
        &modules,
        |plain| edit_settings(plain, &edit),
        &mut ask,
    )?;
    println!("{}; applied the docker and dns modules", edit.describe());
    Ok(())
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
    let change = lanlord_core::operations::Operation::ClockApply { change };
    let lanlord_core::operations::Reply::ClockApplied {
        lines: said,
        stored,
    } = lanlord_core::operations::execute(host, &change, &mut ask)?
    else {
        bail!("unexpected answer");
    };
    for line in said {
        println!("{line}");
    }
    match stored {
        clock::Stored::NoKey => eprintln!(
            "note: this change is not kept in your settings (no key yet), so an apply may undo it"
        ),
        clock::Stored::NotInstalled => eprintln!(
            "note: LANlord is not installed on this server, so the change is not stored in its settings"
        ),
        clock::Stored::Failed(why) => eprintln!(
            "warning: changed, but could not be remembered in LANlord's settings: {why}"
        ),
        clock::Stored::Yes | clock::Stored::Nothing => {}
    }
    Ok(())
}

fn purge(host: &str, remove_admins: bool, confirm: Option<String>) -> Result<()> {
    use lanlord_core::purge::{preview, purge, PurgeOptions};
    let session = lanlord_core::ssh::Session::new(host)?;
    let options = PurgeOptions { remove_admins };
    // an ssh session: the account it signs in as is the one asking
    let caller = lanlord_core::account::login_user(&session)?.name;
    println!("this would remove from {host}:");
    for line in preview(&session, &caller, options, &mut ask)? {
        println!("  {line}");
    }
    let typed = confirmed("type the host name to remove lanlord from it: ", confirm)?;
    if typed != host {
        bail!("the host name does not match; nothing was removed");
    }
    for line in purge(&session, &caller, host, options, &mut ask)? {
        println!("{line}");
    }
    if let Some(path) = lanlord_core::hosts::config_path() {
        let _ = lanlord_core::hosts::remove(&path, host);
    }
    Ok(())
}

/// print an install's steps and what the server says while it runs.
fn print_install(event: lanlord_core::install::Event) {
    use lanlord_core::install::{Event, Step};
    let number = |step: Step| Step::ALL.iter().position(|s| *s == step).unwrap_or(0) + 1;
    match event {
        Event::StepStarted(step) => {
            println!("[{}/{}] {}", number(step), Step::ALL.len(), step.title())
        }
        Event::Line(line) => println!("  {line}"),
        Event::StepDone(step) => println!("[{}/{}] done", number(step), Step::ALL.len()),
        // the error itself is printed once, by the caller
        Event::Failed { step, .. } => {
            eprintln!("[{}/{}] failed", number(step), Step::ALL.len())
        }
    }
}

/// print an upgrade's steps and what the server says while it runs.
fn print_upgrade(event: lanlord_core::upgrade::Event) {
    use lanlord_core::upgrade::{Event, Step};
    let number = |step: Step| {
        Step::ALL
            .iter()
            .position(|candidate| *candidate == step)
            .unwrap_or(0)
            + 1
    };
    match event {
        Event::StepStarted(step) => {
            println!("[{}/{}] {}", number(step), Step::ALL.len(), step.title())
        }
        Event::Line(line) => println!("  {line}"),
        Event::StepDone(step) => println!("[{}/{}] done", number(step), Step::ALL.len()),
        // the error itself is printed once, by the caller
        Event::Failed { step, .. } => {
            eprintln!("[{}/{}] failed", number(step), Step::ALL.len())
        }
    }
}

fn upgrade(
    host: &str,
    web: lanlord_core::webinstall::WebSource,
    yes: bool,
    even_if_down: bool,
) -> Result<()> {
    if !yes {
        let typed = confirmed(
            "this restarts the lanlord services, and restarting lanlord-operations interrupts any operation that is running (nothing is removed). type the host name to go on: ",
            None,
        )?;
        if typed != host {
            bail!("the host name does not match; nothing was changed");
        }
    }
    lanlord_core::upgrade::run(host, &web, even_if_down, &mut ask, &mut print_upgrade)
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
    let Reply::Leases(leases) = execute(host, &Operation::Leases, &mut ask)? else {
        bail!("the server answered something other than leases");
    };
    if leases.is_empty() {
        println!("no leases on {host}");
    }
    for line in lease_lines(&leases) {
        println!("{line}");
    }
    Ok(())
}

/// one line per lease: address, mac, name and when it ends.
fn lease_lines(leases: &[lanlord_core::leases::Lease]) -> Vec<String> {
    leases
        .iter()
        .map(|lease| {
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
            format!(
                "{:<15}  {:<17}  {:<20}  expires {until}",
                lease.address, lease.mac, name
            )
        })
        .collect()
}

fn network_status(host: &str, json: bool) -> Result<()> {
    let Reply::Interfaces(all) = execute(host, &Operation::NetworkStatus, &mut ask)? else {
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
    use lanlord_core::power::Power;
    let (power, verb) = match action {
        PowerChoice::Reboot => (Power::Reboot, "reboot"),
        PowerChoice::Shutdown => (Power::PowerOff, "shut down"),
    };
    let typed = confirmed(&format!("type the host name to {verb} it: "), confirm)?;
    if typed != host {
        bail!("the host name does not match; nothing was done");
    }
    let reply = execute(host, &Operation::Power { power }, &mut ask)?;
    println!("{}", power_said(host, power, &reply)?);
    Ok(())
}

/// what to say once the server took a reboot or shutdown.
fn power_said(host: &str, power: lanlord_core::power::Power, reply: &Reply) -> Result<String> {
    use lanlord_core::power::Outcome;
    Ok(match reply {
        Reply::Outcome(Outcome::Done) => format!("{host} is {}", power.doing()),
        Reply::Outcome(Outcome::Dropped) => format!(
            "{host} dropped the connection, which is what {} looks like",
            power.doing()
        ),
        _ => bail!("the server answered something other than a power outcome"),
    })
}

fn facts(host: &str) -> Result<()> {
    let Reply::Facts(facts) = execute(host, &Operation::ServerFacts, &mut ask)? else {
        bail!("the server answered something other than its facts");
    };
    for line in fact_lines(&facts) {
        println!("{line}");
    }
    Ok(())
}

fn doctor(host: &str, json: bool) -> Result<()> {
    let Reply::Doctor(report) = execute(host, &Operation::Doctor, &mut ask)? else {
        bail!("the server answered something other than a doctor report");
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for line in doctor_lines(&report) {
            println!("{line}");
        }
    }
    if report.worst() == lanlord_core::doctor::Status::Fail {
        bail!("this server has problems that need fixing; see the fixes above");
    }
    Ok(())
}

/// the doctor report as text: what the machine is, then one line per check with its fix
/// indented under it.
fn doctor_lines(report: &lanlord_core::doctor::Report) -> Vec<String> {
    let system = &report.system;
    let mut lines = vec![
        format!("machine:  {}", or_dash(&system.hostname)),
        format!("runs on:  {}", report.environment.describe()),
        format!(
            "system:   {} {}, kernel {}, init {}",
            or_dash(&system.distro),
            system.distro_version,
            or_dash(&report.kernel),
            or_dash(&system.init)
        ),
        String::new(),
    ];
    let width = report
        .checks
        .iter()
        .map(|c| c.title.len())
        .max()
        .unwrap_or(0);
    for check in &report.checks {
        lines.push(format!(
            "[{}] {:width$}  {}",
            check.status.glyph(),
            check.title,
            check.detail
        ));
        if let Some(fix) = &check.fix {
            lines.push(format!("    {:width$}  fix: {fix}", ""));
        }
    }
    lines
}

fn or_dash(text: &str) -> &str {
    if text.is_empty() {
        "-"
    } else {
        text
    }
}

/// the server's facts as labelled lines.
fn fact_lines(facts: &lanlord_core::server::ServerFacts) -> Vec<String> {
    let root = if facts.is_root { " (root)" } else { "" };
    let lanlord = match (facts.lanlord_installed, &facts.lanlord_version) {
        (false, _) => "not installed".to_string(),
        (true, Some(version)) => format!("installed, version {version}"),
        (true, None) => "installed".to_string(),
    };
    let mut lines = vec![
        format!("name:          {}", facts.hostname),
        format!("system:        {}", facts.os),
        format!("kernel:        {}", facts.kernel),
        format!("up for:        {}", facts.uptime()),
        format!("signed in as:  {}{root}", facts.user),
        format!(
            "processor:     {}",
            if facts.cpu_model.is_empty() {
                "unknown"
            } else {
                &facts.cpu_model
            }
        ),
        format!("processors:    {}", facts.processors()),
        format!(
            "memory:        {} MB free of {} MB{}",
            facts.mem_available_mb,
            facts.mem_total_mb,
            if facts.mem_limited {
                " (limited by container)"
            } else {
                ""
            }
        ),
        format!("lanlord:       {lanlord}"),
    ];
    if let Some(when) = &facts.last_applied {
        lines.push(format!("last applied:  {when}"));
    }
    for iface in &facts.interfaces {
        let route = if iface.default_route {
            ", default route"
        } else {
            ""
        };
        lines.push(format!(
            "interface:     {} {} ({}{route}) {}",
            iface.name,
            iface.link(),
            iface.state,
            iface.addresses.join(" ")
        ));
    }
    lines
}

/// one operation in, one reply out, both as json; what the web ui backend calls. it runs the
/// operation without asking for confirmation (the front end that sent it already did) and never
/// prompts on the terminal: a sudo password comes in the request, or the reply asks for one.
fn operation(host: &str) -> Result<()> {
    // the operation may carry passwords
    let mut input = zeroize::Zeroizing::new(String::new());
    std::io::stdin().read_to_string(&mut input)?;
    let answered = answer(
        &input,
        |operation, ask| execute(host, operation, ask),
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    )?;
    if !answered {
        std::process::exit(1);
    }
    Ok(())
}

/// answer `input` with exactly one json line on `out`. whatever the server printed while the
/// operation ran goes to `err`, so `out` stays readable by a program. false when the answer is an
/// error.
fn answer(
    input: &str,
    run: impl FnOnce(&Operation, AskPassword) -> Result<Reply>,
    out: &mut impl Write,
    err: &mut impl Write,
) -> Result<bool> {
    use lanlord_core::log::{self, Kind};
    let printed = log::subscribe();
    let response = respond(input, run);
    for line in printed.try_iter() {
        if line.kind != Kind::Debug {
            writeln!(err, "{}", line.text)?;
        }
    }
    writeln!(out, "{}", serde_json::to_string(&response)?)?;
    Ok(!matches!(
        response,
        Response::Error(_) | Response::Denied(_)
    ))
}

/// read `input` as a request (or a bare operation) and answer it with `run`. sudo prompts are
/// answered with the request's password; without one the reply asks for it instead.
fn respond(input: &str, run: impl FnOnce(&Operation, AskPassword) -> Result<Reply>) -> Response {
    let request = match read_request(input) {
        Ok(request) => request,
        Err(e) => return Response::Error(format!("not an operation: {e}")),
    };
    let mut asked = None;
    let result = {
        let mut ask = |prompt: &str| match &request.password {
            Some(password) => Ok(password.expose().to_string()),
            None => {
                asked.get_or_insert_with(|| prompt.to_string());
                bail!("the sudo password is needed")
            }
        };
        run(&request.operation, &mut ask)
    };
    match asked {
        Some(prompt) => Response::Ok(Reply::NeedsPassword { prompt }),
        None => match result {
            // a refusal is its own answer, so a front end can tell it from a failure
            Err(e) if is_refusal(&e) => Response::Denied(format!("{e:#}")),
            other => other.into(),
        },
    }
}

/// whether the error is the caller's roles (or the arguments) not allowing the operation.
fn is_refusal(error: &anyhow::Error) -> bool {
    use lanlord_core::access::{Denied, Refused};
    error.downcast_ref::<Denied>().is_some() || error.downcast_ref::<Refused>().is_some()
}

/// a request, or a bare operation as older callers send it.
fn read_request(input: &str) -> serde_json::Result<Request> {
    serde_json::from_str::<Request>(input).or_else(|_| {
        serde_json::from_str::<Operation>(input).map(|operation| Request {
            operation,
            password: None,
        })
    })
}

/// the gateway side of `Session::bridge`: only protocol goes to stdout, logs go to stderr.
fn bridge(mode: Option<BridgeMode>) -> Result<()> {
    use lanlord_core::bridge::{run_local, serve};
    let version = env!("CARGO_PKG_VERSION");
    let mut out = std::io::stdout().lock();
    if matches!(mode, Some(BridgeMode::Hello)) {
        serve(std::io::empty(), &mut out, version, &mut run_local)
    } else {
        serve(std::io::stdin().lock(), &mut out, version, &mut run_local)
    }
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

#[cfg(unix)]
/// the account that ran sudo, only when sudo's own two variables agree: SUDO_UID is the uid of
/// the account SUDO_USER names. an account allowed to set variables through sudo can set
/// SUDO_USER, but not make the system say a different uid belongs to that name, so a mismatch
/// is refused.
fn sudo_account(
    user: Option<&str>,
    uid: Option<&str>,
    uid_of: impl Fn(&str) -> Option<u32>,
) -> Option<String> {
    let user = user.filter(|name| lanlord_core::access::plain_account(name))?;
    let claimed: u32 = uid?.parse().ok()?;
    (uid_of(user)? == claimed).then(|| user.to_string())
}

/// the uid the system gives the account `name`.
#[cfg(unix)]
fn uid_of_account(name: &str) -> Option<u32> {
    let out = std::process::Command::new("id")
        .args(["-u", "--", name])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().parse().ok())?
}

#[cfg(not(unix))]
fn operate() -> Result<()> {
    bail!("this runs through sudo on the gateway, which is a unix machine")
}

/// what an operation's outcome is told as: a refusal is `Denied`, any other failure is an
/// `Error` with its first line only, never the chain of causes.
#[cfg(unix)]
fn response_of(ran: Result<lanlord_core::operations::Reply>) -> lanlord_core::operations::Response {
    use lanlord_core::operations::Response;
    match ran {
        Ok(reply) => Response::Ok(reply),
        Err(error) => {
            let first = error
                .to_string()
                .lines()
                .next()
                .unwrap_or("failed")
                .to_string();
            let refused = error
                .downcast_ref::<lanlord_core::access::Denied>()
                .is_some()
                || error
                    .downcast_ref::<lanlord_core::access::Refused>()
                    .is_some();
            if refused {
                Response::Denied(first)
            } else {
                Response::Error(first)
            }
        }
    }
}

/// the response to one request line: refused without an account, an error for a line that is
/// not an operation, else whatever `run` makes of the operation for that account.
#[cfg(unix)]
fn answer_request(
    line: &str,
    account: Option<String>,
    run: impl FnOnce(
        &str,
        &lanlord_core::operations::Operation,
    ) -> Result<lanlord_core::operations::Reply>,
) -> lanlord_core::operations::Response {
    use lanlord_core::operations::Response;
    match (account, lanlord_core::operate::read_request(line)) {
        (None, _) => Response::Denied("run this through sudo from your own account".into()),
        (_, Err(_)) => Response::Error("not an operation".into()),
        (Some(account), Ok(operation)) => response_of(run(&account, &operation)),
    }
}

/// the gateway's own door over ssh: run one operation that needs the settings key, as root, for
/// the account that ran sudo. the account comes from sudo's variables checked against each other
/// (see `sudo_account`), never from the request, so a login can only act as itself. the answer
/// is one `{"response": ..}` line.
#[cfg(unix)]
fn operate() -> Result<()> {
    use lanlord_core::access::Caller;
    use lanlord_core::operations::execute_in;
    use lanlord_core::ssh::Session;
    use std::os::unix::fs::MetadataExt;

    if std::fs::metadata("/proc/self")?.uid() != 0 {
        bail!("this runs through sudo on the gateway");
    }
    let mut line = zeroize::Zeroizing::new(String::new());
    std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut line)?;
    let session = Session::local();
    let account = sudo_account(
        std::env::var("SUDO_USER").ok().as_deref(),
        std::env::var("SUDO_UID").ok().as_deref(),
        uid_of_account,
    );
    let response = answer_request(&line, account, |account, operation| {
        lanlord_core::crypto::use_gateway_key(lanlord_core::crypto::GATEWAY_KEY.into())?;
        let mut ask = |_: &str| -> Result<String> { bail!("no sudo prompt: this is root") };
        Caller::of_account(&session, account)
            .and_then(|caller| execute_in(&session, "localhost", &caller, operation, &mut ask))
    });
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({ "response": response }))?
    );
    Ok(())
}

/// hand the old client key to the gateway once, so the settings are sealed to the gateway's own
/// key, and then remove the key here: clients hold no keys, and no copy is kept.
fn move_secrets(host: &str) -> Result<()> {
    use lanlord_core::operations::execute;
    let Some(path) = paths::identity_path().filter(|path| path.exists()) else {
        bail!("there is no old key on this machine; nothing to move");
    };
    move_key(&path, &mut |operation| execute(host, operation, &mut ask))
}

/// send the key at `path` to the gateway through `run`, and erase it once the gateway confirms.
fn move_key(
    path: &std::path::Path,
    run: &mut dyn FnMut(
        &lanlord_core::operations::Operation,
    ) -> Result<lanlord_core::operations::Reply>,
) -> Result<()> {
    use lanlord_core::operations::{Operation, Reply};
    let key = std::fs::read_to_string(path)?;
    run(&Operation::MoveSecrets { key: key.into() })?;
    // the gateway says it re-sealed; only a read that decrypts with its own key is believed
    match run(&Operation::LoadDocument)? {
        Reply::Document(document) if !document.needs_move => {}
        _ => bail!(
            "the gateway did not confirm the move; the old key is kept at {}",
            path.display()
        ),
    }
    erase(path)?;
    println!("the settings are sealed to the gateway's key now");
    println!(
        "the old key at {} is removed; no copy is kept",
        path.display()
    );
    Ok(())
}

/// overwrite a file with zeros, sync, and unlink it. the overwrite is best effort (a copy-on-write
/// disk may keep old blocks); the unlink must work.
fn erase(path: &std::path::Path) -> Result<()> {
    use std::io::Write;
    if let Ok(len) = std::fs::metadata(path).map(|m| m.len()) {
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .and_then(|mut file| {
                file.write_all(&vec![0u8; len as usize])?;
                file.sync_all()
            });
    }
    std::fs::remove_file(path)?;
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
        #[cfg(feature = "webui")]
        Some(Command::Webui {
            socket,
            listen,
            static_dir,
            operations_socket,
        }) => webui::run(webui::Options {
            socket,
            listen,
            static_dir,
            operations_socket,
        }),
        #[cfg(feature = "webui")]
        Some(Command::OperationsService {
            socket,
            local_socket,
            allowed_uid,
            identity,
        }) => operations_service::run(operations_service::Options {
            socket,
            local_socket,
            allowed_uid,
            identity,
        }),
        #[cfg(not(feature = "webui"))]
        Some(Command::OperationsService { .. }) => {
            bail!("this build has no operations service; build lanlord with --features webui")
        }
        #[cfg(not(feature = "webui"))]
        Some(Command::Webui { .. }) => {
            bail!("this build has no web server; build lanlord with --features webui")
        }
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
            let apply = lanlord_core::operations::Operation::Apply { modules };
            lanlord_core::operations::execute(&host, &apply, &mut ask).map(|_| ())
        }
        Some(Command::Users { host, action }) => users(&host, action),
        Some(Command::Machines { host, action }) => machines(&host, action),
        Some(Command::Networks { host, action }) => networks(&host, action),
        Some(Command::Logon { host, action }) => logon(&host, action),
        Some(Command::Clock { host, action }) => clock(&host, action),
        Some(Command::Purge {
            host,
            remove_admins,
            confirm,
        }) => purge(&host, remove_admins, confirm),
        Some(Command::Install { host, web }) => {
            lanlord_core::install::run(&host, &web.source()?, &mut ask, &mut print_install)
        }
        Some(Command::Upgrade {
            host,
            web,
            yes,
            even_if_down,
        }) => upgrade(&host, web.source()?, yes, even_if_down),
        Some(Command::Reinstall { host, confirm, web }) => lanlord_core::install::reinstall(
            &host,
            &confirm,
            &web.source()?,
            &mut ask,
            &mut print_install,
        ),
        Some(Command::Leases { host }) => leases(&host),
        Some(Command::Power {
            host,
            action,
            confirm,
        }) => power(&host, action, confirm),
        Some(Command::Facts { host }) => facts(&host),
        Some(Command::Doctor { host, json }) => doctor(&host, json),
        Some(Command::Operation { host }) => operation(&host),
        Some(Command::Bridge { mode }) => bridge(mode),
        Some(Command::Software { host, state }) => {
            let on = matches!(state, Switch::On);
            let modules = ["nt4-domain".to_string()];
            lanlord_core::remote::edit(
                &host,
                &modules,
                |plain| lanlord_core::nt4::software_edit(plain, on),
                &mut ask,
            )
        }
        Some(Command::Admin { host, action }) => admin(&host, action),
        Some(Command::Config(ConfigCommand::Push { host, file })) => {
            lanlord_core::remote::push_file(&host, &std::fs::read(&file)?, &mut ask)
        }
        Some(Command::Config(ConfigCommand::Show { host })) => {
            let (plain, secrets) = lanlord_core::remote::show(&host, &mut ask)?;
            print!("{plain}");
            if !secrets.is_empty() {
                println!("# secrets set (their values are kept on the gateway): ");
                for path in secrets {
                    println!("#   {path}");
                }
            }
            Ok(())
        }
        Some(Command::MoveSecrets { host }) => move_secrets(&host),
        Some(Command::Operate) => operate(),
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
        Some(Command::InstallLauncher { dry_run, force }) => launcher::install(dry_run, force),
        Some(Command::UninstallLauncher { dry_run }) => launcher::uninstall(dry_run),
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn an_outcome_is_told_as_ok_denied_or_an_error_of_one_line() {
        use lanlord_core::operations::{Reply, Response};
        let refused: anyhow::Error = lanlord_core::access::Refused("no".into()).into();
        assert!(
            matches!(super::response_of(Err(refused)), Response::Denied(w) if w.contains("no"))
        );
        let failed = anyhow::anyhow!("first\nsecond");
        assert!(matches!(super::response_of(Err(failed)), Response::Error(w) if w == "first"));
        assert!(matches!(
            super::response_of(Ok(Reply::Done)),
            Response::Ok(Reply::Done)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn the_system_gives_root_uid_zero_and_nobody_else_none() {
        assert_eq!(super::uid_of_account("root"), Some(0));
        assert_eq!(super::uid_of_account("no-such-account-here"), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_request_is_refused_without_an_account_and_garbled_ones_are_errors() {
        use lanlord_core::operations::{Reply, Response};
        let line = "{\"op\":{\"op\":\"load_document\"}}";
        let run = |_: &str, _: &lanlord_core::operations::Operation| Ok(Reply::Done);
        assert!(matches!(
            super::answer_request(line, None, run),
            Response::Denied(_)
        ));
        assert!(matches!(
            super::answer_request("nonsense", Some("ann".into()), run),
            Response::Error(_)
        ));
        let seen = |account: &str, _: &lanlord_core::operations::Operation| {
            assert_eq!(account, "ann");
            Ok(Reply::Done)
        };
        assert!(matches!(
            super::answer_request(line, Some("ann".into()), seen),
            Response::Ok(Reply::Done)
        ));
    }

    #[test]
    fn a_key_is_erased_only_after_the_gateway_confirms_the_move() {
        use lanlord_core::operations::{Operation, Reply};
        let dir = std::env::temp_dir().join(format!("lanlord-move-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("identity");
        std::fs::write(&file, "AGE-SECRET-KEY-1TEST").unwrap();
        // the gateway still says the settings need a move: the key stays
        let mut unconfirmed = |operation: &Operation| {
            Ok(match operation {
                Operation::LoadDocument => Reply::Document(lanlord_core::settings::Document {
                    needs_move: true,
                    ..Default::default()
                }),
                _ => Reply::Done,
            })
        };
        assert!(super::move_key(&file, &mut unconfirmed).is_err());
        assert!(file.exists());
        let mut sent = Vec::new();
        let mut confirmed = |operation: &Operation| {
            sent.push(matches!(operation, Operation::MoveSecrets { .. }));
            Ok(match operation {
                Operation::LoadDocument => Reply::Document(Default::default()),
                _ => Reply::Done,
            })
        };
        super::move_key(&file, &mut confirmed).unwrap();
        assert_eq!(sent, [true, false]);
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn sudo_names_an_account_only_when_its_uid_agrees() {
        let uids = |name: &str| (name == "ann").then_some(1000);
        let ask = |user, uid| super::sudo_account(user, uid, uids);
        assert_eq!(ask(Some("ann"), Some("1000")).as_deref(), Some("ann"));
        // set through SETENV: the name of someone else under the caller's own uid
        assert_eq!(ask(Some("ann"), Some("1001")), None);
        assert_eq!(ask(Some("ann"), None), None);
        assert_eq!(ask(Some("ann"), Some("x")), None);
        assert_eq!(ask(None, Some("1000")), None);
        assert_eq!(ask(Some("nobody-here"), Some("1000")), None);
        assert_eq!(ask(Some("../x"), Some("1000")), None);
    }

    #[test]
    fn an_old_key_is_erased_and_nothing_is_left() {
        let dir = std::env::temp_dir().join(format!("lanlord-erase-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("identity");
        std::fs::write(&file, "AGE-SECRET-KEY-1TEST").unwrap();
        super::erase(&file).unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        assert!(super::erase(&file).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

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
            networks_action(&["status", "--json"]),
            NetworksCommand::Status { json: true }
        ));
    }

    #[test]
    fn doctor_takes_a_host_and_check_is_its_old_name() {
        assert!(
            matches!(parse(&["doctor", "gw"]), Command::Doctor { host, json: false } if host == "gw")
        );
        assert!(matches!(
            parse(&["doctor", "gw", "--json"]),
            Command::Doctor { json: true, .. }
        ));
        assert!(matches!(parse(&["check", "gw"]), Command::Doctor { host, .. } if host == "gw"));
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

    fn lease(expires: u64, name: &str) -> lanlord_core::leases::Lease {
        lanlord_core::leases::Lease {
            expires,
            mac: "00:00:5e:00:53:01".into(),
            address: "198.51.100.20".into(),
            name: name.into(),
        }
    }

    #[test]
    fn leases_show_address_mac_name_and_expiry() {
        let lines = lease_lines(&[lease(0, ""), lease(86_400, "pc01")]);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("198.51.100.20"), "{}", lines[0]);
        assert!(lines[0].contains("00:00:5e:00:53:01") && lines[0].contains(" - "));
        assert!(lines[0].ends_with("expires never"));
        assert!(lines[1].contains("pc01"));
        assert!(
            lines[1].ends_with("expires 1970-01-02 00:00:00 UTC"),
            "{}",
            lines[1]
        );
        assert!(lease_lines(&[]).is_empty());
    }

    #[test]
    fn power_says_what_happened() {
        use lanlord_core::power::{Outcome, Power};
        let done = power_said("gw", Power::Reboot, &Reply::Outcome(Outcome::Done)).unwrap();
        assert_eq!(done, "gw is rebooting");
        let dropped = power_said("gw", Power::PowerOff, &Reply::Outcome(Outcome::Dropped)).unwrap();
        assert!(dropped.contains("dropped the connection") && dropped.contains("shutting down"));
        assert!(power_said("gw", Power::Reboot, &Reply::Done).is_err());
    }

    #[test]
    fn the_doctor_report_shows_each_check_with_its_fix_under_it() {
        let report = lanlord_core::doctor::Report::parse(
            "kernel=6.1.0\nhostname=gw\ndistro=debian\ndistro_version=13\ninit=systemd\nvirt_container=lxc\nvirt_vm=none\nuid_map=0 100000 65536\npve_marker=1\nlink_bridge=ok\ntun=missing\n",
        );
        let lines = doctor_lines(&report);
        assert_eq!(lines[0], "machine:  gw");
        assert_eq!(lines[1], "runs on:  unprivileged lxc container on proxmox");
        assert_eq!(lines[2], "system:   debian 13, kernel 6.1.0, init systemd");
        assert!(
            lines.iter().any(|l| l.starts_with("[+] bridges")),
            "{lines:?}"
        );
        let tun = lines
            .iter()
            .position(|l| l.starts_with("[!] tun/tap device"))
            .unwrap();
        assert!(lines[tun + 1]
            .trim_start()
            .starts_with("fix: on the proxmox host run `pct set <CTID> --dev0 /dev/net/tun`"));
        assert_eq!(or_dash(""), "-");
    }

    #[test]
    fn facts_are_labelled_lines() {
        use lanlord_core::server::{Iface, ServerFacts};
        let mut facts = ServerFacts {
            hostname: "gateway".into(),
            os: "Example Linux".into(),
            kernel: "6.1.0".into(),
            uptime_secs: 90_000,
            user: "admin".into(),
            is_root: true,
            cpu_model: "Example CPU @ 2.00GHz".into(),
            cpus: 2,
            cpus_limited: false,
            mem_total_mb: 2_048,
            mem_available_mb: 1_024,
            mem_limited: false,
            lanlord_installed: true,
            lanlord_version: Some("0.1.0".into()),
            last_applied: Some("2026-01-01 12:00".into()),
            interfaces: vec![
                Iface {
                    name: "eth0".into(),
                    mtu: 1_500,
                    state: "UP".into(),
                    addresses: vec!["192.0.2.10/24".into()],
                    default_route: true,
                },
                Iface {
                    name: "eth1".into(),
                    mtu: 1_500,
                    state: "DOWN".into(),
                    addresses: vec![],
                    default_route: false,
                },
            ],
        };
        let text = fact_lines(&facts).join("\n");
        for want in [
            "name:          gateway",
            "signed in as:  admin (root)",
            "up for:        1 day, 1 hour, 0 minutes",
            "processor:     Example CPU @ 2.00GHz",
            "processors:    2",
            "memory:        1024 MB free of 2048 MB",
            "lanlord:       installed, version 0.1.0",
            "last applied:  2026-01-01 12:00",
            "interface:     eth0 Connected (UP, default route) 192.0.2.10/24",
            "interface:     eth1 Disconnected (DOWN) ",
        ] {
            assert!(text.contains(want), "{want:?} in\n{text}");
        }
        facts.cpu_model.clear();
        assert!(fact_lines(&facts)
            .join("\n")
            .contains("processor:     unknown"));
        facts.is_root = false;
        facts.lanlord_version = None;
        facts.last_applied = None;
        let text = fact_lines(&facts).join("\n");
        assert!(text.contains("lanlord:       installed\n") && !text.contains("(root)"));
        assert!(!text.contains("last applied"));
        facts.lanlord_installed = false;
        assert!(fact_lines(&facts).join("\n").contains("not installed"));
    }

    #[test]
    fn operation_answers_json_with_a_reply_or_an_error() {
        let ok = respond(r#"{"op":"leases"}"#, |operation, _| {
            assert_eq!(operation, &Operation::Leases);
            Ok(Reply::Leases(vec![]))
        });
        assert_eq!(ok, Response::Ok(Reply::Leases(vec![])));
        let failed = respond(r#"{"op":"server_facts"}"#, |_, _| bail!("no route to host"));
        assert_eq!(failed, Response::Error("no route to host".into()));
        let refused = respond(r#"{"op":"server_facts"}"#, |_, _| {
            Err(lanlord_core::access::Denied {
                area: lanlord_core::access::Area::Status,
                level: lanlord_core::access::Level::View,
            }
            .into())
        });
        assert!(matches!(refused, Response::Denied(m) if m.contains("permission denied")));
        let garbage = respond("not json", |_, _| unreachable!("never runs"));
        assert!(matches!(garbage, Response::Error(e) if e.starts_with("not an operation")));
        let json = serde_json::to_string(&ok).unwrap();
        assert!(json.starts_with(r#"{"ok":"#), "{json}");
    }

    #[test]
    fn a_request_answers_sudo_with_its_password() {
        let input = r#"{"op":{"op":"leases"},"password":"s3cret"}"#;
        let response = respond(input, |operation, ask| {
            assert_eq!(operation, &Operation::Leases);
            assert_eq!(ask("[sudo] password for admin:")?, "s3cret");
            assert_eq!(ask("[sudo] password for admin:")?, "s3cret", "asked again");
            Ok(Reply::Leases(vec![]))
        });
        assert_eq!(response, Response::Ok(Reply::Leases(vec![])));
    }

    #[test]
    fn without_a_password_the_reply_asks_for_one() {
        for input in [
            r#"{"op":{"op":"leases"}}"#,
            r#"{"op":{"op":"leases"},"password":null}"#,
            r#"{"op":"leases"}"#,
        ] {
            let response = respond(input, |_, ask| {
                // the operation fails once its prompt is refused, whatever it says
                ask("[sudo] password for admin:")?;
                unreachable!("there is no password to give")
            });
            assert_eq!(
                response,
                Response::Ok(Reply::NeedsPassword {
                    prompt: "[sudo] password for admin:".into()
                }),
                "{input}"
            );
        }
        // an operation that never asks runs as before
        let response = respond(r#"{"op":{"op":"probe"}}"#, |_, _| Ok(Reply::Done));
        assert_eq!(response, Response::Ok(Reply::Done));
    }

    #[test]
    fn the_first_prompt_is_the_one_shown() {
        let response = respond(r#"{"op":{"op":"leases"}}"#, |_, ask| {
            let _ = ask("first:");
            let _ = ask("second:");
            bail!("refused")
        });
        assert_eq!(
            response,
            Response::Ok(Reply::NeedsPassword {
                prompt: "first:".into()
            })
        );
    }

    #[test]
    fn operation_writes_exactly_one_json_line_whatever_the_server_prints() {
        use lanlord_core::log::{emit, Kind};
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let answered = answer(
            r#"{"op":"run","command":["uptime"]}"#,
            |_, _| {
                emit(Kind::Out, "operation-test: up 3 days");
                emit(Kind::Err, "operation-test: a warning");
                Ok(Reply::ExitCode(0))
            },
            &mut out,
            &mut err,
        )
        .unwrap();
        assert!(answered);
        let out = String::from_utf8(out).unwrap();
        assert_eq!(out.lines().count(), 1, "{out:?}");
        let reply: Response = serde_json::from_str(out.trim_end()).unwrap();
        assert_eq!(reply, Response::Ok(Reply::ExitCode(0)));
        let err = String::from_utf8(err).unwrap();
        assert!(
            err.contains("operation-test: up 3 days") && err.contains("operation-test: a warning")
        );

        let mut out = Vec::new();
        let answered = answer("{}", |_, _| unreachable!(), &mut out, &mut Vec::new()).unwrap();
        assert!(!answered);
        assert!(String::from_utf8(out).unwrap().starts_with(r#"{"error":"#));
    }

    #[test]
    fn a_given_confirmation_skips_the_prompt() {
        assert_eq!(confirmed("never shown", Some("gw".into())).unwrap(), "gw");
    }

    #[test]
    fn operation_is_hidden_from_help() {
        assert!(matches!(parse(&["op", "gw"]), Command::Operation { host } if host == "gw"));
        assert!(matches!(parse(&["bridge"]), Command::Bridge { mode: None }));
        assert!(matches!(
            parse(&["bridge", "serve"]),
            Command::Bridge {
                mode: Some(BridgeMode::Serve)
            }
        ));
        let help = Cli::command().render_help().to_string();
        assert!(help.contains("leases") && help.contains("facts") && help.contains("power"));
        assert!(
            !help.lines().any(|l| l.trim_start().starts_with("op ")),
            "{help}"
        );
    }

    fn networks_action(args: &[&str]) -> NetworksCommand {
        let mut argv = vec!["lanlord", "networks", "gw"];
        argv.extend_from_slice(args);
        match Cli::try_parse_from(argv).unwrap().command {
            Some(Command::Networks { host, action }) => {
                assert_eq!(host, "gw");
                action
            }
            _ => panic!("a networks command"),
        }
    }

    #[test]
    fn the_networks_commands_parse() {
        assert!(matches!(networks_action(&["list"]), NetworksCommand::List));
        let NetworksCommand::Add {
            name,
            interface,
            mtu,
            dhcp_range,
            lease,
            zone,
            ..
        } = networks_action(&[
            "add",
            "lab",
            "--interface",
            "eth2",
            "--address",
            "192.168.50.1/24",
            "--mtu",
            "1400",
            "--dhcp-range",
            "192.168.50.100-192.168.50.200",
            "--lease",
            "12h",
            "--zone",
            "lab",
        ])
        else {
            panic!("an add")
        };
        assert_eq!(
            (name.as_str(), interface.as_str(), mtu),
            ("lab", "eth2", Some(1400))
        );
        assert_eq!(
            (dhcp_range.as_deref(), lease.as_deref(), zone.as_deref()),
            (
                Some("192.168.50.100-192.168.50.200"),
                Some("12h"),
                Some("lab")
            )
        );
        let NetworksCommand::Set {
            no_mtu, no_dhcp, ..
        } = networks_action(&["set", "lab", "--no-mtu", "--no-dhcp"])
        else {
            panic!("a set")
        };
        assert!(no_mtu && no_dhcp);
        assert!(matches!(
            networks_action(&["remove", "lab"]),
            NetworksCommand::Remove { .. }
        ));
        let NetworksCommand::Lease {
            name,
            action: LeaseCommand::Add { host, mac, address },
        } = networks_action(&[
            "lease",
            "lab",
            "add",
            "--host",
            "pc",
            "--mac",
            "02:00:00:00:00:01",
            "--address",
            "192.168.50.20",
        ])
        else {
            panic!("a lease add")
        };
        assert_eq!(
            (name.as_str(), host.as_str(), mac.as_str(), address.as_str()),
            ("lab", "pc", "02:00:00:00:00:01", "192.168.50.20")
        );
        assert!(matches!(
            networks_action(&["lease", "lab", "remove", "--host", "pc"]),
            NetworksCommand::Lease {
                action: LeaseCommand::Remove { .. },
                ..
            }
        ));
        assert!(Cli::try_parse_from(["lanlord", "networks", "gw", "add", "x"]).is_err());
    }

    #[test]
    fn the_web_server_has_its_own_runtime_folder() {
        // /run/lanlord holds the decrypted settings during an apply; the web server's socket
        // folder is mounted into the proxy, so it must be a different one
        let Some(Command::Webui { socket, .. }) =
            Cli::try_parse_from(["lanlord", "webui"]).unwrap().command
        else {
            panic!("webui parses");
        };
        assert_eq!(socket, PathBuf::from("/run/lanlord-web/web.sock"));
        assert!(!socket.starts_with("/run/lanlord/"));
    }

    #[test]
    fn upgrade_takes_the_web_files_and_a_prompt_skip() {
        let Some(Command::Upgrade {
            host,
            web,
            yes,
            even_if_down,
        }) = Cli::try_parse_from([
            "lanlord",
            "upgrade",
            "gw",
            "--server-binary",
            "lanlord",
            "--auth-helper",
            "helper",
            "--web-dist",
            "dist",
            "--yes",
        ])
        .unwrap()
        .command
        else {
            panic!("upgrade parses");
        };
        assert_eq!(host, "gw");
        assert!(yes);
        // --yes only skips the prompt
        assert!(!even_if_down);
        assert_eq!(web.server_binary, Some(PathBuf::from("lanlord")));
        let Some(Command::Upgrade {
            yes, even_if_down, ..
        }) = Cli::try_parse_from(["lanlord", "upgrade", "gw", "--even-if-down"])
            .unwrap()
            .command
        else {
            panic!("upgrade parses");
        };
        assert!(!yes);
        assert!(even_if_down);
        assert!(Cli::try_parse_from(["lanlord", "upgrade"]).is_err());
    }
}
