//! the nt4-domain module, natively: the machines the playbook (`nt4-domain.yml`) makes from the
//! `nt4-servers` list, with the same files, the same containers and the same `lanlord-user` calls.
//! what it will not do it says is `Unsupported` before it changes anything, and the playbook does
//! it: a change of a domain's identity (which snapshots samba's state first), an image that has to
//! be built here, and settings the playbook's own checks would refuse (so the refusal reads the
//! same). its results are meant to match the playbook's; `tools/engine-parity.sh` compares them.
//!
//! each server in the list has its own container (`lanlord-nt4-<name>`, and `lanlord-nt4` for the
//! server named `nt4-domain`), its own data folder under `/opt/lanlord/nt4` and its own fingerprints.
//! a server taken out of the list loses its container, never its data.
//!
//! three decisions that differ from running the playbook:
//! - a password is set only when it changed in LANlord (a fingerprint of it is kept, see
//!   `state.rs`), so a password a person changed from Windows is not set back by the next apply.
//! - the right to join computers (`SeMachineAccountPrivilege`) is checked for Domain Admins, so a
//!   personal Domain Admin can join a machine, not only Administrator. the `add machine script`
//!   hook in smb.conf is what creates the account when it is joined.
//! - an account that already exists and whose password did not change still gets its unix account,
//!   groups and home folder checked (`lanlord-user ensure`), as a new container starts without them.

use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::Result;
use lanlord_model::apply::ApplyEvent;
use lanlord_model::engine::{Action, ChangeView};
use serde_yaml_ng::{Mapping, Value};
use sha2::{Digest, Sha256};

use crate::engine::host::Host;
use crate::engine::journal::Journal;
use crate::engine::resource::{Container, Dir, File, Resource, Unsupported};
use crate::engine::state::{hex, State};
use crate::engine::{self, Events, Secret, Tally};

pub const ID: &str = "nt4-domain";

const ROOT: &str = "/opt/lanlord/nt4";
const IMAGE: &str = "ghcr.io/muncherelli/lanlord-nt4-server-components";
/// the server that keeps the container and the folder from before there was a list.
const LEGACY: &str = "nt4-domain";

const ENTRYPOINT: &str =
    include_str!("../../../../../ansible/playbooks/nt4-domain/image/entrypoint.sh");
const LANLORD_USER: &str =
    include_str!("../../../../../ansible/playbooks/nt4-domain/image/lanlord-user");
const LANLORD_EXEC: &str =
    include_str!("../../../../../ansible/playbooks/nt4-domain/image/lanlord-exec");
const SMB_CONF: &str = include_str!("../../../../../ansible/playbooks/nt4-domain/smb.conf.j2");

/// what an apply says when no password is recorded as set.
const FIRST_APPLY: &str = "no password is recorded as set yet (this is the first native apply, or the fingerprint key was lost and made again), so every password in the settings is set once now, a password changed from Windows included; to keep those, stop and run `sudo lanlord engine record --module nt4-domain` first (it marks every existing account's password as already set and keeps it as it is, even if it differs from the settings)";

fn unsupported(why: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Unsupported(why.into()))
}

struct User {
    name: String,
    password: Secret,
    admin: bool,
    software: bool,
}

/// one server of the list, with what its domain and the shared section add.
struct Server {
    id: String,
    container: String,
    dir: String,
    /// `domain` or `workgroup`
    mode: String,
    /// the domain or workgroup name, upper case
    name: String,
    netbios: String,
    address: String,
    /// whether it serves wins
    wins: bool,
    roaming: bool,
    software: bool,
    machines: Vec<String>,
    /// what the samba configuration reads as `nt4`: the `nt4-domain` section and the server's own
    section: Value,
}

impl Server {
    fn domain(&self) -> bool {
        self.mode == "domain"
    }

    /// the name a fingerprint of this server's `what` is kept under.
    fn key(&self, what: &str) -> String {
        format!("{}/{what}", self.id)
    }
}

struct Config {
    servers: Vec<Server>,
    admin: Option<Secret>,
    users: Vec<User>,
    absent: Vec<String>,
}

impl Config {
    fn secrets(&self) -> Vec<Secret> {
        self.admin
            .iter()
            .cloned()
            .chain(self.users.iter().map(|u| u.password.clone()))
            .collect()
    }

    /// the containers the list asks for.
    fn wanted(&self) -> Vec<String> {
        self.servers.iter().map(|s| s.container.clone()).collect()
    }
}

fn text(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn flag(v: Option<&Value>) -> bool {
    v.and_then(Value::as_bool) == Some(true)
}

/// a true or false setting; anything else is for the playbook to read.
fn switch(v: Option<&Value>, what: &str) -> Result<bool> {
    match v {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(unsupported(format!("{what} is not true or false"))),
    }
}

fn netbios_ok(s: &str) -> bool {
    lanlord_model::nt4::netbios_ok(s) && s == s.to_ascii_uppercase()
}

fn address_ok(s: &str) -> bool {
    let Some((ip, prefix)) = s.split_once('/') else {
        return false;
    };
    !ip.is_empty()
        && ip.chars().all(|c| c.is_ascii_digit() || c == '.')
        && (1..=2).contains(&prefix.len())
        && prefix.chars().all(|c| c.is_ascii_digit())
}

/// a server's name: letters, digits and inner hyphens, up to 63, and not a folder of the module.
fn id_ok(id: &str) -> bool {
    let edge = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    (1..=63).contains(&id.len())
        && edge(id.chars().next())
        && edge(id.chars().last())
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !["bin", "config", "lib", "snapshots"].contains(&id)
        && !id.ends_with("-prev")
}

/// the admin password, the users and the names to remove, from the `nt4-domain` section.
fn logins(section: &Mapping) -> Result<(Option<Secret>, Vec<User>, Vec<String>)> {
    let admin = text(section.get("admin_password"))
        .filter(|s| !s.is_empty())
        .map(Secret::new);
    let mut users = Vec::new();
    let mut absent = Vec::new();
    match section.get("users") {
        None | Some(Value::Null) => {}
        Some(Value::Sequence(items)) => {
            for item in items {
                let Some(user) = item.as_mapping() else {
                    return Err(unsupported("a user is not a mapping"));
                };
                let Some(name) = text(user.get("name")) else {
                    return Err(unsupported("a user has no name"));
                };
                if text(user.get("state")).as_deref() == Some("absent") {
                    absent.push(name);
                    continue;
                }
                let Some(password) = text(user.get("password")) else {
                    return Err(unsupported("a user has no password"));
                };
                users.push(User {
                    name,
                    password: Secret::new(password),
                    admin: flag(user.get("admin")),
                    software: flag(user.get("software_admin")),
                });
            }
        }
        Some(_) => return Err(unsupported("nt4-domain users is not a list")),
    }
    Ok((admin, users, absent))
}

/// the settings of the domain called `name` (its key in `domains` is matched without case).
fn domain_of(doc: &Value, name: &str) -> Result<Mapping> {
    let domains = match doc.get("domains") {
        None | Some(Value::Null) => return Ok(Mapping::new()),
        Some(Value::Mapping(map)) => map,
        Some(_) => return Err(unsupported("domains is not a mapping")),
    };
    let found = domains
        .iter()
        .find(|(key, _)| text(Some(key)).is_some_and(|key| key.eq_ignore_ascii_case(name)));
    match found.map(|(_, value)| value) {
        None | Some(Value::Null) => Ok(Mapping::new()),
        Some(Value::Mapping(map)) => Ok(map.clone()),
        Some(_) => Err(unsupported("a domain is not a mapping")),
    }
}

fn machines_of(domain: &Mapping) -> Result<Vec<String>> {
    match domain.get("machines") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Sequence(items)) => {
            let machines: Vec<String> = items.iter().filter_map(|m| text(Some(m))).collect();
            if machines.len() == items.len() {
                Ok(machines)
            } else {
                Err(unsupported("a machine is not a name"))
            }
        }
        Some(_) => Err(unsupported("machines is not a list")),
    }
}

/// the server's netbios name: its own, or this machine's host name.
fn netbios_of(host: &dyn Host, srv: &Mapping) -> Result<String> {
    let name = match text(srv.get("netbios_name")) {
        Some(n) => n,
        None => host
            .run(&["hostname"], None)?
            .stdout
            .trim()
            .split('.')
            .next()
            .unwrap_or("")
            .to_string(),
    };
    Ok(name.to_ascii_uppercase().chars().take(15).collect())
}

/// one server of the list, checked as the playbook checks it.
fn server(host: &dyn Host, doc: &Value, section: &Mapping, item: &Value) -> Result<Server> {
    let Some(srv) = item.as_mapping() else {
        return Err(unsupported("a server is not a mapping"));
    };
    let id = text(srv.get("name")).unwrap_or_default();
    let role = text(srv.get("role")).unwrap_or_default();
    let mode = if role == "pdc" { "domain".into() } else { role };
    let name = text(srv.get("domain"))
        .or_else(|| text(srv.get("workgroup")))
        .unwrap_or_default()
        .to_ascii_uppercase();
    let netbios = netbios_of(host, srv)?;
    let network = text(srv.get("network")).unwrap_or_default();
    let address = match text(srv.get("address")) {
        Some(address) => address,
        None => network_address(doc, &network).ok_or_else(|| {
            unsupported("the server's address comes from the playbook's network rules")
        })?,
    };
    let wins = text(srv.get("wins")).unwrap_or_else(|| "serve".into()) == "serve";
    let domain = domain_of(doc, &name)?;
    let roaming = switch(domain.get("roaming_profiles"), "roaming_profiles")?;
    let software = switch(srv.get("software"), "software")?;
    let machines = machines_of(&domain)?;
    if !id_ok(&id)
        || !(mode == "domain" || mode == "workgroup")
        || !netbios_ok(&name)
        || !netbios_ok(&netbios)
        || !address_ok(&address)
        || !machines
            .iter()
            .all(|m| lanlord_model::nt4::valid_machine(m))
    {
        return Err(unsupported(
            "the nt4 server settings need the playbook's checks and messages",
        ));
    }
    let mut merged = section.clone();
    let put = |map: &mut Mapping, key: &str, value: Value| {
        map.insert(Value::String(key.into()), value);
    };
    put(&mut merged, "mode", Value::String(mode.clone()));
    put(&mut merged, "name", Value::String(name.clone()));
    put(&mut merged, "network", Value::String(network));
    put(&mut merged, "roaming_profiles", Value::Bool(roaming));
    put(
        &mut merged,
        "machines",
        Value::Sequence(machines.iter().cloned().map(Value::String).collect()),
    );
    put(&mut merged, "software", Value::Bool(software));
    let legacy = id == LEGACY;
    Ok(Server {
        container: if legacy {
            "lanlord-nt4".into()
        } else {
            format!("lanlord-nt4-{id}")
        },
        dir: if legacy {
            ROOT.into()
        } else {
            format!("{ROOT}/{id}")
        },
        id,
        mode,
        name,
        netbios,
        address,
        wins,
        roaming,
        software,
        machines,
        section: Value::Mapping(merged),
    })
}

