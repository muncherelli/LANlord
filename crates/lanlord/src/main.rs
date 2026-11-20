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
        /// give the whole network internet access, or take it away (none by default); give it
        /// on its own
        #[arg(long, value_enum)]
        internet: Option<Switch>,
    },
    /// remove a network and its dnsmasq
    Remove { name: String },
    /// add or remove a static dhcp lease on a network
    Lease {
        name: String,
        #[command(subcommand)]
        action: LeaseCommand,
    },
    /// let single computers on a network go out to the internet, for good or for a while
    Internet {
        name: String,
        #[command(subcommand)]
        action: InternetCommand,
    },
    /// who may use ssh to the server from a network: nobody by default, a whole network, or
    /// single computers, for good or for a while
    Ssh {
        name: String,
        #[command(subcommand)]
        action: SshCommand,
    },
}

#[derive(Subcommand)]
enum SshCommand {
    /// let every computer on the network use ssh to the server
    On,
    /// keep ssh to the server off for the network (except the computers allowed by mac)
    Off,
    /// let a computer (by mac) use ssh to the server; without --for or --until, for good
    Allow {
        mac: String,
        /// for how long, like 30m, 1h, 1d or 1d12h
        #[arg(long = "for", value_name = "DURATION", conflicts_with = "until")]
        duration: Option<String>,
        /// until a time in your own time zone, like '2030-01-31 09:00'
        #[arg(long, value_name = "YYYY-MM-DD HH:MM")]
        until: Option<String>,
        /// a label for the computer
        #[arg(long, value_name = "LABEL")]
        name: Option<String>,
    },
    /// take a computer's ssh access away
    Deny { mac: String },
    /// show whether the network may use ssh to the server and the computers allowed
    List,
}

#[derive(Subcommand)]
enum InternetCommand {
    /// let a computer (by mac) go out; without --for or --until, for good
    Allow {
        mac: String,
        /// for how long, like 30m, 1h, 1d or 1d12h
        #[arg(long = "for", value_name = "DURATION", conflicts_with = "until")]
        duration: Option<String>,
        /// until a time in your own time zone, like '2030-01-31 09:00'
        #[arg(long, value_name = "YYYY-MM-DD HH:MM")]
        until: Option<String>,
        /// a label for the computer
        #[arg(long, value_name = "LABEL")]
        name: Option<String>,
    },
    /// take a computer's internet access away
    Deny { mac: String },
    /// show the network's internet access and the computers allowed out
    List,
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

/// who becomes the LANlord administrator at the end of an install that finds none.
#[derive(clap::Args, Default)]
struct FirstAdmin {
    /// make this existing account the administrator; it must already be able to become root
    /// with sudo
    #[arg(long, value_name = "USER", conflicts_with_all = ["create_admin", "no_admin"])]
    admin: Option<String>,
    /// create this account as the administrator (a login, in admins and sudo); its password is
    /// asked for, or read from standard input with --admin-password-stdin
    #[arg(long, value_name = "NAME", conflicts_with = "no_admin")]
    create_admin: Option<String>,
    /// read the new administrator's password from one line of standard input
    #[arg(long, requires = "create_admin")]
    admin_password_stdin: bool,
    /// a public key file to authorize for the new administrator
    #[arg(long, value_name = "FILE", requires = "create_admin")]
    admin_ssh_key: Option<String>,
    /// set no administrator up: root signs in to the web UI for the first-run setup
    #[arg(long)]
    no_admin: bool,
}

/// the choice the flags make, or `None` when they make none. the password comes from
/// `password` only when an account is created.
fn flag_choice(
    flags: &FirstAdmin,
    password: impl FnOnce() -> Result<String>,
) -> Result<Option<lanlord_core::firstadmin::Choice>> {
    use lanlord_core::firstadmin::Choice;
    if flags.no_admin {
        return Ok(Some(Choice::Skip));
    }
    if let Some(user) = &flags.admin {
        return Ok(Some(Choice::Adopt(user.clone())));
    }
    let Some(name) = &flags.create_admin else {
        return Ok(None);
    };
    let ssh_public_key = flags
        .admin_ssh_key
        .as_deref()
        .map(lanlord_core::account::read_key)
        .transpose()?;
    Ok(Some(Choice::Create {
        name: name.clone(),
        password: zeroize::Zeroizing::new(password()?),
        ssh_public_key,
    }))
}

/// one line typed in answer to `prompt`, empty when there is none.
fn ask_line(prompt: &str) -> Result<String> {
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

/// the public keys in `~/.ssh`, for offering one to a new account.
fn own_public_keys() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let mut keys: Vec<PathBuf> = std::fs::read_dir(PathBuf::from(home).join(".ssh"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "pub"))
        .collect();
    keys.sort();
    keys
}

/// ask for a new administrator's name, password and (optionally) ssh key at the terminal.
fn ask_new_admin() -> Result<lanlord_core::firstadmin::Choice> {
    ask_new_admin_with(
        &mut ask_line,
        &mut |name| new_password(name),
        &own_public_keys(),
    )
}

/// `ask_new_admin` with the questions and the keys it offers given, so they can be tried
/// without a terminal.
fn ask_new_admin_with(
    ask: &mut dyn FnMut(&str) -> Result<String>,
    password: &mut dyn FnMut(&str) -> Result<String>,
    keys: &[PathBuf],
) -> Result<lanlord_core::firstadmin::Choice> {
    let name = ask("name for the new administrator: ")?;
    if name.is_empty() {
        bail!("no name given");
    }
    let password = password(&name)?;
    let mut ssh_public_key = None;
    if !keys.is_empty() {
        eprintln!("authorize one of your ssh public keys for {name}?");
        for (i, key) in keys.iter().enumerate() {
            eprintln!("  {}) {}", i + 1, key.display());
        }
        let answer = ask("number, or enter for none: ")?;
        if let Some(key) = answer
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|i| keys.get(i))
        {
            ssh_public_key = Some(lanlord_core::account::read_key(&key.to_string_lossy())?);
        }
    }
    Ok(lanlord_core::firstadmin::Choice::Create {
        name,
        password: zeroize::Zeroizing::new(password),
        ssh_public_key,
    })
}

