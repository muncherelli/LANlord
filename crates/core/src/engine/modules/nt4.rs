//! the nt4-domain module, natively: the same machine the playbook (`nt4-domain.yml`) makes, with
//! the same files, the same container and the same `lanlord-user` calls. what it will not do it
//! says is `Unsupported` before it changes anything, and the playbook does it: a change of the
//! domain's identity (which snapshots samba's state first), an image that has to be built here,
//! and settings the playbook's own checks would refuse (so the refusal reads the same).
//!
//! two decisions that differ from running the playbook:
//! - a password is set only when it changed in LANlord (a fingerprint of it is kept, see
//!   `state.rs`), so a password a person changed from Windows is not set back by the next apply.
//! - the right to join computers (`SeMachineAccountPrivilege`) is checked for Domain Admins, so a
//!   personal Domain Admin can join a machine, not only Administrator. the `add machine script`
//!   hook in smb.conf is what creates the account when it is joined.

use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::Result;
use lanlord_model::apply::ApplyEvent;
use lanlord_model::engine::{Action, ChangeView};
use serde_yaml_ng::Value;
use sha2::{Digest, Sha256};

use crate::engine::host::Host;
use crate::engine::journal::Journal;
use crate::engine::resource::{Container, Dir, File, Resource, Unsupported};
use crate::engine::state::{hex, State};
use crate::engine::{self, Events, Secret, Tally};

pub const ID: &str = "nt4-domain";

const STATE: &str = "/opt/lanlord/nt4";
const NAME: &str = "lanlord-nt4";
const IMAGE: &str = "ghcr.io/muncherelli/lanlord-nt4-server-components";

const ENTRYPOINT: &str =
    include_str!("../../../../../ansible/playbooks/nt4-domain/image/entrypoint.sh");
const LANLORD_USER: &str =
    include_str!("../../../../../ansible/playbooks/nt4-domain/image/lanlord-user");
const SMB_CONF: &str = include_str!("../../../../../ansible/playbooks/nt4-domain/smb.conf.j2");

fn unsupported(why: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Unsupported(why.into()))
}

struct User {
    name: String,
    password: Secret,
    admin: bool,
    software: bool,
}

struct Config {
    section: Value,
    mode: String,
    name: String,
    netbios: String,
    address: String,
    roaming: bool,
    software: bool,
    machines: Vec<String>,
    admin: Option<Secret>,
    users: Vec<User>,
    absent: Vec<String>,
}

impl Config {
    fn domain(&self) -> bool {
        self.mode == "domain"
    }