/// the servers of the settings, checked as the playbook checks them. `None` when the settings
/// have no `nt4-servers` list: the module then does nothing.
fn config(host: &dyn Host, doc: &Value) -> Result<Option<Config>> {
    let Some(listed) = doc.get("nt4-servers") else {
        return Ok(None);
    };
    let items = match listed {
        Value::Null => &[][..],
        Value::Sequence(items) => items.as_slice(),
        _ => return Err(unsupported("nt4-servers is not a list")),
    };
    let section = match doc.get(ID) {
        None | Some(Value::Null) => Mapping::new(),
        Some(Value::Mapping(map)) => map.clone(),
        Some(_) => return Err(unsupported("the nt4-domain settings are not a mapping")),
    };
    let (admin, users, absent) = logins(&section)?;
    let mut servers: Vec<Server> = Vec::new();
    for item in items {
        let one = server(host, doc, &section, item)?;
        if servers.iter().any(|s| s.id == one.id) {
            return Err(unsupported("two servers have the same name"));
        }
        servers.push(one);
    }
    if servers.iter().any(Server::domain) && admin.is_none() {
        return Err(unsupported("a domain needs an admin_password"));
    }
    Ok(Some(Config {
        servers,
        admin,
        users,
        absent,
    }))
}

/// the address of the network the module is on, when the settings list the networks plainly.
fn network_address(doc: &Value, wanted: &str) -> Option<String> {
    let networks = doc.get("networks")?.as_sequence()?;
    let found = if wanted.is_empty() {
        networks.first()
    } else {
        networks
            .iter()
            .find(|n| text(n.get("name")).is_some_and(|n| n.eq_ignore_ascii_case(wanted)))
    }?;
    text(found.get("address"))
}

/// the version of the scripts: the first 12 hex digits of the sha256 of all three, one after the other.
pub fn scripts_hash() -> String {
    let mut sha = Sha256::new();
    sha.update(ENTRYPOINT.as_bytes());
    sha.update(LANLORD_USER.as_bytes());
    sha.update(LANLORD_EXEC.as_bytes());
    hex(&sha.finalize())[..12].to_string()
}

/// the samba configuration, from the template the playbook uses. a name the template reads that
/// is not given is an error, not an empty string.
fn render_conf(server: &Server) -> Result<String> {
    let mut env = minijinja::Environment::new();
    env.set_trim_blocks(true);
    env.set_keep_trailing_newline(true);
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
    env.add_template("smb.conf", SMB_CONF)?;
    let context = minijinja::context! {
        nt4 => minijinja::Value::from_serialize(&server.section),
        nt4_name => &server.name,
        nt4_netbios_name => &server.netbios,
        nt4_address => &server.address,
        nt4_mode => &server.mode,
        nt4_wins => server.wins,
    };
    Ok(env.get_template("smb.conf")?.render(context)?)
}

fn logon_script(server: &Server) -> String {
    let unc = format!("\\\\{}", server.netbios);
    let lines = [
        "@echo off".to_string(),
        if server.software {
            format!("net use S: {unc}\\software /persistent:no")
        } else {
            String::new()
        },
        if server.roaming {
            "if not \"%OS%\"==\"Windows_NT\" net use H: /home".to_string()
        } else {
            String::new()
        },
        format!("if exist {unc}\\netlogon\\logon.bat call {unc}\\netlogon\\logon.bat"),
        format!("if exist {unc}\\netlogon\\%USERNAME%.bat call {unc}\\netlogon\\%USERNAME%.bat"),
    ];
    format!("{}\r\n", lines.join("\r\n"))
}

fn image() -> String {
    format!("{IMAGE}:{}", env!("LANLORD_IMAGE_VERSION"))
}

fn docker(host: &dyn Host, args: &[&str]) -> Result<engine::host::Output> {
    let mut argv = vec!["docker"];
    argv.extend_from_slice(args);
    host.run(&argv, None)
}

struct Prepared {
    config: Config,
    /// the folder all servers share and the scripts in it
    common: Vec<Box<dyn Resource>>,
    /// what each server needs, in the order of `config.servers`
    servers: Vec<Vec<Box<dyn Resource>>>,
}

/// the words of an earlier smb.conf that say which domain it is.
fn identity_changed(old: &str, server: &Server) -> bool {
    let word = |key: &str| {
        old.lines()
            .find_map(|l| {
                l.trim_start()
                    .strip_prefix(key)?
                    .split_whitespace()
                    .next()
                    .map(String::from)
            })
            .unwrap_or_default()
    };
    let logons = old.lines().any(|l| l.contains("domain logons = yes"));
    word("workgroup = ") != server.name
        || word("netbios name = ") != server.netbios
        || logons != server.domain()
}

/// the container of `server` and the files it reads.
fn resources_of(
    server: &Server,
    image: &str,
    image_id: &str,
    hash: &str,
) -> Result<Vec<Box<dyn Resource>>> {
    let conf_path = format!("{}/config/smb.conf", server.dir);
    let conf = File {
        path: conf_path.clone(),
        content: render_conf(server)?.into_bytes(),
        mode: 0o640,
        sensitive: false,
    };
    let conf_id = conf.id();
    let mut run: Vec<String> = [
        "docker",
        "run",
        "-d",
        "--name",
        &server.container,
        "--network",
        "host",
        "--restart",
        "unless-stopped",
        "--label",
    ]
    .map(String::from)
    .to_vec();
    run.push(format!("lanlord.scripts={hash}"));
    for (from, to, mode) in [
        (
            format!("{ROOT}/bin/entrypoint.sh"),
            "/usr/local/bin/entrypoint.sh",
            ":ro",
        ),
        (
            format!("{ROOT}/bin/lanlord-user"),
            "/usr/local/bin/lanlord-user",
            ":ro",
        ),
        (
            format!("{ROOT}/bin/lanlord-exec"),
            "/usr/local/bin/lanlord-exec",
            ":ro",
        ),
        (format!("{}/lib", server.dir), "/var/lib/samba", ""),
        (conf_path, "/etc/samba/smb.conf", ":ro"),
    ] {
        run.push("-v".into());
        run.push(format!("{from}:{to}{mode}"));
    }
    run.push(image.to_string());
    let mut container = Container::new(
        &server.container,
        format!("true {image_id} {hash}"),
        "{{.State.Running}} {{.Image}} {{index .Config.Labels \"lanlord.scripts\"}}",
        run,
        [
            "docker",
            "exec",
            &server.container,
            "smbcontrol",
            "smbd",
            "ping",
        ]
        .map(String::from)
        .to_vec(),
    );
    container.retries = 20;
    container.follows = vec![conf_id];
    let mut resources: Vec<Box<dyn Resource>> = Vec::new();
    if server.dir != ROOT {
        resources.push(Box::new(Dir::new(&server.dir, 0o750)));
    }
    resources.push(Box::new(Dir::new(&format!("{}/config", server.dir), 0o750)));
    resources.push(Box::new(Dir::new(&format!("{}/lib", server.dir), 0o751)));
    resources.push(Box::new(conf));
    resources.push(Box::new(container));
    Ok(resources)
}

/// everything that is decided before the machine changes. fails with `Unsupported` when the
/// playbook has to do it.
fn prepare(host: &dyn Host, doc: &Value) -> Result<Option<Prepared>> {
    let Some(config) = config(host, doc)? else {
        return Ok(None);
    };
    if !docker(host, &["info", "--format", "{{.ServerVersion}}"])?.success() {
        return Err(unsupported("docker does not answer"));
    }
    // a different name, server name or mode is a different domain: its state is kept first
    for server in &config.servers {
        if let Some(old) = host.read(&format!("{}/config/smb.conf", server.dir))? {
            if identity_changed(&String::from_utf8_lossy(&old), server) {
                return Err(unsupported(
                    "the domain identity changes, samba's state is snapshotted first",
                ));
            }
        }
    }
    let mut servers = Vec::new();
    if !config.servers.is_empty() {
        let image = image();
        // the published image, or the playbook builds it here
        if !docker(host, &["pull", "--quiet", &image])?.success() {
            return Err(unsupported(
                "the image cannot be pulled, so it is built here",
            ));
        }
        let id = docker(host, &["image", "inspect", "--format", "{{.Id}}", &image])?;
        if !id.success() {
            return Err(unsupported("the image is not there"));
        }
        let hash = scripts_hash();
        for server in &config.servers {
            servers.push(resources_of(server, &image, id.stdout.trim(), &hash)?);
        }
    }
    let common: Vec<Box<dyn Resource>> = vec![
        Box::new(Dir::new(ROOT, 0o750)),
        Box::new(Dir::new(&format!("{ROOT}/bin"), 0o750)),
        Box::new(File::new(
            &format!("{ROOT}/bin/entrypoint.sh"),
            ENTRYPOINT,
            0o755,
        )),
        Box::new(File::new(
            &format!("{ROOT}/bin/lanlord-user"),
            LANLORD_USER,
            0o755,
        )),
        Box::new(File::new(
            &format!("{ROOT}/bin/lanlord-exec"),
            LANLORD_EXEC,
            0o755,
        )),
    ];
    Ok(Some(Prepared {
        config,
        common,
        servers,
    }))
}