/// the first administrator: from the flags, else by asking at a terminal, else none (the
/// next steps then point at the web UI's first-run setup).
fn decide_admin(
    flags: &FirstAdmin,
    login: &lanlord_core::account::LoginUser,
) -> Result<lanlord_core::firstadmin::Choice> {
    use lanlord_core::firstadmin::Choice;
    let from_stdin = || {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        Ok(line.trim_end_matches(['\n', '\r']).to_string())
    };
    let password = || {
        if flags.admin_password_stdin {
            from_stdin()
        } else {
            new_password(flags.create_admin.as_deref().unwrap_or("the new account"))
        }
    };
    if let Some(choice) = flag_choice(flags, password)? {
        return Ok(choice);
    }
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        return Ok(Choice::Skip);
    }
    println!();
    println!("this server has no LANlord administrator yet.");
    ask_choice(login, &mut ask_line, &mut ask_new_admin)
}

/// the questions of the first administrator, answered through `ask`; `new_admin` collects the
/// details of an account to create.
fn ask_choice(
    login: &lanlord_core::account::LoginUser,
    ask: &mut dyn FnMut(&str) -> Result<String>,
    new_admin: &mut dyn FnMut() -> Result<lanlord_core::firstadmin::Choice>,
) -> Result<lanlord_core::firstadmin::Choice> {
    use lanlord_core::firstadmin::Choice;
    if login.is_root {
        let answer = ask(
            "create a new administrator account? [Y] yes, [s] skip and sign in to the web UI as root: ",
        )?;
        return match answer.to_lowercase().as_str() {
            "" | "y" | "yes" => new_admin(),
            _ => Ok(Choice::Skip),
        };
    }
    let answer = ask(&format!(
        "make {0} your LANlord administrator? [Y] yes, [c] create a new account, [s] skip: ",
        login.name
    ))?;
    match answer.to_lowercase().as_str() {
        "" | "y" | "yes" => Ok(Choice::Adopt(login.name.clone())),
        "c" | "create" => new_admin(),
        _ => Ok(Choice::Skip),
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
        #[command(flatten)]
        admin: FirstAdmin,
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
        #[command(flatten)]
        admin: FirstAdmin,
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
    /// take the internet grants that have ended out of the settings (run by a timer on the
    /// gateway, as root)
    #[command(name = "internet-prune", hide = true)]
    InternetPrune {
        /// write the rules again even when nothing ended (after a boot)
        #[arg(long)]
        refresh: bool,
    },
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
    if let Some(done) = set_internet(host, &action) {
        return done;
    }
    match action {
        NetworksCommand::Status { json } => network_status(host, json),
        NetworksCommand::Internet { name, action } => internet(host, &name, action),
        NetworksCommand::Ssh { name, action } => ssh_access(host, &name, action),
        edit => change_networks(host, edit),
    }
}

/// switch the internet for a whole network. `set --internet` goes through its own operation, so
/// it is not mixed with the other changes of `set`, which are stored by another route.
fn set_internet(host: &str, set: &NetworksCommand) -> Option<Result<()>> {
    match set_internet_change(set)? {
        Ok(change) => Some(change_internet(host, change)),
        Err(error) => Some(Err(error)),
    }
}

/// the change `set --internet` stands for: none when `set` has no `--internet`.
fn set_internet_change(set: &NetworksCommand) -> Option<Result<lanlord_model::internet::Change>> {
    let NetworksCommand::Set {
        name,
        interface,
        address,
        mtu,
        no_mtu,
        zone,
        dhcp_range,
        lease,
        no_dhcp,
        internet: Some(state),
    } = set
    else {
        return None;
    };
    let other = interface.is_some()
        || address.is_some()
        || mtu.is_some()
        || *no_mtu
        || zone.is_some()
        || dhcp_range.is_some()
        || lease.is_some()
        || *no_dhcp;
    if other {
        return Some(Err(anyhow::anyhow!(
            "give --internet on its own, not with the other changes"
        )));
    }
    Some(Ok(lanlord_model::internet::Change::Network {
        network: name.clone(),
        on: matches!(state, Switch::On),
    }))
}

/// the change `internet NAME allow MAC ...` stands for, with the end worked out from `now`.
fn allow_change(
    network: &str,
    allow: InternetCommand,
    now: i64,
) -> Result<lanlord_model::internet::Change> {
    use lanlord_model::internet::{parse_duration, parse_until_local, Change, Expiry};
    match allow {
        InternetCommand::Deny { mac } => Ok(Change::Deny {
            network: network.to_string(),
            mac,
        }),
        InternetCommand::Allow {
            mac,
            duration,
            until,
            name,
        } => {
            let expiry = match (duration, until) {
                (Some(text), _) => Expiry::At(now + parse_duration(&text)?),
                (None, Some(text)) => Expiry::At(parse_until_local(&text)?),
                (None, None) => Expiry::Never,
            };
            Ok(Change::Allow {
                network: network.to_string(),
                mac,
                name,
                expiry,
            })
        }
        InternetCommand::List => bail!("listing changes nothing"),
    }
}

/// send a change of the internet or ssh grants and say it was applied.
fn send_grant(host: &str, text: String, operation: Operation) -> Result<()> {
    match execute(host, &operation, &mut ask)? {
        Reply::Done => {
            println!("{text}; applied the docker and dns modules");
            Ok(())
        }
        _ => bail!("unexpected answer"),
    }
}

fn change_internet(host: &str, change: lanlord_model::internet::Change) -> Result<()> {
    send_grant(
        host,
        change.describe(),
        Operation::ChangeInternet { change },
    )
}

/// print the lines `details` makes of the network called `name`.
fn print_details(
    host: &str,
    name: &str,
    details: fn(&lanlord_model::lan_networks::LanNetwork) -> Vec<(String, String)>,
) -> Result<()> {
    let document = lanlord_core::remote::document(host, &mut ask)?;
    let networks = lanlord_core::lan::read_networks(&document.plain)?;
    let Some(network) = lanlord_model::lan_networks::find(&networks.list, name) else {
        bail!("there is no network called {name}");
    };
    for (label, value) in details(network) {
        println!("{label}: {value}");
    }
    Ok(())
}

/// `lanlord networks HOST internet NAME ...`.
fn internet(host: &str, name: &str, action: InternetCommand) -> Result<()> {
    if matches!(action, InternetCommand::List) {
        return print_details(host, name, |n| n.internet.details());
    }
    change_internet(
        host,
        allow_change(name, action, lanlord_core::internet::now())?,
    )
}

/// the ssh change `ssh NAME on|off|allow|deny ...` stands for, with the end worked out from `now`.
fn ssh_change(
    network: &str,
    action: SshCommand,
    now: i64,
) -> Result<lanlord_model::ssh_access::Change> {
    use lanlord_model::ssh_access::Change;
    // the arguments of allow and deny and their reading are the internet ones'
    let like = match action {
        SshCommand::On | SshCommand::Off => {
            return Ok(Change::Network {
                network: network.to_string(),
                on: matches!(action, SshCommand::On),
            })
        }
        SshCommand::List => InternetCommand::List,
        SshCommand::Deny { mac } => InternetCommand::Deny { mac },
        SshCommand::Allow {
            mac,
            duration,
            until,
            name,
        } => InternetCommand::Allow {
            mac,
            duration,
            until,
            name,
        },
    };
    Ok(allow_change(network, like, now)?.into())
}

/// `lanlord networks HOST ssh NAME ...`.
fn ssh_access(host: &str, name: &str, action: SshCommand) -> Result<()> {
    if matches!(action, SshCommand::List) {
        return print_details(host, name, |n| n.ssh.details());
    }
    let change = ssh_change(name, action, lanlord_core::internet::now())?;
    send_grant(
        host,
        change.describe(),
        Operation::ChangeSshAccess { change },
    )
}

/// the gateway's clean-up of internet grants that have ended, run by its timer as root.
#[cfg(unix)]
fn internet_prune(refresh: bool) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    if std::fs::metadata("/proc/self")?.uid() != 0 {
        bail!("this runs as root on the gateway");
    }
    lanlord_core::crypto::use_gateway_key(lanlord_core::crypto::GATEWAY_KEY.into())?;
    let Some(key) = lanlord_core::crypto::load_default()? else {
        return Ok(());
    };
    let mut ask = |_: &str| -> Result<String> { bail!("no sudo prompt: this is root") };
    let session = lanlord_core::ssh::Session::local();
    let gone = lanlord_core::internet::prune(&session, &key, refresh, &mut ask)?;
    if gone > 0 {
        println!("took {gone} ended internet grant(s) out of the settings");
    }
    Ok(())
}