    fn secrets(&self) -> Vec<Secret> {
        self.admin
            .iter()
            .cloned()
            .chain(self.users.iter().map(|u| u.password.clone()))
            .collect()
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

/// the section, checked as the playbook checks it. `None` when there is no section: the module
/// then does nothing.
fn config(host: &dyn Host, doc: &Value) -> Result<Option<Config>> {
    let Some(section) = doc.get(ID) else {
        return Ok(None);
    };
    let Some(map) = section.as_mapping() else {
        return Err(unsupported("the nt4-domain settings are not a mapping"));
    };
    let get = |key: &str| map.get(key);
    let mode = text(get("mode")).unwrap_or_default();
    let name = text(get("name")).unwrap_or_default().to_ascii_uppercase();
    let netbios = match text(get("netbios_name")) {
        Some(n) => n,
        None => {
            let out = host.run(&["hostname"], None)?;
            out.stdout
                .trim()
                .split('.')
                .next()
                .unwrap_or("")
                .to_string()
        }
    };
    let netbios: String = netbios.to_ascii_uppercase().chars().take(15).collect();
    let address = match text(get("address")) {
        Some(a) => a,
        None => network_address(doc, text(get("network")).unwrap_or_default().as_str())
            .ok_or_else(|| {
                unsupported("the server's address comes from the playbook's network rules")
            })?,
    };
    let admin = text(get("admin_password"))
        .filter(|s| !s.is_empty())
        .map(Secret::new);
    let machines: Vec<String> = match get("machines") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Sequence(items)) => items.iter().filter_map(|m| text(Some(m))).collect(),
        Some(_) => return Err(unsupported("nt4-domain machines is not a list")),
    };
    let machine_count = match get("machines") {
        Some(Value::Sequence(items)) => items.len(),
        _ => 0,
    };
    let mut users = Vec::new();
    let mut absent = Vec::new();
    match get("users") {
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
    let valid = (mode == "domain" || mode == "workgroup")
        && netbios_ok(&name)
        && netbios_ok(&netbios)
        && address_ok(&address)
        && (mode == "workgroup" || admin.is_some())
        && machines.len() == machine_count
        && machines
            .iter()
            .all(|m| lanlord_model::nt4::valid_machine(m));
    if !valid {
        return Err(unsupported(
            "the nt4-domain settings need the playbook's checks and messages",
        ));
    }
    Ok(Some(Config {
        section: section.clone(),
        mode,
        name,
        netbios,
        address,
        roaming: flag(get("roaming_profiles")),
        software: flag(get("software")),
        machines,
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

/// the version of the scripts: the first 12 hex digits of the sha256 of both, one after the other.
pub fn scripts_hash() -> String {
    let mut sha = Sha256::new();
    sha.update(ENTRYPOINT.as_bytes());
    sha.update(LANLORD_USER.as_bytes());
    hex(&sha.finalize())[..12].to_string()
}

/// the samba configuration, from the template the playbook uses.
fn render_conf(config: &Config) -> Result<String> {
    let mut env = minijinja::Environment::new();
    env.set_trim_blocks(true);
    env.set_keep_trailing_newline(true);
    env.add_template("smb.conf", SMB_CONF)?;
    let context = minijinja::context! {
        nt4 => minijinja::Value::from_serialize(&config.section),
        nt4_name => &config.name,
        nt4_netbios_name => &config.netbios,
        nt4_address => &config.address,
        nt4_mode => &config.mode,
    };
    Ok(env.get_template("smb.conf")?.render(context)?)
}

fn logon_script(config: &Config) -> String {
    let unc = format!("\\\\{}", config.netbios);
    let lines = [
        "@echo off".to_string(),
        if config.software {
            format!("net use S: {unc}\\software /persistent:no")
        } else {
            String::new()
        },
        if config.roaming {
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
    resources: Vec<Box<dyn Resource>>,
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
    let conf_path = format!("{STATE}/config/smb.conf");
    if let Some(old) = host.read(&conf_path)? {
        let old = String::from_utf8_lossy(&old);
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
        if word("workgroup = ") != config.name
            || word("netbios name = ") != config.netbios
            || logons != config.domain()
        {
            return Err(unsupported(
                "the domain identity changes, samba's state is snapshotted first",
            ));
        }
    }
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
    let conf = File {
        path: conf_path.clone(),
        content: render_conf(&config)?.into_bytes(),
        mode: 0o640,
        sensitive: false,
    };
    let conf_id = conf.id();
    let mut run = vec![
        "docker",
        "run",
        "-d",
        "--name",
        NAME,
        "--network",
        "host",
        "--restart",
        "unless-stopped",
        "--label",
    ]
    .into_iter()
    .map(String::from)
    .collect::<Vec<_>>();
    run.push(format!("lanlord.scripts={hash}"));
    for (from, to, mode) in [
        (
            format!("{STATE}/bin/entrypoint.sh"),
            "/usr/local/bin/entrypoint.sh",
            ":ro",
        ),
        (
            format!("{STATE}/bin/lanlord-user"),
            "/usr/local/bin/lanlord-user",
            ":ro",
        ),
        (format!("{STATE}/lib"), "/var/lib/samba", ""),
        (conf_path.clone(), "/etc/samba/smb.conf", ":ro"),
    ] {
        run.push("-v".into());
        run.push(format!("{from}:{to}{mode}"));
    }
    run.push(image);
    let mut container = Container::new(
        NAME,
        format!("true {} {hash}", id.stdout.trim()),
        "{{.State.Running}} {{.Image}} {{index .Config.Labels \"lanlord.scripts\"}}",
        run,
        ["docker", "exec", NAME, "smbcontrol", "smbd", "ping"]
            .map(String::from)
            .to_vec(),
    );
    container.retries = 20;
    container.follows = vec![conf_id];
    let resources: Vec<Box<dyn Resource>> = vec![
        Box::new(Dir::new(STATE, 0o750)),
        Box::new(Dir::new(&format!("{STATE}/config"), 0o750)),
        Box::new(Dir::new(&format!("{STATE}/bin"), 0o750)),
        Box::new(Dir::new(&format!("{STATE}/lib"), 0o751)),
        Box::new(conf),
        Box::new(File::new(
            &format!("{STATE}/bin/entrypoint.sh"),
            ENTRYPOINT,
            0o755,
        )),
        Box::new(File::new(
            &format!("{STATE}/bin/lanlord-user"),
            LANLORD_USER,
            0o755,
        )),
        Box::new(container),
    ];
    Ok(Some(Prepared { config, resources }))
}

/// the names of the accounts, from `pdbedit -L`; `None` when the container does not answer.
fn accounts(host: &dyn Host) -> Option<BTreeSet<String>> {
    let out = docker(host, &["exec", NAME, "lanlord-user", "list"]).ok()?;
    if !out.success() {
        return None;
    }
    Some(
        out.stdout
            .lines()
            .filter_map(|l| l.split(':').next())
            .filter(|n| !n.is_empty())
            .map(String::from)
            .collect(),
    )
}

/// what the accounts still need, given the accounts there are and what was set before.
#[derive(Default)]
struct Needs {
    admin: bool,
    machines: Vec<String>,
    /// indexes into `Config::users`
    users: Vec<usize>,
    deletes: Vec<String>,
}

fn needs(config: &Config, have: &BTreeSet<String>, state: &State) -> Needs {
    let mut needs = Needs::default();
    if config.domain() {
        if let Some(admin) = &config.admin {
            needs.admin = !have.contains("root") || !state.is_current("admin", admin);
        }
        needs.machines = config
            .machines
            .iter()
            .map(|m| m.to_ascii_lowercase())
            .filter(|m| !have.contains(&format!("{m}$")))
            .collect();
    }
    for (index, user) in config.users.iter().enumerate() {
        if !have.contains(&user.name)
            || !state.is_current(&format!("user:{}", user.name), &user.password)
        {
            needs.users.push(index);
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

/// what an apply would change. nothing is changed (an image is pulled, as the playbook does).
pub fn plan(host: &dyn Host, doc: &Value, state_dir: &str) -> Result<Vec<ChangeView>> {
    let Some(prepared) = prepare(host, doc)? else {
        return Ok(Vec::new());
    };
    let changes = engine::look(host, &prepared.resources)?;
    let mut out = engine::views(&changes);
    let config = &prepared.config;
    let replacing = changes.last().is_some_and(Option::is_some);
    if replacing {
        out.push(view(
            "accounts",
            Action::Run,
            "accounts and the logon script are checked once the container runs".into(),
            false,
        ));
        return Ok(out);
    }
    let state = State::load(host, state_dir, false)?;
    let Some(have) = accounts(host) else {
        return Ok(out);
    };
    let needs = needs(config, &have, &state);
    if needs.admin {
        out.push(view(
            "account Administrator",
            Action::Update,
            "the domain administrator's password will be set".into(),
            true,
        ));
    }
    for machine in &needs.machines {
        out.push(view(
            &format!("machine {machine}"),
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
            &format!("user {}", user.name),
            action,
            what.to_string(),
            true,
        ));
    }
    for name in &needs.deletes {
        out.push(view(
            &format!("user {name}"),
            Action::Remove,
            "it will be removed".into(),
            false,
        ));
    }
    if config.domain() {
        let script = File::new(
            &format!("{STATE}/lib/netlogon/lanlord.bat"),
            logon_script(config),
            0o644,
        );
        let seen = script.check(host)?;
        if let Some(change) = script.diff(&seen, &[]) {
            out.push(change.view);
        }
    }
    Ok(out)
}

/// make the module's machine. `run` names the journal of this run.
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
    let mut journal = Journal::new(format!("{state_dir}/journal/{run}"));

    let changes = engine::look(host, &prepared.resources)?;
    engine::change(
        host,
        &prepared.resources,
        &changes,
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
    };
    job.after_container();
    job.clean_images()?;
    job.wait_for_samba()?;
    if config.domain() {
        job.domain_admins_may_join()?;
    }
    job.accounts()?;
    if config.domain() {
        job.logon_script(state_dir, run)?;
    }
    job.list()?;
    Ok(job.tally)
}

/// what the steps after the container share: the machine, the settings, and where the events and
/// the count of what was done go.
struct Job<'a, 'e> {
    host: &'a dyn Host,
    config: &'a Config,
    secrets: Vec<Secret>,
    events: &'e mut Events<'e>,
    tally: Tally,
    state: State,
}

impl Job<'_, '_> {
    /// say how many steps the first part has.
    fn after_container(&mut self) {
        let n = 2 + u32::from(self.config.domain());
        (self.events)(ApplyEvent::Total { n });
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

    fn wait_for_samba(&mut self) -> Result<()> {
        let host = self.host;
        engine::task(
            &mut *self.events,
            &mut self.tally,
            &self.secrets,
            "wait for samba to answer",
            |_| {
                for attempt in 0..20 {
                    if docker(host, &["exec", NAME, "smbcontrol", "smbd", "ping"])?.success() {
                        return Ok((false, String::new()));
                    }
                    if attempt < 19 {
                        host.sleep(Duration::from_secs(2));
                    }
                }
                anyhow::bail!("samba does not answer")
            },
        )
    }

    fn domain_admins_may_join(&mut self) -> Result<()> {
        let (host, config) = (self.host, self.config);
        engine::task(
            &mut *self.events,
            &mut self.tally,
            &self.secrets,
            "domain admins may join computers",
            |_| {
                let right = "SeMachineAccountPrivilege";
                let have = docker(host, &["exec", NAME, "net", "sam", "rights", "list", right])?;
                if have.stdout.contains("Domain Admins") {
                    return Ok((false, String::new()));
                }
                let who = format!("{}\\Domain Admins", config.name);
                let given = docker(
                    host,
                    &["exec", NAME, "net", "sam", "rights", "grant", &who, right],
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
    fn accounts(&mut self) -> Result<()> {
        let have = accounts(self.host).unwrap_or_default();
        let todo = needs(self.config, &have, &self.state);
        // the steps below: each account change, the sync of a domain, and the listing
        let changes = u32::from(todo.admin)
            + (todo.machines.len() + todo.users.len() + todo.deletes.len()) as u32;
        (self.events)(ApplyEvent::Total {
            n: changes + u32::from(self.config.domain()) + 1,
        });
        if todo.admin {
            self.set_admin_password()?;
        }
        for machine in &todo.machines {
            self.add_machine(machine)?;
        }
        for &index in &todo.users {
            self.set_up_user(index)?;
        }
        for gone in &todo.deletes {
            self.remove_user(gone)?;
        }
        Ok(())
    }

    fn set_admin_password(&mut self) -> Result<()> {
        let host = self.host;
        let Some(password) = &self.config.admin else {
            return Ok(());
        };
        let state = &mut self.state;
        engine::task(
            &mut *self.events,
            &mut self.tally,
            &self.secrets,
            "set the domain administrator's password",
            |_| {
                let argv = ["docker", "exec", "-i", NAME, "lanlord-user", "admin"];
                let out = host.run(&argv, Some(format!("{}\n", password.expose()).as_bytes()))?;
                if !out.success() {
                    anyhow::bail!("lanlord-user admin failed: {}", out.stderr.trim());
                }
                state.remember(host, "admin", password)?;
                Ok((true, String::new()))
            },
        )
    }

    fn add_machine(&mut self, machine: &str) -> Result<()> {
        let host = self.host;
        engine::task(
            &mut *self.events,
            &mut self.tally,
            &self.secrets,
            &format!("create the account for {machine}"),
            |_| {
                let out = docker(host, &["exec", NAME, "lanlord-user", "machine", machine])?;
                if !out.success() {
                    anyhow::bail!("lanlord-user machine failed: {}", out.stderr.trim());
                }
                Ok((out.stdout.contains("created"), String::new()))
            },
        )
    }

    fn set_up_user(&mut self, index: usize) -> Result<()> {
        let host = self.host;
        let user = &self.config.users[index];
        let state = &mut self.state;
        engine::task(
            &mut *self.events,
            &mut self.tally,
            &self.secrets,
            &format!("set up user {}", user.name),
            |_| {
                let role = if user.admin { "admin" } else { "user" };
                let mut argv = vec![
                    "docker",
                    "exec",
                    "-i",
                    NAME,
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
                state.remember(host, &format!("user:{}", user.name), &user.password)?;
                Ok((out.stdout.contains("created"), String::new()))
            },
        )
    }

    fn remove_user(&mut self, gone: &str) -> Result<()> {
        let host = self.host;
        let state = &mut self.state;
        engine::task(
            &mut *self.events,
            &mut self.tally,
            &self.secrets,
            &format!("remove user {gone}"),
            |_| {
                let out = docker(host, &["exec", NAME, "lanlord-user", "delete", gone])?;
                if !out.success() {
                    anyhow::bail!("lanlord-user delete {gone} failed: {}", out.stderr.trim());
                }
                state.forget(host, &format!("user:{gone}"))?;
                Ok((true, String::new()))
            },
        )
    }

    /// the logon script file, then the sync that hands it to the users.
    fn logon_script(&mut self, state_dir: &str, run: &str) -> Result<()> {
        let host = self.host;
        let script: Vec<Box<dyn Resource>> = vec![Box::new(File::new(
            &format!("{STATE}/lib/netlogon/lanlord.bat"),
            logon_script(self.config),
            0o644,
        ))];
        let seen = engine::look(host, &script)?;
        let mut after = Journal::new(format!("{state_dir}/journal/{run}-logon"));
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
            "sync the logon scripts and groups",
            |_| {
                let out = docker(host, &["exec", NAME, "lanlord-user", "sync"])?;
                if !out.success() {
                    anyhow::bail!("lanlord-user sync failed: {}", out.stderr.trim());
                }
                Ok((false, String::new()))
            },
        )
    }

    fn list(&mut self) -> Result<()> {
        let (host, config) = (self.host, self.config);
        engine::task(
            &mut *self.events,
            &mut self.tally,
            &self.secrets,
            "list the accounts",
            |events| {
                if let Some(names) = accounts(host) {
                    let names: Vec<String> = names.into_iter().collect();
                    let kind = if config.domain() {
                        "domain controller"
                    } else {
                        "workgroup server"
                    };
                    events(ApplyEvent::Log {
                        text: format!(
                            "{} is running as a {kind} named {} on {}; accounts: {}",
                            config.name,
                            config.netbios,
                            config.address.split('/').next().unwrap_or(""),
                            names.join(", ")
                        ),
                    });
                }
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

    const SETTINGS: &str = "nt4-domain:\n  mode: domain\n  name: retronet\n  netbios_name: lanlord\n  address: 192.0.2.1/24\n  admin_password: s3cretadmin\n  machines: [ws1]\n  users:\n    - name: dana\n      password: s3cretdana\n      admin: true\n";

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
    }

    fn settled(host: &FakeHost) {
        // what a finished apply leaves behind
        let p = prepare(host, &doc(SETTINGS)).unwrap().unwrap();
        for r in &p.resources {
            let _ = r.id();
        }
        host.put(
            &format!("{STATE}/config/smb.conf"),
            render_conf(&p.config).unwrap().as_bytes(),
            0o640,
        );
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
        for (dir, mode) in [
            ("", 0o750),
            ("/config", 0o750),
            ("/bin", 0o750),
            ("/lib", 0o751),
        ] {
            host.mkdir(&format!("{STATE}{dir}"), mode).unwrap();
        }
        host.put(
            &format!("{STATE}/lib/netlogon/lanlord.bat"),
            logon_script(&p.config).as_bytes(),
            0o644,
        );
    }

    #[test]
    fn the_scripts_hash_is_what_the_playbook_computes() {
        // `cat entrypoint.sh lanlord-user | sha256sum | cut -c1-12`
        let mut sha = Sha256::new();
        sha.update(
            std::fs::read("../../ansible/playbooks/nt4-domain/image/entrypoint.sh").unwrap(),
        );
        sha.update(std::fs::read("../../ansible/playbooks/nt4-domain/image/lanlord-user").unwrap());
        assert_eq!(scripts_hash(), hex(&sha.finalize())[..12]);
    }

    #[test]
    fn the_configuration_keeps_the_add_machine_hook_and_the_join_rights() {
        let host = FakeHost::new();
        let config = config(&host, &doc(SETTINGS)).unwrap().unwrap();
        let conf = render_conf(&config).unwrap();
        assert!(conf.contains("   workgroup = RETRONET\n"));
        assert!(conf.contains("   netbios name = LANLORD\n"));
        assert!(conf.contains("   interfaces = 192.0.2.1/24\n"));
        assert!(conf
            .contains("   add machine script = /usr/local/bin/lanlord-user hook machine '%u'\n"));
        assert!(conf.contains("   domain logons = yes\n"));
        assert!(conf.contains("   log level = 1\n"));
        assert!(conf.ends_with('\n'));
        // domain admins hold the join right; the entrypoint grants it at start
        assert!(ENTRYPOINT.contains("SeMachineAccountPrivilege"));
        assert!(ENTRYPOINT.contains("Domain Admins"));
        let workgroup = doc("nt4-domain:\n  mode: workgroup\n  name: HOME\n  netbios_name: box\n  address: 192.0.2.1/24\n");
        let conf = render_conf(&self::config(&host, &workgroup).unwrap().unwrap()).unwrap();
        assert!(
            conf.contains("server role = standalone server")
                && !conf.contains("add machine script")
        );
    }

    #[test]
    fn no_section_means_nothing_to_do() {
        let host = FakeHost::new();
        assert!(plan(&host, &doc("dns: {}\n"), "/s").unwrap().is_empty());
    }

    #[test]
    fn settings_the_playbook_would_refuse_fall_back() {
        let host = FakeHost::new();
        for bad in [
            "nt4-domain:\n  mode: domain\n  name: R\n  address: 192.0.2.1/24\n",
            "nt4-domain:\n  mode: nope\n  name: R\n  address: 192.0.2.1/24\n",
            "nt4-domain:\n  mode: workgroup\n  name: R\n",
        ] {
            let error = plan(&host, &doc(bad), "/s").unwrap_err();
            assert!(error.downcast_ref::<Unsupported>().is_some(), "{bad}");
        }
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
        let error = plan(&host, &doc(SETTINGS), "/s").unwrap_err();
        assert!(error.downcast_ref::<Unsupported>().is_some());
        let host = FakeHost::new();
        host.when("docker pull", vec![Output::failed(1, "denied")]);
        assert!(plan(&host, &doc(SETTINGS), "/s")
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
        let fresh = plan(&host, &doc(SETTINGS), "/s").unwrap();
        assert!(fresh
            .iter()
            .any(|c| c.resource == format!("file {STATE}/config/smb.conf")));
        assert!(fresh
            .iter()
            .any(|c| c.resource == "container lanlord-nt4" && c.action == Action::Create));

        let host = FakeHost::new();
        healthy(&host);
        settled(&host);
        let mut state = State::load(&host, "/s", true).unwrap();
        state
            .remember(&host, "admin", &Secret::new("s3cretadmin"))
            .unwrap();
        state
            .remember(&host, "user:dana", &Secret::new("s3cretdana"))
            .unwrap();
        assert!(plan(&host, &doc(SETTINGS), "/s").unwrap().is_empty());
    }

    #[test]
    fn a_password_changed_from_windows_is_not_set_back_but_one_changed_in_lanlord_is() {
        let host = FakeHost::new();
        healthy(&host);
        settled(&host);
        let mut state = State::load(&host, "/s", true).unwrap();
        state
            .remember(&host, "admin", &Secret::new("s3cretadmin"))
            .unwrap();
        state
            .remember(&host, "user:dana", &Secret::new("s3cretdana"))
            .unwrap();
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
    fn a_first_apply_sets_the_passwords_and_no_secret_reaches_an_event_or_an_error() {
        let host = FakeHost::new();
        healthy(&host);
        settled(&host);
        let mut events = Vec::new();
        apply(&host, &doc(SETTINGS), "/s", "r1", &mut |e| events.push(e)).unwrap();
        assert_eq!(host.inputs.borrow().len(), 2);
        let shown = format!("{events:?}");
        for secret in ["s3cretadmin", "s3cretdana"] {
            assert!(!shown.contains(secret));
        }
        assert!(!host.text("/s/state.json").unwrap().contains("s3cretdana"));
        // a failing step names no password either
        let host = FakeHost::new();
        healthy(&host);
        settled(&host);
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
    fn a_missing_join_right_is_granted_to_domain_admins() {
        let host = FakeHost::new();
        healthy(&host);
        settled(&host);
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
        let host = FakeHost::new();
        healthy(&host);
        settled(&host);
        apply(&host, &doc(&text), "/s", "r1", &mut |_| {}).unwrap();
        assert_eq!(host.ran("docker exec lanlord-nt4 lanlord-user delete"), 0);
        host.when(
            "docker exec lanlord-nt4 lanlord-user list",
            vec![Output::ok(
                "root:0:\ndana:1000:\nolduser:1001:\nws1$:1002:\n",
            )],
        );
        apply(&host, &doc(&text), "/s", "r2", &mut |_| {}).unwrap();
        assert_eq!(
            host.ran("docker exec lanlord-nt4 lanlord-user delete olduser"),
            1
        );
    }

    #[test]
    fn the_logon_script_has_dos_line_endings_and_the_optional_lines() {
        let host = FakeHost::new();
        let text = "nt4-domain:\n  mode: domain\n  name: R\n  netbios_name: srv\n  address: 192.0.2.1/24\n  admin_password: x\n  software: true\n";
        let config = config(&host, &doc(text)).unwrap().unwrap();
        assert_eq!(
            logon_script(&config),
            "@echo off\r\nnet use S: \\\\SRV\\software /persistent:no\r\n\r\nif exist \\\\SRV\\netlogon\\logon.bat call \\\\SRV\\netlogon\\logon.bat\r\nif exist \\\\SRV\\netlogon\\%USERNAME%.bat call \\\\SRV\\netlogon\\%USERNAME%.bat\r\n"
        );
    }

    fn settled_host() -> FakeHost {
        let host = FakeHost::new();
        healthy(&host);
        settled(&host);
        host
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