/// the names of the accounts of `server`, from `pdbedit -L`. a container that does not answer is
/// an error: it is not the same as having no accounts.
fn accounts(host: &dyn Host, server: &Server) -> Result<BTreeSet<String>> {
    let out = docker(host, &["exec", &server.container, "lanlord-user", "list"])?;
    if !out.success() {
        anyhow::bail!(
            "lanlord-user list failed in {}: {}",
            server.container,
            out.stderr.trim()
        );
    }
    Ok(out
        .stdout
        .lines()
        .filter_map(|l| l.split(':').next())
        .filter(|n| !n.is_empty())
        .map(String::from)
        .collect())
}

/// what the accounts of one server still need, given the accounts there are and what was set before.
#[derive(Default)]
struct Needs {
    admin: bool,
    /// the computers to make an account for (a plan shows those that are missing)
    machines: Vec<String>,
    /// indexes into `Config::users` whose password is to be set
    users: Vec<usize>,
    /// users that exist and keep their password: only their unix side is checked
    ensure: Vec<String>,
    deletes: Vec<String>,
}

/// `known` is whether what was set is recorded; when it is not (the playbook set the passwords and
/// recorded nothing) only a missing account counts.
fn needs(
    config: &Config,
    server: &Server,
    have: &BTreeSet<String>,
    state: &State,
    known: bool,
) -> Needs {
    let mut needs = Needs::default();
    let current =
        |what: &str, secret: &Secret| !known || state.is_current(&server.key(what), secret);
    if server.domain() {
        if let Some(admin) = &config.admin {
            needs.admin = !have.contains("root") || !current("admin", admin);
        }
        needs.machines = server
            .machines
            .iter()
            .map(|m| m.to_ascii_lowercase())
            .collect();
    }
    for (index, user) in config.users.iter().enumerate() {
        if !have.contains(&user.name) || !current(&format!("user:{}", user.name), &user.password) {
            needs.users.push(index);
        } else {
            needs.ensure.push(user.name.clone());
        }
    }
    needs.deletes = config
        .absent
        .iter()
        .filter(|n| have.contains(*n))
        .cloned()
        .collect();
    needs
}

fn view(resource: &str, action: Action, summary: String, sensitive: bool) -> ChangeView {
    ChangeView {
        resource: resource.into(),
        action,
        summary,
        diff: None,
        sensitive,
    }
}

/// the containers named like the module's that the list does not ask for (and their `-prev`).
fn gone(host: &dyn Host, config: &Config) -> Result<Vec<String>> {
    let listed = docker(
        host,
        &[
            "ps",
            "-a",
            "--filter",
            "name=^lanlord-nt4",
            "--format",
            "{{.Names}}",
        ],
    )?;
    if !listed.success() {
        anyhow::bail!("docker ps failed: {}", listed.stderr.trim());
    }
    let wanted = config.wanted();
    Ok(listed
        .stdout
        .lines()
        .map(str::trim)
        .filter(|name| ours(name))
        .filter(|name| {
            !wanted
                .iter()
                .any(|w| w == name.strip_suffix("-prev").unwrap_or(name))
        })
        .map(String::from)
        .collect())
}

/// whether `name` is `lanlord-nt4` or `lanlord-nt4-<name>`: the only containers the module touches.
fn ours(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("lanlord-nt4") else {
        return false;
    };
    let alnum = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    match rest.strip_prefix('-') {
        None => rest.is_empty(),
        Some(tail) => {
            alnum(tail.chars().next())
                && alnum(tail.chars().last())
                && tail.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        }
    }
}

/// what an apply would change. nothing is changed (an image is pulled, as the playbook does).
/// `shadow` is a plan after the playbook ran: it set the passwords but recorded nothing, so a
/// password is not reported as changed, only an account that is missing.
pub fn plan(
    host: &dyn Host,
    doc: &Value,
    state_dir: &str,
    shadow: bool,
) -> Result<Vec<ChangeView>> {
    let Some(prepared) = prepare(host, doc)? else {
        return Ok(Vec::new());
    };
    let mut out = engine::views(&engine::look(host, &prepared.common)?);
    let state = State::load(host, state_dir, false)?;
    for (server, resources) in prepared.config.servers.iter().zip(&prepared.servers) {
        let changes = engine::look(host, resources)?;
        out.extend(engine::views(&changes));
        if changes.last().is_some_and(Option::is_some) {
            out.push(view(
                &format!("accounts of {}", server.container),
                Action::Run,
                "accounts and the logon script are checked once the container runs".into(),
                false,
            ));
        } else {
            out.extend(plan_accounts(
                host,
                &prepared.config,
                server,
                &state,
                !shadow,
            )?);
        }
    }
    for name in gone(host, &prepared.config)? {
        out.push(view(
            &format!("container {name}"),
            Action::Remove,
            "it is not in the list; its data folder stays".into(),
            false,
        ));
    }
    Ok(out)
}

/// what the accounts of a running server would change.
fn plan_accounts(
    host: &dyn Host,
    config: &Config,
    server: &Server,
    state: &State,
    known: bool,
) -> Result<Vec<ChangeView>> {
    let mut out = Vec::new();
    let have = accounts(host, server)?;
    let needs = needs(config, server, &have, state, known);
    let at = |what: String| format!("{what} ({})", server.id);
    if needs.admin {
        out.push(view(
            &at("account Administrator".into()),
            Action::Update,
            "the domain administrator's password will be set".into(),
            true,
        ));
    }
    for machine in needs
        .machines
        .iter()
        .filter(|m| !have.contains(&format!("{m}$")))
    {
        out.push(view(
            &at(format!("machine {machine}")),
            Action::Create,
            format!("the account for {machine} will be created"),
            false,
        ));
    }
    for &index in &needs.users {
        let user = &config.users[index];
        let (action, what) = if have.contains(&user.name) {
            (Action::Update, "its password will be set")
        } else {
            (Action::Create, "it will be created")
        };
        out.push(view(
            &at(format!("user {}", user.name)),
            action,
            what.to_string(),
            true,
        ));
    }
    for name in &needs.deletes {
        out.push(view(
            &at(format!("user {name}")),
            Action::Remove,
            "it will be removed".into(),
            false,
        ));
    }
    if server.domain() {
        let script = File::new(
            &format!("{}/lib/netlogon/lanlord.bat", server.dir),
            logon_script(server),
            0o644,
        );
        let seen = script.check(host)?;
        if let Some(change) = script.diff(&seen, &[]) {
            out.push(change.view);
        }
    }
    Ok(out)
}

/// record the passwords in the settings as the ones that are set, without setting any: for the
/// accounts that exist, so a first native apply keeps a password changed from Windows. returns
/// what was recorded, by name only.
pub fn record(host: &dyn Host, doc: &Value, state_dir: &str) -> Result<Vec<String>> {
    let Some(config) = config(host, doc)? else {
        return Ok(Vec::new());
    };
    let mut state = State::load(host, state_dir, true)?;
    let mut recorded = Vec::new();
    for server in &config.servers {
        let have = accounts(host, server)?;
        if let Some(admin) = config.admin.as_ref().filter(|_| server.domain()) {
            if have.contains("root") {
                state.remember(host, &server.key("admin"), admin)?;
                recorded.push(server.key("admin"));
            }
        }
        for user in config.users.iter().filter(|u| have.contains(&u.name)) {
            let key = server.key(&format!("user:{}", user.name));
            state.remember(host, &key, &user.password)?;
            recorded.push(key);
        }
    }
    Ok(recorded)
}

/// one server as the parity collector sees it: where it lives and the passwords it should hold.
pub struct Target {
    pub id: String,
    pub container: String,
    pub dir: String,
    pub domain: bool,
    /// fingerprint key and the password the settings give it
    pub passwords: Vec<(String, Secret)>,
}

/// the servers of the list, for the collector. none when the settings have no list.
pub fn targets(host: &dyn Host, doc: &Value) -> Result<Option<Vec<Target>>> {
    let Some(config) = config(host, doc)? else {
        return Ok(None);
    };
    Ok(Some(
        config
            .servers
            .iter()
            .map(|s| {
                let mut passwords = Vec::new();
                if let Some(admin) = config.admin.as_ref().filter(|_| s.domain()) {
                    passwords.push((s.key("admin"), admin.clone()));
                }
                for u in &config.users {
                    passwords.push((s.key(&format!("user:{}", u.name)), u.password.clone()));
                }
                Target {
                    id: s.id.clone(),
                    container: s.container.clone(),
                    dir: s.dir.clone(),
                    domain: s.domain(),
                    passwords,
                }
            })
            .collect(),
    ))
}

/// make the module's machines. `run` names the journals of this run.
pub fn apply(
    host: &dyn Host,
    doc: &Value,
    state_dir: &str,
    run: &str,
    events: &mut Events,
) -> Result<Tally> {
    let mut tally = Tally::default();
    let Some(prepared) = prepare(host, doc)? else {
        return Ok(tally);
    };
    let config = &prepared.config;
    let secrets = config.secrets();
    let state = State::load(host, state_dir, true)?;
    if state.is_fresh() && !secrets.is_empty() {
        events(ApplyEvent::Log {
            text: FIRST_APPLY.into(),
        });
    }
    let mut journal = Journal::new(format!("{state_dir}/journal/{run}"));
    let seen = engine::look(host, &prepared.common)?;
    engine::change(
        host,
        &prepared.common,
        &seen,
        &mut journal,
        events,
        &mut tally,
        &secrets,
    )?;
    journal.finish(host);

    let mut job = Job {
        host,
        config,
        secrets,
        events,
        tally,
        state,
        state_dir,
        run,
    };
    for (server, resources) in config.servers.iter().zip(&prepared.servers) {
        job.serve(server, resources)?;
    }
    job.remove_gone()?;
    job.clean_images()?;
    Ok(job.tally)
}