#[cfg(not(unix))]
fn internet_prune(_refresh: bool) -> Result<()> {
    bail!("this runs on the gateway, which is a unix machine")
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
            internet: _,
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
        NetworksCommand::Internet { .. } | NetworksCommand::Ssh { .. } => {
            unreachable!("run by `networks`")
        }
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

/// an install shows its steps, not every task of the playbooks, unless `--verbose` asks.
fn quiet_install() {
    lanlord_core::log::set_quiet_tasks(!lanlord_core::log::verbose());
}

/// what the server printed in the step that runs: package managers and ansible are loud, so it
/// is shown with --verbose, and otherwise only when the step fails.
#[derive(Default)]
struct StepOutput(Vec<String>);

impl StepOutput {
    /// a new step: what the last one printed is no longer wanted.
    fn start(&mut self) {
        self.0.clear();
    }

    fn push(&mut self, line: String) {
        self.0.push(line);
    }

    /// the lines to show for a failed step: the playbook's own output (`from_log`, held by the
    /// log) first, then the step's lines, the last 40 of them.
    fn failure(&mut self, from_log: Vec<String>) -> Vec<String> {
        let mut held = from_log;
        held.append(&mut self.0);
        let skip = held.len().saturating_sub(40);
        held.split_off(skip)
    }
}

/// print an install's steps and what the server says while it runs.
fn print_install(event: lanlord_core::install::Event) {
    use lanlord_core::install::{Event, Step};
    static HELD: std::sync::Mutex<StepOutput> = std::sync::Mutex::new(StepOutput(Vec::new()));
    let number = |step: Step| Step::ALL.iter().position(|s| *s == step).unwrap_or(0) + 1;
    match event {
        Event::StepStarted(step) => {
            HELD.lock().unwrap().start();
            lanlord_core::log::take_held();
            println!("[{}/{}] {}", number(step), Step::ALL.len(), step.title())
        }
        Event::Line(line) if lanlord_core::log::verbose() => println!("  {line}"),
        Event::Line(line) => HELD.lock().unwrap().push(line),
        Event::NextSteps(lines) => {
            println!();
            for line in lines {
                println!("{line}");
            }
        }
        Event::StepDone(step) => println!("[{}/{}] done", number(step), Step::ALL.len()),
        // the error itself is printed once, by the caller
        Event::Failed { step, .. } => {
            let held = HELD.lock().unwrap().failure(lanlord_core::log::take_held());
            for line in &held {
                eprintln!("  {line}");
            }
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
    // an unreadable login leaves the root rule unapplied; a failed move then names its command
    let root = lanlord_core::ssh::Session::new(host)
        .and_then(|session| lanlord_core::account::login_user(&session))
        .map(|user| user.is_root)
        .unwrap_or(false);
    let key = paths::identity_path().filter(|path| path.exists());
    let target = UpgradeTarget {
        host,
        yes,
        root,
        key: key.as_deref(),
    };
    upgrade_steps(
        &target,
        &mut || match execute(host, &Operation::LoadDocument, &mut ask)? {
            Reply::Document(document) => Ok(document.needs_move),
            _ => bail!("the server answered something other than the settings"),
        },
        &mut |operation| execute(host, operation, &mut ask),
        &mut |question| Ok(confirmed(question, None)?.eq_ignore_ascii_case("yes")),
        &mut || {
            let typed = confirmed(
                "this restarts the lanlord services, and restarting lanlord-operations interrupts any operation that is running (nothing is removed). type the host name to go on: ",
                None,
            )?;
            if typed != host {
                bail!("the host name does not match; nothing was changed");
            }
            Ok(())
        },
        &mut || lanlord_core::upgrade::run(host, &web, even_if_down, &mut ask, &mut print_upgrade),
    )
}

/// what the check on the stored secrets needs to know about the upgrade.
struct UpgradeTarget<'a> {
    host: &'a str,
    yes: bool,
    /// whether the login on the gateway is root's.
    root: bool,
    /// the old client key on this machine, if there is one.
    key: Option<&'a std::path::Path>,
}

/// the upgrade, with the check on the stored secrets first: when they are sealed to a key the
/// gateway does not hold, the upgrade would fail at its last step after changing everything, so
/// it is settled (or refused) before `go` (the first change) is called. `host_name` is the
/// host-name prompt, asked first so that no prompt comes after a change. `needs_move` reads the
/// gateway without writing; `key` is the old client key on this machine, if there is one.
/// with --yes the move is done without asking: the upgrade cannot finish without it, the move
/// only seals the secrets to the gateway's own key, and the old key is erased only after the
/// gateway has confirmed that it reads them.
fn upgrade_steps(
    target: &UpgradeTarget,
    needs_move: &mut dyn FnMut() -> Result<bool>,
    run: &mut dyn FnMut(
        &lanlord_core::operations::Operation,
    ) -> Result<lanlord_core::operations::Reply>,
    agree: &mut dyn FnMut(&str) -> Result<bool>,
    host_name: &mut dyn FnMut() -> Result<()>,
    go: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    let UpgradeTarget {
        host,
        yes,
        root,
        key,
    } = *target;
    let command = lanlord_core::upgrade::move_command(host);
    // every prompt comes before the first change, the move included
    if !yes {
        host_name()?;
    }
    // a gateway that cannot answer the check is not a reason to stop: the upgrade's own first
    // step says what is wrong with it, and its last step names the command if it is this
    let moving = match needs_move() {
        Ok(moving) => moving,
        Err(error) => {
            eprintln!("could not check the stored secrets ({error:#}); going on");
            false
        }
    };
    if moving {
        let Some(path) = key else {
            bail!(
                "the stored secrets on {host} are sealed to a key the gateway does not hold, and \
                 this machine has no old key; nothing was changed. the machine that saved the \
                 settings there before has it: run `{command}` from an administrator's own \
                 account on that machine, then upgrade again"
            );
        };
        if root {
            bail!(
                "the stored secrets on {host} are sealed to a key the gateway does not hold; \
                 moving them cannot be done as root; nothing was changed. run `{command}`, \
                 then upgrade again"
            );
        }
        println!("the stored secrets on {host} are sealed to the old key on this machine");
        if !yes
            && !agree(&format!(
                "move them to the gateway's key first (the old key at {} is erased once the gateway confirms it reads them)? type yes: ",
                path.display()
            ))?
        {
            bail!("nothing was changed; run `{command}` and then upgrade again");
        }
        move_key(path, run).map_err(|error| {
            anyhow::anyhow!(
                "{error:#}; nothing else was changed; run `{command}`, then upgrade again"
            )
        })?;
    }
    go()
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
    ];
    for (label, value) in facts.cpu_rows() {
        lines.push(format!(
            "{:<15}{value}",
            format!("{}:", label.to_lowercase())
        ));
    }
    lines.push(format!(
        "memory:        {} MB free of {} MB{}",
        facts.mem_available_mb,
        facts.mem_total_mb,
        if facts.mem_limited {
            " (limited by container)"
        } else {
            ""
        }
    ));
    lines.push(format!("lanlord:       {lanlord}"));
    if let Some(when) = facts.last_applied_local() {
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

/// who is asking `lanlord operate`: the account sudo names (see `sudo_account`), or root itself
/// when sudo took no part, which is how a root login over ssh arrives. root is always let run
/// operations; sudo variables that are set but disagree are refused, as before.
#[cfg(unix)]
fn operating_account(
    user: Option<&str>,
    uid: Option<&str>,
    uid_of: impl Fn(&str) -> Option<u32>,
) -> Option<String> {
    match (user, uid) {
        (None, None) => Some("root".to_string()),
        // `sudo lanlord operate` run as root: sudo names root, and the system agrees
        (Some("root"), Some(claimed))
            if claimed.parse() == Ok(0u32) && uid_of("root") == Some(0) =>
        {
            Some("root".to_string())
        }
        _ => sudo_account(user, uid, uid_of),
    }
}

/// the caller for `account`: root is an administrator without the group, as it is for a root
/// login anywhere else.
#[cfg(unix)]
fn operating_caller(
    session: &lanlord_core::ssh::Session,
    account: &str,
) -> Result<lanlord_core::access::Caller> {
    if account == "root" {
        return Ok(lanlord_core::access::Caller {
            user: "root".into(),
            admin: true,
        });
    }
    lanlord_core::access::Caller::of_account(session, account)
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
        (None, _) => Response::Denied("could not tell which account ran sudo".into()),
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
    use lanlord_core::operations::execute_in;
    use lanlord_core::ssh::Session;
    use std::os::unix::fs::MetadataExt;

    if std::fs::metadata("/proc/self")?.uid() != 0 {
        bail!("this runs through sudo on the gateway");
    }
    let mut line = zeroize::Zeroizing::new(String::new());
    std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut line)?;
    let session = Session::local();
    let account = operating_account(
        std::env::var("SUDO_USER").ok().as_deref(),
        std::env::var("SUDO_UID").ok().as_deref(),
        uid_of_account,
    );
    let response = answer_request(&line, account, |account, operation| {
        lanlord_core::crypto::use_gateway_key(lanlord_core::crypto::GATEWAY_KEY.into())?;
        let mut ask = |_: &str| -> Result<String> { bail!("no sudo prompt: this is root") };
        // the journal names the actor; best effort, the operation does not wait on it
        let _ = std::process::Command::new("logger")
            .args(["-t", "lanlord", &format!("operation run as {account}")])
            .status();
        operating_caller(&session, account)
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
            lanlord_core::operations::execute(&host, &apply, &mut ask)?;
            // a last line of its own: the progress above goes to the log, which a terminal
            // session may not show
            println!("applied to {host}");
            Ok(())
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
        Some(Command::Install { host, web, admin }) => {
            quiet_install();
            lanlord_core::install::run(
                &host,
                &web.source()?,
                &mut ask,
                &mut |login| decide_admin(&admin, login),
                &mut print_install,
            )
        }
        Some(Command::Upgrade {
            host,
            web,
            yes,
            even_if_down,
        }) => upgrade(&host, web.source()?, yes, even_if_down),
        Some(Command::Reinstall {
            host,
            confirm,
            web,
            admin,
        }) => {
            quiet_install();
            lanlord_core::install::reinstall(
                &host,
                &confirm,
                &web.source()?,
                &mut ask,
                &mut |login| decide_admin(&admin, login),
                &mut print_install,
            )
        }
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
            lanlord_core::remote::push_file(&host, &std::fs::read(&file)?, &mut ask)?;
            println!("settings stored on {host}");
            Ok(())
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
        Some(Command::InternetPrune { refresh }) => internet_prune(refresh),
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

    #[test]
    fn the_flags_pick_the_first_administrator() {
        use lanlord_core::firstadmin::Choice;
        let none = || -> anyhow::Result<String> { panic!("no password needed") };
        let flags = super::FirstAdmin::default();
        assert_eq!(super::flag_choice(&flags, none).unwrap(), None);
        let flags = super::FirstAdmin {
            no_admin: true,
            ..Default::default()
        };
        assert_eq!(
            super::flag_choice(&flags, none).unwrap(),
            Some(Choice::Skip)
        );
        let flags = super::FirstAdmin {
            admin: Some("olive".into()),
            ..Default::default()
        };
        assert_eq!(
            super::flag_choice(&flags, none).unwrap(),
            Some(Choice::Adopt("olive".into()))
        );
        let flags = super::FirstAdmin {
            create_admin: Some("olive".into()),
            ..Default::default()
        };
        assert_eq!(
            super::flag_choice(&flags, || Ok(["fixture", "2"].join("-"))).unwrap(),
            Some(Choice::Create {
                name: "olive".into(),
                password: zeroize::Zeroizing::new(["fixture", "2"].join("-")),
                ssh_public_key: None
            })
        );
    }

    #[test]
    fn without_flags_or_a_terminal_the_first_administrator_is_skipped() {
        use std::io::IsTerminal;
        // a terminal would be asked; the check is for scripts and ci
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            return;
        }
        let login = lanlord_core::account::LoginUser {
            name: "root".into(),
            is_root: true,
        };
        let choice = super::decide_admin(&super::FirstAdmin::default(), &login).unwrap();
        assert_eq!(choice, lanlord_core::firstadmin::Choice::Skip);
    }

    #[test]
    fn the_admin_flags_exclude_each_other() {
        use clap::Parser;
        let parse = |args: &[&str]| {
            let mut argv = vec!["lanlord", "install", "gw"];
            argv.extend_from_slice(args);
            super::Cli::try_parse_from(argv)
        };
        assert!(parse(&["--admin", "a"]).is_ok());
        assert!(parse(&["--no-admin"]).is_ok());
        assert!(parse(&["--create-admin", "a", "--admin-password-stdin"]).is_ok());
        assert!(parse(&["--admin", "a", "--no-admin"]).is_err());
        assert!(parse(&["--admin", "a", "--create-admin", "b"]).is_err());
        assert!(parse(&["--admin-password-stdin"]).is_err());
    }

    /// answers handed out one by one, as typed lines.
    fn typed(lines: &[&str]) -> impl FnMut(&str) -> anyhow::Result<String> {
        let mut lines: Vec<String> = lines.iter().rev().map(|l| l.to_string()).collect();
        move |_| Ok(lines.pop().unwrap_or_default())
    }

    #[test]
    fn the_questions_for_a_root_login_offer_a_new_account_or_the_web_ui() {
        use lanlord_core::firstadmin::Choice;
        let root = lanlord_core::account::LoginUser {
            name: "root".into(),
            is_root: true,
        };
        let create = || Ok(Choice::Adopt("made".into()));
        for answer in ["", "y", "YES"] {
            let got = super::ask_choice(&root, &mut typed(&[answer]), &mut { create }).unwrap();
            assert_eq!(got, Choice::Adopt("made".into()), "{answer}");
        }
        let got = super::ask_choice(&root, &mut typed(&["s"]), &mut { create }).unwrap();
        assert_eq!(got, Choice::Skip);
    }

    #[test]
    fn the_questions_for_another_login_offer_adopting_it_or_a_new_account() {
        use lanlord_core::firstadmin::Choice;
        let ann = lanlord_core::account::LoginUser {
            name: "ann".into(),
            is_root: false,
        };
        let create = || Ok(Choice::Adopt("made".into()));
        let ask = |answer: &str| super::ask_choice(&ann, &mut typed(&[answer]), &mut { create });
        assert_eq!(ask("").unwrap(), Choice::Adopt("ann".into()));
        assert_eq!(ask("yes").unwrap(), Choice::Adopt("ann".into()));
        assert_eq!(ask("c").unwrap(), Choice::Adopt("made".into()));
        assert_eq!(ask("create").unwrap(), Choice::Adopt("made".into()));
        assert_eq!(ask("s").unwrap(), Choice::Skip);
        assert_eq!(ask("anything else").unwrap(), Choice::Skip);
    }

    #[test]
    fn a_new_administrator_is_asked_for_a_name_a_password_and_a_key() {
        use lanlord_core::firstadmin::Choice;
        let dir = std::env::temp_dir().join(format!("lanlord-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = dir.join("id_ed25519.pub");
        let public = format!(
            "ssh-ed25519 {} olive@example",
            "AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl"
        );
        std::fs::write(&key, format!("{public}\n")).unwrap();
        let secret = format!("{}-{}", "fixture", 1);
        let mut pass = |name: &str| {
            assert_eq!(name, "olive");
            Ok(secret.clone())
        };
        let got = super::ask_new_admin_with(
            &mut typed(&["olive", "1"]),
            &mut pass,
            std::slice::from_ref(&key),
        )
        .unwrap();
        assert_eq!(
            got,
            Choice::Create {
                name: "olive".into(),
                password: zeroize::Zeroizing::new(secret.clone()),
                ssh_public_key: Some(public),
            }
        );
        // no key offered, or none chosen
        let got = super::ask_new_admin_with(&mut typed(&["olive"]), &mut pass, &[]).unwrap();
        assert!(matches!(
            got,
            Choice::Create {
                ssh_public_key: None,
                ..
            }
        ));
        let got = super::ask_new_admin_with(&mut typed(&["olive", ""]), &mut pass, &[key]).unwrap();
        assert!(matches!(
            got,
            Choice::Create {
                ssh_public_key: None,
                ..
            }
        ));
        // a name is needed
        assert!(super::ask_new_admin_with(&mut typed(&[""]), &mut pass, &[]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_step_shows_the_playbook_output_then_its_own_lines_and_nothing_older() {
        let mut out = super::StepOutput::default();
        out.push("old step line".into());
        out.start();
        out.push("pacman line".into());
        out.push("last line".into());
        let log = vec!["fatal: [gw]: FAILED! => boom".to_string()];
        assert_eq!(
            out.failure(log),
            ["fatal: [gw]: FAILED! => boom", "pacman line", "last line"]
        );
        // shown once: the hold is empty afterwards
        assert!(out.failure(Vec::new()).is_empty());
        // only the last 40 lines are kept
        for n in 0..50 {
            out.push(format!("line {n}"));
        }
        let shown = out.failure(vec!["first".into()]);
        assert_eq!(shown.len(), 40);
        assert_eq!(shown.last().unwrap(), "line 49");
        assert_eq!(shown[0], "line 10");
    }

    #[cfg(unix)]
    #[test]
    fn root_over_ssh_is_let_run_operations_and_mismatched_sudo_variables_are_not() {
        let uids = |name: &str| match name {
            "ann" => Some(1000),
            "root" => Some(0),
            _ => None,
        };
        // `sudo lanlord operate` as root: the same as no sudo, unless the variables lie
        assert_eq!(
            super::operating_account(Some("root"), Some("0"), uids).as_deref(),
            Some("root")
        );
        assert_eq!(
            super::operating_account(Some("root"), Some("1000"), uids),
            None
        );
        assert_eq!(super::operating_account(Some("root"), None, uids), None);
        // no sudo at all: a root login over ssh
        assert_eq!(
            super::operating_account(None, None, uids).as_deref(),
            Some("root")
        );
        assert_eq!(
            super::operating_account(Some("ann"), Some("1000"), uids).as_deref(),
            Some("ann")
        );
        // half set, or set to a lie: refused
        assert_eq!(super::operating_account(Some("ann"), None, uids), None);
        assert_eq!(super::operating_account(None, Some("0"), uids), None);
        assert_eq!(super::operating_account(Some("ann"), Some("0"), uids), None);
    }

    #[cfg(unix)]
    #[test]
    fn root_is_an_administrator_without_the_group() {
        let session = lanlord_core::ssh::Session::local();
        let caller = super::operating_caller(&session, "root").unwrap();
        assert_eq!((caller.user.as_str(), caller.admin), ("root", true));
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

    /// what the fake gateway of `preflight_with` does.
    struct Knobs {
        needs: bool,
        key: bool,
        root: bool,
        yes: bool,
        answer: bool,
        typed_ok: bool,
        probe_fails: bool,
        move_fails: bool,
    }

    fn preflight(
        needs: bool,
        key: bool,
        root: bool,
        yes: bool,
        answer: bool,
    ) -> (Result<()>, Vec<String>, bool) {
        preflight_with(Knobs {
            needs,
            key,
            root,
            yes,
            answer,
            typed_ok: true,
            probe_fails: false,
            move_fails: false,
        })
    }

    /// one run of `upgrade_steps` with a fake gateway; the log says what was called, and the
    /// last answer is whether the old key file is gone.
    fn preflight_with(k: Knobs) -> (Result<()>, Vec<String>, bool) {
        let dir = std::env::temp_dir().join(format!(
            "lanlord-preflight-{}-{}{}{}{}{}{}{}{}",
            std::process::id(),
            k.needs,
            k.key,
            k.root,
            k.yes,
            k.answer,
            k.typed_ok,
            k.probe_fails,
            k.move_fails
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("identity");
        std::fs::write(&file, "AGE-SECRET-KEY-1TEST").unwrap();
        let log = std::cell::RefCell::new(Vec::new());
        let moved = std::cell::Cell::new(false);
        let target = super::UpgradeTarget {
            host: "root@192.0.2.9",
            yes: k.yes,
            root: k.root,
            key: k.key.then_some(file.as_path()),
        };
        let result = super::upgrade_steps(
            &target,
            &mut || {
                log.borrow_mut().push("check".to_string());
                if k.probe_fails {
                    bail!("no answer");
                }
                Ok(k.needs && !moved.get())
            },
            &mut |operation| {
                use lanlord_core::operations::{Operation, Reply};
                Ok(match operation {
                    Operation::MoveSecrets { .. } => {
                        log.borrow_mut().push("move".to_string());
                        if k.move_fails {
                            bail!("the gateway refused");
                        }
                        moved.set(true);
                        Reply::Done
                    }
                    _ => Reply::Document(Default::default()),
                })
            },
            &mut |_| {
                log.borrow_mut().push("ask".to_string());
                Ok(k.answer)
            },
            &mut || {
                log.borrow_mut().push("host".to_string());
                if k.typed_ok {
                    Ok(())
                } else {
                    bail!("the host name does not match; nothing was changed")
                }
            },
            &mut || {
                log.borrow_mut().push("upgrade".to_string());
                Ok(())
            },
        );
        let erased = !file.exists();
        let _ = std::fs::remove_dir_all(&dir);
        (result, log.into_inner(), erased)
    }

    #[test]
    fn a_wrong_host_name_stops_before_the_move_and_leaves_the_key() {
        let (result, log, erased) = preflight_with(Knobs {
            needs: true,
            key: true,
            root: false,
            yes: false,
            answer: true,
            typed_ok: false,
            probe_fails: false,
            move_fails: false,
        });
        assert!(result.is_err());
        assert_eq!(log, ["host"]);
        assert!(!erased);
    }

    #[test]
    fn a_failed_probe_warns_and_the_upgrade_goes_on() {
        let (result, log, _) = preflight_with(Knobs {
            needs: true,
            key: true,
            root: false,
            yes: true,
            answer: true,
            typed_ok: true,
            probe_fails: true,
            move_fails: false,
        });
        result.unwrap();
        assert_eq!(log, ["check", "upgrade"]);
    }

    #[test]
    fn a_failed_move_keeps_the_key_and_does_not_start_the_upgrade() {
        let (result, log, erased) = preflight_with(Knobs {
            needs: true,
            key: true,
            root: false,
            yes: true,
            answer: true,
            typed_ok: true,
            probe_fails: false,
            move_fails: true,
        });
        let message = format!("{:#}", result.unwrap_err());
        assert!(
            message.contains("lanlord move-secrets <your-admin>@192.0.2.9"),
            "{message}"
        );
        assert_eq!(log, ["check", "move"]);
        assert!(!erased);
    }

    #[test]
    fn the_upgrade_goes_straight_on_when_nothing_needs_moving() {
        let (result, log, erased) = preflight(false, true, false, false, false);
        result.unwrap();
        assert_eq!(log, ["host", "check", "upgrade"]);
        assert!(!erased);
    }

    #[test]
    fn a_move_is_done_first_when_the_key_is_here_and_the_user_agrees() {
        let (result, log, erased) = preflight(true, true, false, false, true);
        result.unwrap();
        assert_eq!(log, ["host", "check", "ask", "move", "upgrade"]);
        assert!(erased);
    }

    #[test]
    fn yes_moves_without_asking_and_a_no_stops_before_any_change() {
        let (result, log, _) = preflight(true, true, false, true, false);
        result.unwrap();
        assert_eq!(log, ["check", "move", "upgrade"]);
        let (result, log, erased) = preflight(true, true, false, false, false);
        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("nothing was changed"), "{message}");
        assert!(message.contains("lanlord move-secrets <your-admin>@192.0.2.9"));
        assert_eq!(log, ["host", "check", "ask"]);
        assert!(!erased);
    }

    #[test]
    fn without_the_old_key_here_the_upgrade_stops_before_any_change() {
        let (result, log, _) = preflight(true, false, false, true, true);
        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("nothing was changed"), "{message}");
        assert!(message.contains("`lanlord move-secrets <your-admin>@192.0.2.9`"));
        assert_eq!(log, ["check"]);
    }

    #[test]
    fn a_root_target_needing_a_move_stops_with_the_command_before_any_change() {
        let (result, log, erased) = preflight(true, true, true, true, true);
        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("as root"), "{message}");
        assert!(message.contains("`lanlord move-secrets <your-admin>@192.0.2.9`"));
        assert_eq!(log, ["check"]);
        assert!(!erased);
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
            sockets: 1,
            cores: 2,
            cpus_limited: false,
            mem_total_mb: 2_048,
            mem_available_mb: 1_024,
            mem_limited: false,
            lanlord_installed: true,
            lanlord_version: Some("0.1.0".into()),
            last_applied: Some("2026-01-01T12:00:00Z".into()),
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
            "cores:         2",
            "memory:        1024 MB free of 2048 MB",
            "lanlord:       installed, version 0.1.0",
            "interface:     eth0 Connected (UP, default route) 192.0.2.10/24",
            "interface:     eth1 Disconnected (DOWN) ",
        ] {
            assert!(text.contains(want), "{want:?} in\n{text}");
        }
        assert!(text.contains("last applied:  ") && !text.contains("T12:00:00Z"));
        // threads past the cores show both; a container's limit shows threads alone
        facts.cpus = 4;
        let text = fact_lines(&facts).join("\n");
        assert!(text.contains("cores:         2\n") && text.contains("threads:       4\n"));
        facts.cpus_limited = true;
        let text = fact_lines(&facts).join("\n");
        assert!(
            text.contains("threads:       4 (limited by container)") && !text.contains("cores:")
        );
        facts.cpus_limited = false;
        facts.sockets = 2;
        assert!(fact_lines(&facts).join("\n").contains("processors:    2\n"));
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
    fn the_ssh_commands_parse_and_become_changes() {
        use lanlord_model::ssh_access::Change;
        let command = |args: &[&str]| {
            let mut full = vec!["ssh", "lab"];
            full.extend_from_slice(args);
            match networks_action(&full) {
                NetworksCommand::Ssh { action, .. } => action,
                _ => panic!("ssh"),
            }
        };
        let now = 1_900_000_000;
        let on = ssh_change("lab", command(&["on"]), now).unwrap();
        assert_eq!(
            on,
            Change::Network {
                network: "lab".into(),
                on: true
            }
        );
        assert!(matches!(
            ssh_change("lab", command(&["off"]), now).unwrap(),
            Change::Network { on: false, .. }
        ));
        let mac = "02:00:00:00:00:01";
        match ssh_change(
            "lab",
            command(&["allow", mac, "--for", "1h", "--name", "dana-pc"]),
            now,
        )
        .unwrap()
        {
            Change::Allow {
                mac: m,
                name,
                expiry,
                ..
            } => {
                assert_eq!(m, mac);
                assert_eq!(name.as_deref(), Some("dana-pc"));
                assert_eq!(expiry, lanlord_model::internet::Expiry::At(now + 3600));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            ssh_change("lab", command(&["deny", mac]), now).unwrap(),
            Change::Deny { .. }
        ));
        assert!(ssh_change("lab", SshCommand::List, now).is_err());
        assert!(ssh_change("lab", command(&["allow", mac, "--for", "soon"]), now).is_err());
        assert!(Cli::try_parse_from(["lanlord", "networks", "gw", "ssh", "lab", "allow"]).is_err());
    }

    #[test]
    fn the_internet_commands_parse() {
        let NetworksCommand::Set { internet, .. } =
            networks_action(&["set", "lab", "--internet", "on"])
        else {
            panic!("set");
        };
        assert!(matches!(internet, Some(Switch::On)));
        assert!(Cli::try_parse_from([
            "lanlord",
            "networks",
            "gw",
            "set",
            "lab",
            "--internet",
            "maybe"
        ])
        .is_err());
        let NetworksCommand::Internet { name, action } = networks_action(&[
            "internet",
            "lab",
            "allow",
            "02:00:00:00:00:01",
            "--for",
            "1h",
            "--name",
            "dana-pc",
        ]) else {
            panic!("internet");
        };
        assert_eq!(name, "lab");
        let InternetCommand::Allow {
            mac,
            duration,
            until,
            name,
        } = action
        else {
            panic!("allow");
        };
        assert_eq!(
            (mac.as_str(), duration.as_deref(), until, name.as_deref()),
            ("02:00:00:00:00:01", Some("1h"), None, Some("dana-pc"))
        );
        let NetworksCommand::Internet {
            action: InternetCommand::Allow { until, .. },
            ..
        } = networks_action(&[
            "internet",
            "lab",
            "allow",
            "02:00:00:00:00:01",
            "--until",
            "2030-01-31 09:00",
        ])
        else {
            panic!("allow until");
        };
        assert_eq!(until.as_deref(), Some("2030-01-31 09:00"));
        assert!(matches!(
            networks_action(&["internet", "lab", "deny", "02:00:00:00:00:01"]),
            NetworksCommand::Internet {
                action: InternetCommand::Deny { .. },
                ..
            }
        ));
        assert!(matches!(
            networks_action(&["internet", "lab", "list"]),
            NetworksCommand::Internet {
                action: InternetCommand::List,
                ..
            }
        ));
        // for good is the default, and --for and --until are one or the other
        assert!(
            Cli::try_parse_from(["lanlord", "networks", "gw", "internet", "lab", "allow"]).is_err()
        );
        assert!(Cli::try_parse_from([
            "lanlord",
            "networks",
            "gw",
            "internet",
            "lab",
            "allow",
            "02:00:00:00:00:01",
            "--for",
            "1h",
            "--until",
            "2030-01-31 09:00",
        ])
        .is_err());
        // --internet is not mixed with the other changes of `set`
        let mixed = NetworksCommand::Set {
            name: "lab".into(),
            interface: None,
            address: None,
            mtu: Some(1400),
            no_mtu: false,
            zone: None,
            dhcp_range: None,
            lease: None,
            no_dhcp: false,
            internet: Some(Switch::On),
        };
        let refused = set_internet_change(&mixed)
            .expect("--internet is seen")
            .unwrap_err();
        assert!(refused.to_string().contains("on its own"), "{refused}");
        assert!(set_internet_change(&NetworksCommand::List).is_none());
        // on its own it is the change for the whole network
        let off = networks_action(&["set", "lab", "--internet", "off"]);
        match set_internet_change(&off) {
            Some(Ok(lanlord_model::internet::Change::Network { network, on })) => {
                assert_eq!((network.as_str(), on), ("lab", false));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn allow_and_deny_become_changes_with_the_end_worked_out() {
        use lanlord_model::internet::{Change, Expiry};
        let command = |args: &[&str]| match networks_action(args) {
            NetworksCommand::Internet { action, .. } => action,
            _ => panic!("internet"),
        };
        let mac = "02:00:00:00:00:01";
        let now = 1_900_000_000;
        // for good, for a while, and until a time
        let for_good = allow_change("lab", command(&["internet", "lab", "allow", mac]), now);
        assert!(matches!(
            for_good.unwrap(),
            Change::Allow {
                expiry: Expiry::Never,
                ..
            }
        ));
        let hour = command(&[
            "internet", "lab", "allow", mac, "--for", "1h", "--name", "dana-pc",
        ]);
        match allow_change("lab", hour, now).unwrap() {
            Change::Allow {
                network,
                mac: got,
                name,
                expiry,
            } => {
                assert_eq!((network.as_str(), got.as_str()), ("lab", mac));
                assert_eq!(name.as_deref(), Some("dana-pc"));
                assert_eq!(expiry, Expiry::At(now + 3_600));
            }
            other => panic!("{other:?}"),
        }
        let until = command(&[
            "internet",
            "lab",
            "allow",
            mac,
            "--until",
            "2999-01-31 09:00",
        ]);
        assert!(matches!(
            allow_change("lab", until, now).unwrap(),
            Change::Allow {
                expiry: Expiry::At(_),
                ..
            }
        ));
        let deny = allow_change("lab", command(&["internet", "lab", "deny", mac]), now);
        assert!(matches!(deny.unwrap(), Change::Deny { .. }));
        // what cannot be understood is said plainly
        let bad_for = command(&["internet", "lab", "allow", mac, "--for", "soon"]);
        assert!(allow_change("lab", bad_for, now).is_err());
        let bad_until = command(&["internet", "lab", "allow", mac, "--until", "tomorrow"]);
        assert!(allow_change("lab", bad_until, now).is_err());
        assert!(allow_change("lab", InternetCommand::List, now).is_err());
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