/// what the steps share: the machine, the settings, and where the events and the count of what
/// was done go.
struct Job<'a, 'e> {
    host: &'a dyn Host,
    config: &'a Config,
    secrets: Vec<Secret>,
    events: &'e mut Events<'e>,
    tally: Tally,
    state: State,
    state_dir: &'a str,
    run: &'a str,
}

impl Job<'_, '_> {
    /// one server: its files and container, then its accounts.
    fn serve(&mut self, server: &Server, resources: &[Box<dyn Resource>]) -> Result<()> {
        let host = self.host;
        let seen = engine::look(host, resources)?;
        let mut journal = Journal::new(format!(
            "{}/journal/{}-{}",
            self.state_dir, self.run, server.id
        ));
        engine::change(
            host,
            resources,
            &seen,
            &mut journal,
            &mut *self.events,
            &mut self.tally,
            &self.secrets,
        )?;
        journal.finish(host);
        // the steps below: the wait, the join right of a domain, and the listing
        let n = 2 + u32::from(server.domain());
        (self.events)(ApplyEvent::Total { n });
        self.wait_for_samba(server)?;
        if server.domain() {
            self.domain_admins_may_join(server)?;
        }
        self.accounts(server)?;
        if server.domain() {
            self.logon_script(server)?;
        }
        self.list(server)
    }

    /// the containers of servers that left the list; their data folders stay.
    fn remove_gone(&mut self) -> Result<()> {
        let (host, config) = (self.host, self.config);
        engine::task(
            &mut *self.events,
            &mut self.tally,
            &self.secrets,
            "remove the containers of servers that are gone",
            |_| {
                let names = gone(host, config)?;
                for name in &names {
                    let out = docker(host, &["rm", "-f", name])?;
                    if !out.success() {
                        anyhow::bail!("docker rm {name} failed: {}", out.stderr.trim());
                    }
                }
                Ok((!names.is_empty(), names.join(", ")))
            },
        )
    }

    fn clean_images(&mut self) -> Result<()> {
        let host = self.host;
        engine::task(
            &mut *self.events,
            &mut self.tally,
            &self.secrets,
            "remove the old hash-tagged images",
            |_| {
                let listed = docker(
                    host,
                    &["image", "ls", "--format", "{{.Repository}}:{{.Tag}}"],
                )?;
                for img in listed.stdout.lines().filter(|l| {
                    l.contains("/lanlord-samba-nt4:") || l.starts_with("lanlord-samba-nt4:")
                }) {
                    let used = docker(
                        host,
                        &["ps", "-a", "--filter", &format!("ancestor={img}"), "-q"],
                    )?;
                    if used.stdout.trim().is_empty() {
                        let _ = docker(host, &["image", "rm", img]);
                    }
                }
                Ok((false, String::new()))
            },
        )
    }

    fn wait_for_samba(&mut self, server: &Server) -> Result<()> {
        let host = self.host;
        engine::task(
            &mut *self.events,
            &mut self.tally,
            &self.secrets,
            &format!("wait for samba to answer in {}", server.container),
            |_| {
                for attempt in 0..20 {
                    let ping = ["exec", &server.container, "smbcontrol", "smbd", "ping"];
                    if docker(host, &ping)?.success() {
                        return Ok((false, String::new()));
                    }
                    if attempt < 19 {
                        host.sleep(Duration::from_secs(2));
                    }
                }
                anyhow::bail!("samba does not answer in {}", server.container)
            },
        )
    }

    fn domain_admins_may_join(&mut self, server: &Server) -> Result<()> {
        let host = self.host;
        engine::task(
            &mut *self.events,
            &mut self.tally,
            &self.secrets,
            &format!("domain admins may join computers ({})", server.id),
            |_| {
                let right = "SeMachineAccountPrivilege";
                let name = &server.container;
                let have = docker(host, &["exec", name, "net", "sam", "rights", "list", right])?;
                if have.stdout.contains("Domain Admins") {
                    return Ok((false, String::new()));
                }
                let who = format!("{}\\Domain Admins", server.name);
                let given = docker(
                    host,
                    &["exec", name, "net", "sam", "rights", "grant", &who, right],
                )?;
                if !given.success() {
                    anyhow::bail!(
                        "could not let Domain Admins join computers: {}",
                        given.stderr.trim()
                    );
                }
                Ok((true, "granted".into()))
            },
        )
    }

    /// the accounts: the administrator, the machines, the users, and the removals.
    fn accounts(&mut self, server: &Server) -> Result<()> {
        let host = self.host;
        let mut have = BTreeSet::new();
        engine::task(
            &mut *self.events,
            &mut self.tally,
            &self.secrets,
            &format!("read the accounts of {}", server.container),
            |_| {
                have = accounts(host, server)?;
                Ok((false, String::new()))
            },
        )?;
        let todo = needs(self.config, server, &have, &self.state, true);
        // the steps below: each account change, the ensure, the sync of a domain, and the listing
        let changes = u32::from(todo.admin)
            + (todo.machines.len() + todo.users.len() + todo.deletes.len()) as u32;
        (self.events)(ApplyEvent::Total {
            n: changes + u32::from(!todo.ensure.is_empty()) + u32::from(server.domain()) + 1,
        });
        if todo.admin {
            self.set_admin_password(server)?;
        }
        for machine in &todo.machines {
            self.add_machine(server, machine)?;
        }
        for &index in &todo.users {
            self.set_up_user(server, index)?;
        }
        if !todo.ensure.is_empty() {
            self.ensure_users(server, &todo.ensure)?;
        }
        for gone in &todo.deletes {
            self.remove_user(server, gone)?;
        }
        Ok(())
    }

    fn set_admin_password(&mut self, server: &Server) -> Result<()> {
        let host = self.host;
        let Some(password) = &self.config.admin else {
            return Ok(());
        };
        let state = &mut self.state;
        engine::task(
            &mut *self.events,
            &mut self.tally,
            &self.secrets,
            &format!("set the domain administrator's password ({})", server.id),
            |_| {
                let argv = [
                    "docker",
                    "exec",
                    "-i",
                    &server.container,
                    "lanlord-user",
                    "admin",
                ];
                let out = host.run(&argv, Some(format!("{}\n", password.expose()).as_bytes()))?;
                if !out.success() {
                    anyhow::bail!("lanlord-user admin failed: {}", out.stderr.trim());
                }
                state.remember(host, &server.key("admin"), password)?;
                Ok((true, String::new()))
            },
        )
    }

    fn add_machine(&mut self, server: &Server, machine: &str) -> Result<()> {
        let host = self.host;
        engine::task(
            &mut *self.events,
            &mut self.tally,
            &self.secrets,
            &format!("create the account for {machine} ({})", server.id),
            |_| {
                let out = docker(
                    host,
                    &[
                        "exec",
                        &server.container,
                        "lanlord-user",
                        "machine",
                        machine,
                    ],
                )?;
                if !out.success() {
                    anyhow::bail!("lanlord-user machine failed: {}", out.stderr.trim());
                }
                Ok((out.stdout.contains("created"), String::new()))
            },
        )
    }

    fn set_up_user(&mut self, server: &Server, index: usize) -> Result<()> {
        let host = self.host;
        let user = &self.config.users[index];
        let state = &mut self.state;
        engine::task(
            &mut *self.events,
            &mut self.tally,
            &self.secrets,
            &format!("set up user {} ({})", user.name, server.id),
            |_| {
                let role = if user.admin { "admin" } else { "user" };
                let mut argv = vec![
                    "docker",
                    "exec",
                    "-i",
                    &server.container,
                    "lanlord-user",
                    "add",
                    &user.name,
                    role,
                ];
                if user.software {
                    argv.push("software");
                }
                let out = host.run(
                    &argv,
                    Some(format!("{}\n", user.password.expose()).as_bytes()),
                )?;
                if !out.success() {
                    anyhow::bail!(
                        "lanlord-user add {} failed: {}",
                        user.name,
                        out.stderr.trim()
                    );
                }
                let key = server.key(&format!("user:{}", user.name));
                state.remember(host, &key, &user.password)?;
                Ok((out.stdout.contains("created"), String::new()))
            },
        )
    }

    /// the users that keep their password: their unix account, groups, home folder and logon
    /// wrapper are checked, as a new container starts without them.
    fn ensure_users(&mut self, server: &Server, names: &[String]) -> Result<()> {
        let host = self.host;
        engine::task(
            &mut *self.events,
            &mut self.tally,
            &self.secrets,
            &format!("check the unix side of the users ({})", server.id),
            |_| {
                let mut argv = vec!["exec", &server.container, "lanlord-user", "ensure"];
                argv.extend(names.iter().map(String::as_str));
                let out = docker(host, &argv)?;
                if !out.success() {
                    anyhow::bail!("lanlord-user ensure failed: {}", out.stderr.trim());
                }
                Ok((false, String::new()))
            },
        )
    }

    fn remove_user(&mut self, server: &Server, gone: &str) -> Result<()> {
        let host = self.host;
        let state = &mut self.state;
        engine::task(
            &mut *self.events,
            &mut self.tally,
            &self.secrets,
            &format!("remove user {gone} ({})", server.id),
            |_| {
                let out = docker(
                    host,
                    &["exec", &server.container, "lanlord-user", "delete", gone],
                )?;
                if !out.success() {
                    anyhow::bail!("lanlord-user delete {gone} failed: {}", out.stderr.trim());
                }
                state.forget(host, &server.key(&format!("user:{gone}")))?;
                Ok((true, String::new()))
            },
        )
    }

    /// the logon script file, then the sync that hands it to the users.
    fn logon_script(&mut self, server: &Server) -> Result<()> {
        let host = self.host;
        let script: Vec<Box<dyn Resource>> = vec![Box::new(File::new(
            &format!("{}/lib/netlogon/lanlord.bat", server.dir),
            logon_script(server),
            0o644,
        ))];
        let seen = engine::look(host, &script)?;
        let mut after = Journal::new(format!(
            "{}/journal/{}-{}-logon",
            self.state_dir, self.run, server.id
        ));
        engine::change(
            host,
            &script,
            &seen,
            &mut after,
            &mut *self.events,
            &mut self.tally,
            &self.secrets,
        )?;
        after.finish(host);
        engine::task(
            &mut *self.events,
            &mut self.tally,
            &self.secrets,
            &format!("sync the logon scripts and groups ({})", server.id),
            |_| {
                let out = docker(host, &["exec", &server.container, "lanlord-user", "sync"])?;
                if !out.success() {
                    anyhow::bail!("lanlord-user sync failed: {}", out.stderr.trim());
                }
                Ok((false, String::new()))
            },
        )
    }

    fn list(&mut self, server: &Server) -> Result<()> {
        let host = self.host;
        engine::task(
            &mut *self.events,
            &mut self.tally,
            &self.secrets,
            &format!("list the accounts of {}", server.container),
            |events| {
                let names: Vec<String> = accounts(host, server)?.into_iter().collect();
                let kind = if server.domain() {
                    "domain controller"
                } else {
                    "workgroup server"
                };
                events(ApplyEvent::Log {
                    text: format!(
                        "{} is running as a {kind} named {} on {}; accounts: {}",
                        server.name,
                        server.netbios,
                        server.address.split('/').next().unwrap_or(""),
                        names.join(", ")
                    ),
                });
                Ok((false, String::new()))
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::host::{FakeHost, Output};
    use sha2::Digest;

    const STATE: &str = "/opt/lanlord/nt4";

    const SETTINGS: &str = "nt4-servers:\n  - name: nt4-domain\n    role: pdc\n    domain: retronet\n    netbios_name: lanlord\n    address: 192.0.2.1/24\ndomains:\n  RETRONET:\n    machines: [ws1]\nnt4-domain:\n  admin_password: s3cretadmin\n  users:\n    - name: dana\n      password: s3cretdana\n      admin: true\n";

    /// the legacy domain server and a workgroup server that does not serve wins.
    const TWO: &str = "nt4-servers:\n  - name: nt4-domain\n    role: pdc\n    domain: retronet\n    netbios_name: lanlord\n    address: 192.0.2.1/24\n  - name: lab\n    role: workgroup\n    workgroup: home\n    netbios_name: labbox\n    address: 198.51.100.1/24\n    wins: off\ndomains:\n  RETRONET:\n    machines: [ws1]\nnt4-domain:\n  admin_password: s3cretadmin\n  users:\n    - name: dana\n      password: s3cretdana\n";

    fn doc(text: &str) -> Value {
        serde_yaml_ng::from_str(text).unwrap()
    }

    fn healthy(host: &FakeHost) {
        host.when("docker image inspect", vec![Output::ok("sha256:img\n")]);
        let want = format!("true sha256:img {}\n", scripts_hash());
        host.when(
            "docker inspect --format {{.State.Running}} {{.Image}}",
            vec![Output::ok(&want)],
        );
        host.when(
            "docker exec lanlord-nt4 net sam rights list",
            vec![Output::ok("RETRONET\\Domain Admins\n")],
        );
        host.when(
            "docker exec lanlord-nt4 lanlord-user list",
            vec![Output::ok("root:0:Administrator\ndana:1000:\nws1$:1001:\n")],
        );
        host.when(
            "docker exec lanlord-nt4-lab lanlord-user list",
            vec![Output::ok("dana:1000:\n")],
        );
    }

    /// what a finished apply of `text` leaves behind.
    fn settle(host: &FakeHost, text: &str) {
        let p = prepare(host, &doc(text)).unwrap().unwrap();
        host.put(
            &format!("{STATE}/bin/entrypoint.sh"),
            ENTRYPOINT.as_bytes(),
            0o755,
        );
        host.put(
            &format!("{STATE}/bin/lanlord-user"),
            LANLORD_USER.as_bytes(),
            0o755,
        );
        host.put(
            &format!("{STATE}/bin/lanlord-exec"),
            LANLORD_EXEC.as_bytes(),
            0o755,
        );
        for dir in [STATE.to_string(), format!("{STATE}/bin")] {
            host.mkdir(&dir, 0o750).unwrap();
        }
        for server in &p.config.servers {
            host.put(
                &format!("{}/config/smb.conf", server.dir),
                render_conf(server).unwrap().as_bytes(),
                0o640,
            );
            for (dir, mode) in [("", 0o750), ("/config", 0o750), ("/lib", 0o751)] {
                host.mkdir(&format!("{}{dir}", server.dir), mode).unwrap();
            }
            if server.domain() {
                host.put(
                    &format!("{}/lib/netlogon/lanlord.bat", server.dir),
                    logon_script(server).as_bytes(),
                    0o644,
                );
            }
        }
    }

    fn settled(host: &FakeHost) {
        settle(host, SETTINGS);
    }

    fn settled_host() -> FakeHost {
        let host = FakeHost::new();
        healthy(&host);
        settled(&host);
        host
    }

    /// every password of the settings recorded as set, for `ids` (the server names).
    fn recorded(host: &FakeHost, ids: &[&str]) {
        let mut state = State::load(host, "/s", true).unwrap();
        for id in ids {
            state
                .remember(host, &format!("{id}/admin"), &Secret::new("s3cretadmin"))
                .unwrap();
            state
                .remember(host, &format!("{id}/user:dana"), &Secret::new("s3cretdana"))
                .unwrap();
        }
    }

    /// how many times exactly this container was removed.
    fn removed(host: &FakeHost, name: &str) -> usize {
        let line = format!("docker rm -f {name}");
        host.log().iter().filter(|l| **l == line).count()
    }

    /// a machine with no containers, where a new one comes up steady.
    fn bare(host: &FakeHost) {
        healthy(host);
        host.when(
            "docker inspect --format {{.State.Running}} {{.Image}}",
            vec![Output::failed(1, "no such")],
        );
        host.when(
            "docker inspect --format {{.State.Running}} lanlord",
            vec![Output::failed(1, "no such")],
        );
        host.when(
            "docker inspect --format {{.State.Running}} {{.State.Restarting}}",
            vec![Output::ok("true false 0\n")],
        );
    }

    fn server_of(text: &str) -> Server {
        let host = FakeHost::new();
        config(&host, &doc(text))
            .unwrap()
            .unwrap()
            .servers
            .remove(0)
    }

    #[test]
    fn the_scripts_hash_is_what_the_playbook_computes() {
        // `cat entrypoint.sh lanlord-user lanlord-exec | sha256sum | cut -c1-12`
        let mut sha = Sha256::new();
        sha.update(
            std::fs::read("../../ansible/playbooks/nt4-domain/image/entrypoint.sh").unwrap(),
        );
        sha.update(std::fs::read("../../ansible/playbooks/nt4-domain/image/lanlord-user").unwrap());
        sha.update(std::fs::read("../../ansible/playbooks/nt4-domain/image/lanlord-exec").unwrap());
        assert_eq!(scripts_hash(), hex(&sha.finalize())[..12]);
    }

    #[test]
    fn the_configuration_is_what_ansible_renders() {
        // the golden file is the playbook's own render of these settings
        let text = "nt4-servers:\n  - name: nt4-domain\n    role: pdc\n    domain: RETRODOM\n    netbios_name: LANLORD\n    address: 192.0.2.1/24\n    software: true\ndomains:\n  RETRODOM:\n    roaming_profiles: true\nnt4-domain:\n  admin_password: x\n  log_level: 2\n";
        assert_eq!(
            render_conf(&server_of(text)).unwrap(),
            include_str!("../../../tests/fixtures/nt4-smb-single.conf")
        );
    }

    #[test]
    fn netlogon_and_software_allow_execute_without_the_unix_x_bit() {
        let text = "nt4-servers:\n  - name: nt4-domain\n    role: pdc\n    domain: RETRODOM\n    netbios_name: LANLORD\n    address: 192.0.2.1/24\n    software: true\ndomains:\n  RETRODOM: {}\nnt4-domain:\n  admin_password: x\n";
        let conf = render_conf(&server_of(text)).unwrap();
        for share in ["[netlogon]", "[software]"] {
            let body = conf
                .split(share)
                .nth(1)
                .unwrap()
                .split("\n[")
                .next()
                .unwrap();
            assert!(
                body.contains("   acl allow execute always = yes\n"),
                "{share}"
            );
        }
    }

    #[test]
    fn the_configuration_keeps_the_add_machine_hook_and_wins_follows_the_server() {
        let conf = render_conf(&server_of(SETTINGS)).unwrap();
        assert!(conf.contains("   workgroup = RETRONET\n"));
        assert!(conf.contains("   netbios name = LANLORD\n"));
        assert!(conf.contains("   interfaces = 192.0.2.1/24\n"));
        assert!(conf
            .contains("   add machine script = /usr/local/bin/lanlord-user hook machine '%u'\n"));
        assert!(conf.contains("   domain logons = yes\n"));
        assert!(conf.contains("   log level = 1\n"));
        assert!(conf.contains("   wins support = yes\n"));
        assert!(conf.ends_with('\n'));
        let off = SETTINGS.replace(
            "address: 192.0.2.1/24\n",
            "address: 192.0.2.1/24\n    wins: off\n",
        );
        assert!(render_conf(&server_of(&off))
            .unwrap()
            .contains("   wins support = no\n"));
        let workgroup = "nt4-servers:\n  - name: box\n    role: workgroup\n    workgroup: home\n    netbios_name: box\n    address: 192.0.2.1/24\n";
        let conf = render_conf(&server_of(workgroup)).unwrap();
        assert!(
            conf.contains("server role = standalone server")
                && !conf.contains("add machine script")
        );
    }

    #[test]
    fn the_entrypoint_grants_every_right_the_collector_lists() {
        let loop_start = ENTRYPOINT
            .find("for right in")
            .expect("a loop over the rights");
        let rights = &ENTRYPOINT[loop_start..];
        let rights = &rights[..rights.find("; do").unwrap()];
        assert!(rights.contains("SeMachineAccountPrivilege"));
        assert!(ENTRYPOINT.contains("Domain Admins"));
    }

    #[test]
    fn no_list_means_nothing_to_do_and_an_empty_one_only_tidies() {
        let host = FakeHost::new();
        assert!(plan(&host, &doc("dns: {}\n"), "/s", false)
            .unwrap()
            .is_empty());
        let mut events = Vec::new();
        apply(&host, &doc("dns: {}\n"), "/s", "r1", &mut |e| {
            events.push(e)
        })
        .unwrap();
        assert!(host.log().is_empty() && host.text(&format!("{STATE}/bin/lanlord-user")).is_none());
        // a list with nothing in it: the folders and scripts, and the containers that are gone
        host.when(
            "docker ps -a --filter name=^lanlord-nt4",
            vec![Output::ok("lanlord-nt4\nlanlord-nt4-lab-prev\nother\n")],
        );
        apply(&host, &doc("nt4-servers: []\n"), "/s", "r2", &mut |_| {}).unwrap();
        assert_eq!(host.ran("docker pull"), 0);
        assert_eq!(removed(&host, "lanlord-nt4"), 1);
        assert_eq!(removed(&host, "lanlord-nt4-lab-prev"), 1);
        assert_eq!(removed(&host, "other"), 0);
        assert_eq!(
            host.text(&format!("{STATE}/bin/lanlord-user")).as_deref(),
            Some(LANLORD_USER)
        );
    }

    #[test]
    fn settings_the_playbook_would_refuse_fall_back() {
        let host = FakeHost::new();
        let server = |extra: &str| {
            format!(
                "nt4-servers:\n  - name: nt4-domain\n    role: pdc\n    domain: R\n    address: 192.0.2.1/24\n{extra}"
            )
        };
        for bad in [
            // a domain without an administrator password
            server(""),
            // a role that is neither
            "nt4-servers:\n  - name: a\n    role: nope\n    domain: R\n    address: 192.0.2.1/24\n".to_string(),
            // no address and no networks to take it from
            "nt4-servers:\n  - name: a\n    role: workgroup\n    workgroup: R\n".to_string(),
            // names the playbook refuses
            "nt4-servers:\n  - name: bin\n    role: workgroup\n    workgroup: R\n    address: 192.0.2.1/24\n".to_string(),
            "nt4-servers:\n  - name: a-prev\n    role: workgroup\n    workgroup: R\n    address: 192.0.2.1/24\n".to_string(),
            "nt4-servers:\n  - name: a\n    role: workgroup\n    workgroup: TOOLONGNAMEFORNETBIOS\n    address: 192.0.2.1/24\n".to_string(),
            "nt4-servers:\n  - name: a\n    role: workgroup\n    workgroup: R\n    address: nope\n".to_string(),
            // the same name twice, and settings that are not what they should be
            "nt4-servers:\n  - {name: a, role: workgroup, workgroup: R, address: 192.0.2.1/24}\n  - {name: a, role: workgroup, workgroup: S, address: 192.0.2.2/24}\n".to_string(),
            "nt4-servers: nope\n".to_string(),
            "nt4-servers: [nope]\n".to_string(),
            "nt4-servers: []\nnt4-domain: nope\n".to_string(),
            "nt4-servers: []\nnt4-domain:\n  users: nope\n".to_string(),
            "nt4-servers: []\nnt4-domain:\n  users: [{password: x}]\n".to_string(),
            "nt4-servers: []\nnt4-domain:\n  users: [{name: a}]\n".to_string(),
            "nt4-servers:\n  - {name: a, role: workgroup, workgroup: R, address: 192.0.2.1/24}\ndomains: {R: nope}\n".to_string(),
            "nt4-servers:\n  - {name: a, role: workgroup, workgroup: R, address: 192.0.2.1/24}\ndomains: {R: {machines: nope}}\n".to_string(),
            "nt4-servers:\n  - {name: a, role: workgroup, workgroup: R, address: 192.0.2.1/24}\ndomains: {R: {machines: [[1]]}}\n".to_string(),
            "nt4-servers:\n  - {name: a, role: workgroup, workgroup: R, address: 192.0.2.1/24, software: maybe}\n".to_string(),
        ] {
            let error = plan(&host, &doc(&bad), "/s", false)
                .err()
                .unwrap_or_else(|| panic!("accepted: {bad}"));
            assert!(error.downcast_ref::<Unsupported>().is_some(), "{bad}");
        }
    }

    #[test]
    fn a_server_takes_its_address_from_its_network() {
        let text = "networks:\n  - {name: lab, address: 192.0.2.1/24}\n  - {name: other, address: 198.51.100.1/24}\nnt4-servers:\n  - {name: a, role: workgroup, workgroup: R, network: Other}\n  - {name: b, role: workgroup, workgroup: S}\n";
        let host = FakeHost::new();
        host.when("hostname", vec![Output::ok("gw\n")]);
        let servers = config(&host, &doc(text)).unwrap().unwrap().servers;
        assert_eq!(servers[0].address, "198.51.100.1/24");
        assert_eq!(servers[1].address, "192.0.2.1/24");
        // and the netbios name is this machine's host name, cut and upper case
        host.when(
            "hostname",
            vec![Output::ok("gateway-with-a-long-name.example\n")],
        );
        let servers = config(&host, &doc(text)).unwrap().unwrap().servers;
        assert_eq!(servers[0].netbios, "GATEWAY-WITH-A-");
    }

    #[test]
    fn an_identity_change_and_a_missing_image_fall_back() {
        let host = FakeHost::new();
        healthy(&host);
        host.put(
            &format!("{STATE}/config/smb.conf"),
            b"[global]\n   workgroup = OTHER\n   netbios name = LANLORD\n",
            0o640,
        );
        let error = plan(&host, &doc(SETTINGS), "/s", false).unwrap_err();
        assert!(error.downcast_ref::<Unsupported>().is_some());
        // the same for the second server, in its own folder
        let host = FakeHost::new();
        healthy(&host);
        host.put(
            &format!("{STATE}/lab/config/smb.conf"),
            b"[global]\n   workgroup = HOME\n   netbios name = LABBOX\n   domain logons = yes\n",
            0o640,
        );
        assert!(plan(&host, &doc(TWO), "/s", false)
            .unwrap_err()
            .downcast_ref::<Unsupported>()
            .is_some());
        let host = FakeHost::new();
        host.when("docker pull", vec![Output::failed(1, "denied")]);
        assert!(plan(&host, &doc(SETTINGS), "/s", false)
            .unwrap_err()
            .downcast_ref::<Unsupported>()
            .is_some());
        let host = FakeHost::new();
        host.when("docker image inspect", vec![Output::failed(1, "none")]);
        assert!(plan(&host, &doc(SETTINGS), "/s", false)
            .unwrap_err()
            .downcast_ref::<Unsupported>()
            .is_some());
        let host = FakeHost::new();
        host.when("docker info", vec![Output::failed(1, "down")]);
        assert!(plan(&host, &doc(SETTINGS), "/s", false)
            .unwrap_err()
            .downcast_ref::<Unsupported>()
            .is_some());
    }

    #[test]
    fn a_fresh_machine_plans_everything_and_an_up_to_date_one_plans_nothing_to_replace() {
        let host = FakeHost::new();
        healthy(&host);
        host.when(
            "docker inspect --format {{.State.Running}} {{.Image}}",
            vec![Output::failed(1, "no such")],
        );
        let fresh = plan(&host, &doc(SETTINGS), "/s", false).unwrap();
        assert!(fresh
            .iter()
            .any(|c| c.resource == format!("file {STATE}/config/smb.conf")));
        assert!(fresh
            .iter()
            .any(|c| c.resource == "container lanlord-nt4" && c.action == Action::Create));
        assert!(fresh
            .iter()
            .any(|c| c.resource == "accounts of lanlord-nt4"));

        let host = settled_host();
        recorded(&host, &["nt4-domain"]);
        assert!(plan(&host, &doc(SETTINGS), "/s", false).unwrap().is_empty());
    }

    #[test]
    fn each_server_has_its_own_container_folder_and_configuration() {
        let host = FakeHost::new();
        bare(&host);
        let fresh = plan(&host, &doc(TWO), "/s", false).unwrap();
        for resource in [
            "container lanlord-nt4",
            "container lanlord-nt4-lab",
            &format!("file {STATE}/config/smb.conf"),
            &format!("file {STATE}/lab/config/smb.conf"),
            &format!("dir {STATE}/lab"),
        ] {
            assert!(fresh.iter().any(|c| c.resource == *resource), "{resource}");
        }
        let mut events = Vec::new();
        apply(&host, &doc(TWO), "/s", "r1", &mut |e| events.push(e)).unwrap();
        let run = host
            .log()
            .into_iter()
            .find(|l| l.starts_with("docker run -d --name lanlord-nt4-lab "))
            .unwrap();
        assert!(
            run.contains(&format!("-v {STATE}/lab/lib:/var/lib/samba")),
            "{run}"
        );
        assert!(run.contains(&format!(
            "-v {STATE}/bin/lanlord-user:/usr/local/bin/lanlord-user:ro"
        )));
        assert!(run.contains(&format!(
            "-v {STATE}/bin/lanlord-exec:/usr/local/bin/lanlord-exec:ro"
        )));
        assert!(run.contains(&format!(
            "-v {STATE}/lab/config/smb.conf:/etc/samba/smb.conf:ro"
        )));
        assert!(run.contains(&format!("--label lanlord.scripts={}", scripts_hash())));
        assert!(host
            .text(&format!("{STATE}/lab/config/smb.conf"))
            .unwrap()
            .contains("wins support = no"));
        assert!(host
            .text(&format!("{STATE}/config/smb.conf"))
            .unwrap()
            .contains("wins support = yes"));
        // the workgroup server has no administrator, machines or logon script
        assert_eq!(
            host.ran("docker exec -i lanlord-nt4-lab lanlord-user admin"),
            0
        );
        assert_eq!(
            host.ran("docker exec lanlord-nt4-lab lanlord-user machine"),
            0
        );
        assert!(host
            .text(&format!("{STATE}/lab/lib/netlogon/lanlord.bat"))
            .is_none());
        // a password is kept per server
        let state = host.text("/s/state.json").unwrap();
        assert!(state.contains("nt4-domain/admin") && state.contains("lab/user:dana"));
        assert!(!state.contains("lab/admin"));
        let shown = format!("{events:?}");
        assert!(
            shown.contains("HOME is running as a workgroup server named LABBOX on 198.51.100.1"),
            "{shown}"
        );
    }

    #[test]
    fn a_failure_in_one_server_stops_before_the_next() {
        let host = FakeHost::new();
        healthy(&host);
        settle(&host, TWO);
        host.when(
            "docker exec lanlord-nt4 smbcontrol smbd ping",
            vec![Output::failed(1, "no")],
        );
        let error = apply(&host, &doc(TWO), "/s", "r1", &mut |_| {})
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("samba does not answer in lanlord-nt4"),
            "{error}"
        );
        assert_eq!(host.ran("docker exec lanlord-nt4-lab"), 0);
    }

    #[test]
    fn servers_that_left_the_list_lose_their_containers_and_never_their_data() {
        let host = settled_host();
        recorded(&host, &["nt4-domain"]);
        host.when(
            "docker ps -a --filter name=^lanlord-nt4",
            vec![Output::ok(
                "lanlord-nt4\nlanlord-nt4-old\nlanlord-nt4-old-prev\nlanlord-nt4-lab\nlanlord-nt4-\nlanlord-nt4x\nlanlord-ntp\n",
            )],
        );
        let shown = plan(&host, &doc(SETTINGS), "/s", false).unwrap();
        assert_eq!(
            shown
                .iter()
                .filter(|c| c.action == Action::Remove)
                .map(|c| c.resource.as_str())
                .collect::<Vec<_>>(),
            [
                "container lanlord-nt4-old",
                "container lanlord-nt4-old-prev",
                "container lanlord-nt4-lab"
            ]
        );
        host.put(&format!("{STATE}/old/lib/keep"), b"data", 0o600);
        let mut events = Vec::new();
        let done = apply(&host, &doc(SETTINGS), "/s", "r1", &mut |e| events.push(e)).unwrap();
        for gone in ["lanlord-nt4-old", "lanlord-nt4-old-prev", "lanlord-nt4-lab"] {
            assert_eq!(removed(&host, gone), 1, "{gone}");
        }
        for kept in ["lanlord-nt4", "lanlord-nt4x", "lanlord-ntp", "lanlord-nt4-"] {
            assert_eq!(removed(&host, kept), 0, "{kept}");
        }
        assert!(host.text(&format!("{STATE}/old/lib/keep")).is_some());
        assert!(done.changed >= 1);
        host.when("docker rm -f", vec![Output::failed(1, "busy")]);
        let error = apply(&host, &doc(SETTINGS), "/s", "r2", &mut |_| {})
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("docker rm lanlord-nt4-old failed: busy"),
            "{error}"
        );
        host.when(
            "docker ps -a --filter name=^lanlord-nt4",
            vec![Output::failed(1, "down")],
        );
        assert!(plan(&host, &doc(SETTINGS), "/s", false)
            .unwrap_err()
            .to_string()
            .contains("docker ps failed"));
    }

    #[test]
    fn a_password_changed_from_windows_is_not_set_back_but_one_changed_in_lanlord_is() {
        let host = settled_host();
        recorded(&host, &["nt4-domain"]);
        let mut events = Vec::new();
        apply(&host, &doc(SETTINGS), "/s", "r1", &mut |e| events.push(e)).unwrap();
        assert!(host.inputs.borrow().is_empty(), "no password was sent");
        // the same settings but dana's password changed in lanlord
        let changed = SETTINGS.replace("s3cretdana", "newdana1");
        apply(&host, &doc(&changed), "/s", "r2", &mut |e| events.push(e)).unwrap();
        let inputs = host.inputs.borrow().clone();
        assert_eq!(inputs.len(), 1);
        assert!(inputs[0].0.contains("lanlord-user add dana admin"));
        assert_eq!(inputs[0].1, b"newdana1\n");
        // and it is not sent again
        apply(&host, &doc(&changed), "/s", "r3", &mut |e| events.push(e)).unwrap();
        assert_eq!(host.inputs.borrow().len(), 1);
    }

    #[test]
    fn a_first_apply_warns_sets_the_passwords_and_no_secret_reaches_an_event_or_an_error() {
        let host = settled_host();
        let mut events = Vec::new();
        apply(&host, &doc(SETTINGS), "/s", "r1", &mut |e| events.push(e)).unwrap();
        assert_eq!(host.inputs.borrow().len(), 2);
        let shown = format!("{events:?}");
        assert!(
            shown.contains("engine record --module nt4-domain"),
            "{shown}"
        );
        for secret in ["s3cretadmin", "s3cretdana"] {
            assert!(!shown.contains(secret));
        }
        assert!(!host.text("/s/state.json").unwrap().contains("s3cretdana"));
        // the second apply has recorded them and says nothing
        let mut again = Vec::new();
        apply(&host, &doc(SETTINGS), "/s", "r2", &mut |e| again.push(e)).unwrap();
        assert!(!format!("{again:?}").contains("engine record"));
        // a failing step names no password either
        let host = settled_host();
        host.when(
            "docker exec -i lanlord-nt4 lanlord-user admin",
            vec![Output::failed(1, "bad s3cretadmin")],
        );
        let mut events = Vec::new();
        let error = apply(&host, &doc(SETTINGS), "/s", "r1", &mut |e| events.push(e))
            .unwrap_err()
            .to_string();
        assert!(
            !error.contains("s3cretadmin") && !format!("{events:?}").contains("s3cretadmin"),
            "{error}"
        );
    }

    #[test]
    fn recording_keeps_the_passwords_as_they_are_and_a_later_apply_sets_none() {
        let host = settled_host();
        let kept = record(&host, &doc(SETTINGS), "/s").unwrap();
        assert_eq!(kept, ["nt4-domain/admin", "nt4-domain/user:dana"]);
        assert!(host.inputs.borrow().is_empty(), "nothing was set");
        assert!(!host.text("/s/state.json").unwrap().contains("s3cret"));
        let mut events = Vec::new();
        apply(&host, &doc(SETTINGS), "/s", "r1", &mut |e| events.push(e)).unwrap();
        assert!(host.inputs.borrow().is_empty(), "no password was sent");
        assert!(!format!("{events:?}").contains("engine record"));
        // nothing to record without a list, and an account that is not there is not recorded
        assert!(record(&host, &doc("dns: {}\n"), "/s").unwrap().is_empty());
        host.when(
            "docker exec lanlord-nt4 lanlord-user list",
            vec![Output::ok("ws1$:1:\n")],
        );
        assert!(record(&host, &doc(SETTINGS), "/s").unwrap().is_empty());
        host.when(
            "docker exec lanlord-nt4 lanlord-user list",
            vec![Output::failed(1, "down")],
        );
        assert!(record(&host, &doc(SETTINGS), "/s").is_err());
    }

    #[test]
    fn after_the_playbook_a_plan_reports_missing_accounts_but_not_passwords() {
        let host = settled_host();
        // nothing is recorded: the playbook set the passwords and kept no fingerprint
        let after = plan(&host, &doc(SETTINGS), "/s", true).unwrap();
        assert!(after.is_empty(), "{after:?}");
        let before = plan(&host, &doc(SETTINGS), "/s", false).unwrap();
        assert!(before
            .iter()
            .any(|c| c.resource.starts_with("account Administrator")));
        assert!(before.iter().any(|c| c.resource.starts_with("user dana")));
        // a missing account is still reported
        host.when(
            "docker exec lanlord-nt4 lanlord-user list",
            vec![Output::ok("root:0:\nws1$:1:\n")],
        );
        let missing = plan(&host, &doc(SETTINGS), "/s", true).unwrap();
        assert!(missing
            .iter()
            .any(|c| c.resource.starts_with("user dana") && c.action == Action::Create));
        assert!(!missing
            .iter()
            .any(|c| c.resource.starts_with("account Administrator")));
    }

    #[test]
    fn a_plan_lists_the_machines_and_removals_and_a_running_list_that_fails_is_an_error() {
        let text = format!("{SETTINGS}    - name: olduser\n      state: absent\n");
        let host = settled_host();
        host.when(
            "docker exec lanlord-nt4 lanlord-user list",
            vec![Output::ok("root:0:\ndana:1000:\nolduser:1001:\n")],
        );
        let shown = plan(&host, &doc(&text), "/s", true).unwrap();
        assert!(shown
            .iter()
            .any(|c| c.resource == "machine ws1 (nt4-domain)" && c.action == Action::Create));
        assert!(shown
            .iter()
            .any(|c| c.resource == "user olduser (nt4-domain)" && c.action == Action::Remove));
        let host = settled_host();
        host.when(
            "docker exec lanlord-nt4 lanlord-user list",
            vec![Output::failed(1, "no samba")],
        );
        let error = plan(&host, &doc(SETTINGS), "/s", false)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("lanlord-user list failed in lanlord-nt4: no samba"),
            "{error}"
        );
        // the logon script that differs is planned too
        let host = settled_host();
        host.put(&format!("{STATE}/lib/netlogon/lanlord.bat"), b"old", 0o644);
        recorded(&host, &["nt4-domain"]);
        let shown = plan(&host, &doc(SETTINGS), "/s", false).unwrap();
        assert_eq!(shown.len(), 1, "{shown:?}");
    }

    #[test]
    fn a_failed_account_listing_fails_the_step_and_changes_no_account() {
        let host = settled_host();
        host.when(
            "docker exec lanlord-nt4 lanlord-user list",
            vec![Output::failed(1, "pdbedit is down")],
        );
        let mut events = Vec::new();
        let error = failure_of(&host, &mut events);
        assert!(
            error.contains("lanlord-user list failed in lanlord-nt4: pdbedit is down"),
            "{error}"
        );
        assert!(host.inputs.borrow().is_empty(), "no account was touched");
        assert_eq!(host.ran("docker exec lanlord-nt4 lanlord-user machine"), 0);
        // the listing at the end fails the step too
        let host = settled_host();
        host.when(
            "docker exec lanlord-nt4 lanlord-user list",
            vec![
                Output::ok("root:0:\ndana:1000:\nws1$:1:\n"),
                Output::failed(1, "gone"),
            ],
        );
        recorded(&host, &["nt4-domain"]);
        let error = failure_of(&host, &mut events);
        assert!(error.contains("lanlord-user list failed"), "{error}");
    }

    #[test]
    fn users_that_keep_their_password_still_get_their_unix_side_checked() {
        let host = settled_host();
        recorded(&host, &["nt4-domain"]);
        apply(&host, &doc(SETTINGS), "/s", "r1", &mut |_| {}).unwrap();
        assert!(host.inputs.borrow().is_empty());
        assert_eq!(
            host.ran("docker exec lanlord-nt4 lanlord-user ensure dana"),
            1
        );
        // a user whose password is being set is not ensured as well
        let changed = SETTINGS.replace("s3cretdana", "newdana1");
        let host = settled_host();
        recorded(&host, &["nt4-domain"]);
        apply(&host, &doc(&changed), "/s", "r1", &mut |_| {}).unwrap();
        assert_eq!(host.ran("docker exec lanlord-nt4 lanlord-user ensure"), 0);
        // and a failure is reported
        let host = settled_host();
        recorded(&host, &["nt4-domain"]);
        host.when(
            "docker exec lanlord-nt4 lanlord-user ensure",
            vec![Output::failed(1, "no home")],
        );
        let mut events = Vec::new();
        let error = failure_of(&host, &mut events);
        assert!(
            error.contains("lanlord-user ensure failed: no home"),
            "{error}"
        );
    }

    #[test]
    fn a_missing_join_right_is_granted_to_domain_admins() {
        let host = settled_host();
        host.when(
            "docker exec lanlord-nt4 net sam rights list",
            vec![Output::ok("BUILTIN\\Administrators\n")],
        );
        apply(&host, &doc(SETTINGS), "/s", "r1", &mut |_| {}).unwrap();
        assert_eq!(host.ran("docker exec lanlord-nt4 net sam rights grant RETRONET\\Domain Admins SeMachineAccountPrivilege"), 1);
    }

    #[test]
    fn users_marked_absent_are_removed_only_when_they_exist() {
        let text = format!("{SETTINGS}    - name: olduser\n      state: absent\n");
        let host = settled_host();
        apply(&host, &doc(&text), "/s", "r1", &mut |_| {}).unwrap();
        assert_eq!(host.ran("docker exec lanlord-nt4 lanlord-user delete"), 0);
        host.when(
            "docker exec lanlord-nt4 lanlord-user list",
            vec![Output::ok(
                "root:0:\ndana:1000:\nolduser:1001:\nws1$:1002:\n",
            )],
        );
        recorded(&host, &["nt4-domain"]);
        host.inputs.borrow_mut().clear();
        // the fingerprint of the removed user goes with it
        State::load(&host, "/s", true)
            .unwrap()
            .remember(&host, "nt4-domain/user:olduser", &Secret::new("x"))
            .unwrap();
        apply(&host, &doc(&text), "/s", "r2", &mut |_| {}).unwrap();
        assert_eq!(
            host.ran("docker exec lanlord-nt4 lanlord-user delete olduser"),
            1
        );
        assert!(!host.text("/s/state.json").unwrap().contains("olduser"));
    }

    #[test]
    fn the_logon_script_has_dos_line_endings_and_the_optional_lines() {
        let text = "nt4-servers:\n  - {name: nt4-domain, role: pdc, domain: R, netbios_name: srv, address: 192.0.2.1/24, software: true}\nnt4-domain:\n  admin_password: x\n";
        assert_eq!(
            logon_script(&server_of(text)),
            "@echo off\r\nnet use S: \\\\SRV\\software /persistent:no\r\n\r\nif exist \\\\SRV\\netlogon\\logon.bat call \\\\SRV\\netlogon\\logon.bat\r\nif exist \\\\SRV\\netlogon\\%USERNAME%.bat call \\\\SRV\\netlogon\\%USERNAME%.bat\r\n"
        );
        let roaming = "nt4-servers:\n  - {name: nt4-domain, role: pdc, domain: R, netbios_name: srv, address: 192.0.2.1/24}\ndomains: {r: {roaming_profiles: true}}\nnt4-domain:\n  admin_password: x\n";
        assert!(logon_script(&server_of(roaming)).contains("net use H: /home"));
    }

    fn failure_of(host: &FakeHost, events: &mut Vec<ApplyEvent>) -> String {
        apply(host, &doc(SETTINGS), "/s", "r1", &mut |e| events.push(e))
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn samba_that_never_answers_stops_the_apply_after_waiting() {
        let host = settled_host();
        host.when(
            "docker exec lanlord-nt4 smbcontrol smbd ping",
            vec![Output::failed(1, "no")],
        );
        let mut events = Vec::new();
        let error = failure_of(&host, &mut events);
        assert!(error.contains("samba does not answer"), "{error}");
        assert_eq!(host.ran("docker exec lanlord-nt4 smbcontrol smbd ping"), 20);
        assert_eq!(*host.slept.borrow(), Duration::from_secs(38));
    }

    #[test]
    fn a_refused_join_right_names_the_reason() {
        let host = settled_host();
        host.when(
            "docker exec lanlord-nt4 net sam rights list",
            vec![Output::ok("")],
        );
        host.when(
            "docker exec lanlord-nt4 net sam rights grant",
            vec![Output::failed(1, "denied")],
        );
        let mut events = Vec::new();
        let error = failure_of(&host, &mut events);
        assert!(
            error.contains("could not let Domain Admins join computers: denied"),
            "{error}"
        );
    }

    #[test]
    fn a_machine_that_is_missing_is_created_and_a_failure_is_reported() {
        let host = settled_host();
        host.when(
            "docker exec lanlord-nt4 lanlord-user list",
            vec![Output::ok("root:0:\ndana:1000:\n")],
        );
        host.when(
            "docker exec lanlord-nt4 lanlord-user machine ws1",
            vec![Output::ok("created\n")],
        );
        let mut tally = Vec::new();
        let done = apply(&host, &doc(SETTINGS), "/s", "r1", &mut |e| tally.push(e)).unwrap();
        assert!(done.changed >= 1);
        assert_eq!(
            host.ran("docker exec lanlord-nt4 lanlord-user machine ws1"),
            1
        );
        host.when(
            "docker exec lanlord-nt4 lanlord-user machine ws1",
            vec![Output::failed(1, "no room")],
        );
        let mut events = Vec::new();
        let error = failure_of(&host, &mut events);
        assert!(
            error.contains("lanlord-user machine failed: no room"),
            "{error}"
        );
    }

    #[test]
    fn a_failing_user_change_is_reported_and_does_not_record_the_password() {
        let host = settled_host();
        host.when(
            "docker exec -i lanlord-nt4 lanlord-user add dana",
            vec![Output::failed(1, "weak password")],
        );
        let mut events = Vec::new();
        let error = failure_of(&host, &mut events);
        assert!(
            error.contains("lanlord-user add dana failed: weak password"),
            "{error}"
        );
        assert!(!host
            .text("/s/state.json")
            .unwrap_or_default()
            .contains("user:dana"));
    }

    #[test]
    fn a_failing_removal_and_a_failing_sync_are_reported() {
        let text = format!("{SETTINGS}    - name: olduser\n      state: absent\n");
        let host = settled_host();
        host.when(
            "docker exec lanlord-nt4 lanlord-user list",
            vec![Output::ok(
                "root:0:\ndana:1000:\nolduser:1001:\nws1$:1002:\n",
            )],
        );
        host.when(
            "docker exec lanlord-nt4 lanlord-user delete",
            vec![Output::failed(1, "busy")],
        );
        let error = apply(&host, &doc(&text), "/s", "r1", &mut |_| {})
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("lanlord-user delete olduser failed: busy"),
            "{error}"
        );
        let host = settled_host();
        host.when(
            "docker exec lanlord-nt4 lanlord-user sync",
            vec![Output::failed(1, "no sync")],
        );
        let mut events = Vec::new();
        let error = failure_of(&host, &mut events);
        assert!(
            error.contains("lanlord-user sync failed: no sync"),
            "{error}"
        );
    }

    #[test]
    fn a_failing_admin_password_names_the_command() {
        let host = settled_host();
        host.when(
            "docker exec -i lanlord-nt4 lanlord-user admin",
            vec![Output::failed(1, "locked")],
        );
        let mut events = Vec::new();
        let error = failure_of(&host, &mut events);
        assert!(
            error.contains("lanlord-user admin failed: locked"),
            "{error}"
        );
    }

    #[test]
    fn old_images_nobody_uses_are_removed_and_the_accounts_are_listed() {
        let host = settled_host();
        host.when(
            "docker image ls",
            vec![Output::ok(
                "lanlord-samba-nt4:old\nother:1\nlanlord-samba-nt4:busy\n",
            )],
        );
        host.when(
            "docker ps -a --filter ancestor=lanlord-samba-nt4:old",
            vec![Output::ok("")],
        );
        host.when(
            "docker ps -a --filter ancestor=lanlord-samba-nt4:busy",
            vec![Output::ok("abc\n")],
        );
        let mut events = Vec::new();
        apply(&host, &doc(SETTINGS), "/s", "r1", &mut |e| events.push(e)).unwrap();
        assert_eq!(host.ran("docker image rm lanlord-samba-nt4:old"), 1);
        assert_eq!(host.ran("docker image rm lanlord-samba-nt4:busy"), 0);
        let shown = format!("{events:?}");
        assert!(
            shown.contains("domain controller named LANLORD on 192.0.2.1"),
            "{shown}"
        );
        assert!(shown.contains("accounts: dana, root, ws1$"), "{shown}");
    }
}
