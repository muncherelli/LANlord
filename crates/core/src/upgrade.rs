//! upgrade an installed gateway in place: new server binary, login helper and browser build, the
//! current playbooks, and a restart of every lanlord service whose binary or unit changed.
//!
//! nothing is purged and nothing the admin set up is touched. the work is the installer's own:
//! the bootstrap (idempotent), then [`crate::webinstall`] stages the files, puts them in place
//! beside `lanlord.prev` and `web.prev`, and applies the web modules with the install's settings.
//! afterwards the services that run the new binary or a new unit are restarted and checked; when
//! they do not come back, the binary and the web folder are put back from the `.prev` copies.
//!
//! a server binary built without the `webui` feature is refused before anything is uploaded: the
//! service it would replace would crash-loop.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use lanlord_model::operations::{Operation, Reply};

use crate::bootstrap::{
    bootstrap_lines, capture_as_root, run_as_root, run_as_root_lines, AskPassword, Options,
};
use crate::install::{options_for, status_with, Status};
use crate::log::{emit, Kind};
use crate::ssh::Session;
use crate::stored;
use crate::webinstall::WebSource;
use crate::{account::login_user, webinstall};

/// the server binary the services run.
const BINARY: &str = "/opt/lanlord/bin/lanlord";

/// a lanlord service that runs the server binary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Service {
    pub name: &'static str,
    pub unit: &'static str,
}

/// the services that run the server binary, the web ui first.
pub const SERVICES: [Service; 2] = [
    Service {
        name: "lanlord-webui",
        unit: "/etc/systemd/system/lanlord-webui.service",
    },
    Service {
        name: "lanlord-operations",
        unit: "/etc/systemd/system/lanlord-operations.service",
    },
];

/// what a build without the web server prints (see `main.rs`), written backwards so that this
/// crate's own binary does not contain the text it looks for.
const NO_WEB_SERVER_REVERSED: &str = "revres bew on sah dliub siht";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Check,
    Bootstrap,
    Files,
    Apply,
    Restart,
    Verify,
    Migrate,
}

impl Step {
    pub const ALL: [Step; 7] = [
        Step::Check,
        Step::Bootstrap,
        Step::Files,
        Step::Apply,
        Step::Restart,
        Step::Verify,
        Step::Migrate,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Step::Check => "Checking the server",
            Step::Bootstrap => "Checking the gateway account",
            Step::Files => "Replacing the server binary and the browser build",
            Step::Apply => "Applying the web UI modules",
            Step::Restart => "Restarting the changed services",
            Step::Verify => "Checking that the services came back",
            Step::Migrate => "Bringing the stored files up to the new format",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    StepStarted(Step),
    Line(String),
    StepDone(Step),
    Failed { step: Step, message: String },
}

/// whether the binary at `path` contains the web server: it does unless it is a build without the
/// `webui` feature, which carries the message it dies with.
pub fn has_web_server(path: &Path) -> Result<bool> {
    let bytes =
        std::fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let needle: Vec<u8> = NO_WEB_SERVER_REVERSED.bytes().rev().collect();
    Ok(!bytes.windows(needle.len()).any(|window| window == needle))
}

/// refuse a local server binary built without the web server.
fn require_web_server(source: &WebSource) -> Result<()> {
    let WebSource::Local(local) = source else {
        return Ok(());
    };
    if !has_web_server(&local.server_binary)? {
        bail!(
            "{} was built without the webui feature, and its services would crash-loop; \
             build it with --features webui (tools/dev.sh linux-build does)",
            local.server_binary.display()
        );
    }
    Ok(())
}

/// hashes of the files whose change decides what is restarted; `None` for a missing file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot(BTreeMap<String, Option<String>>);

impl Snapshot {
    pub fn parse(text: &str) -> Self {
        Snapshot(
            text.lines()
                .filter_map(|line| line.split_once(' '))
                .map(|(hash, path)| {
                    let hash = (hash != "none").then(|| hash.to_string());
                    (path.trim().to_string(), hash)
                })
                .collect(),
        )
    }

    fn get(&self, path: &str) -> Option<&String> {
        self.0.get(path).and_then(Option::as_ref)
    }
}

/// the script that prints one `hash path` line per watched file, `none` for a missing one.
pub fn snapshot_script() -> String {
    let mut files = vec![BINARY.to_string()];
    files.extend(SERVICES.iter().map(|service| service.unit.to_string()));
    format!(
        "# upgrade snapshot\n\
         for file in {}; do\n\
         \x20 if [ -f \"$file\" ]; then echo \"$(sha256sum \"$file\" | cut -d' ' -f1) $file\"; else echo \"none $file\"; fi\n\
         done\n",
        files.join(" ")
    )
}

/// which services are restarted and which are checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub restart: Vec<Service>,
    pub check: Vec<Service>,
    /// enabled lanlord units besides [`SERVICES`] (the firewall, say), checked after the restart.
    pub extra_check: Vec<String>,
    /// the ones of them that are not active, started again whether or not a file changed.
    pub extra_start: Vec<String>,
}

impl Plan {
    /// a new binary restarts every service; a new unit restarts that service. every service whose
    /// unit exists is checked, restarted or not. a new unit is restarted too, though the playbook
    /// has usually started it already.
    pub fn new(before: &Snapshot, after: &Snapshot) -> Self {
        let binary_changed = before.get(BINARY) != after.get(BINARY);
        let check = existing(after);
        let restart = check
            .iter()
            .copied()
            .filter(|service| binary_changed || before.get(service.unit) != after.get(service.unit))
            .collect();
        Plan {
            restart,
            check,
            extra_check: Vec::new(),
            extra_start: Vec::new(),
        }
    }

    /// when the files after the upgrade cannot be read: restart every service, and check the
    /// ones that were there before.
    pub fn fallback(before: &Snapshot) -> Self {
        Plan {
            restart: SERVICES.to_vec(),
            check: existing(before),
            extra_check: Vec::new(),
            extra_start: Vec::new(),
        }
    }

    /// take in the enabled lanlord units found on the server: one that is not active is started
    /// again and every one of them is checked, so a stopped or failed unit does not stay down
    /// after an upgrade that did not touch its files.
    pub fn with_units(mut self, units: &[Unit]) -> Self {
        for unit in units {
            let core = SERVICES
                .iter()
                .find(|service| format!("{}.service", service.name) == unit.name);
            match core {
                Some(service) => {
                    let known = |list: &[Service]| list.contains(service);
                    if !unit.active && known(&self.check) && !known(&self.restart) {
                        self.restart.push(*service);
                    }
                }
                None => {
                    self.extra_check.push(unit.name.clone());
                    if !unit.active {
                        self.extra_start.push(unit.name.clone());
                    }
                }
            }
        }
        self
    }
}

/// an enabled lanlord unit and whether it is active.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    pub name: String,
    pub active: bool,
}

/// the script that prints `unit state` for every enabled lanlord service, socket and timer.
pub fn units_script() -> String {
    "# upgrade units\n\
     systemctl list-unit-files 'lanlord-*.service' 'lanlord-*.socket' 'lanlord-*.timer' --state=enabled --no-legend 2>/dev/null | while read -r unit _; do\n\
     \x20 echo \"$unit $(systemctl is-active \"$unit\" 2>/dev/null | head -n 1)\"\n\
     done\n"
        .to_string()
}

/// read the output of [`units_script`]; lines that are not a lanlord unit are skipped. the name
/// keeps its suffix.
pub fn parse_units(text: &str) -> Vec<Unit> {
    text.lines()
        .filter_map(|line| {
            let (unit, state) = line.split_once(' ')?;
            let named = [".service", ".socket", ".timer"]
                .iter()
                .any(|suffix| unit.ends_with(suffix));
            (named && unit.starts_with("lanlord-")).then(|| Unit {
                name: unit.to_string(),
                active: state.trim() == "active",
            })
        })
        .collect()
}

/// the script that prints the name of each of `units` that is still not active, after giving it
/// up to fifteen seconds; it always exits 0.
pub fn extras_script(units: &[String]) -> String {
    format!(
        "# upgrade extras\n\
         for unit in {}; do\n\
         \x20 tries=0\n\
         \x20 while [ \"$(systemctl is-active \"$unit\" 2>/dev/null | head -n 1)\" != active ] && [ \"$tries\" -lt 15 ]; do\n\
         \x20   tries=$((tries + 1)); sleep 1\n\
         \x20 done\n\
         \x20 [ \"$(systemctl is-active \"$unit\" 2>/dev/null | head -n 1)\" = active ] || echo \"$unit\"\n\
         done\n",
        units.join(" ")
    )
}

/// the services whose unit exists in `snapshot`.
fn existing(snapshot: &Snapshot) -> Vec<Service> {
    SERVICES
        .into_iter()
        .filter(|service| snapshot.get(service.unit).is_some())
        .collect()
}

fn names(services: &[Service]) -> String {
    names_with(services, &[])
}

/// the names of `services` and then of the `extra` units, space separated.
fn names_with(services: &[Service], extra: &[String]) -> String {
    services
        .iter()
        .map(|service| service.name)
        .chain(extra.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn restart_script(services: &[Service], extra: &[String]) -> String {
    format!(
        "# upgrade restart\nsystemctl restart {}\n",
        names_with(services, extra)
    )
}

/// the shell function `check`: every service active, and the web ui answering on its socket,
/// three times in a row a second apart, within thirty seconds. the socket is the one the
/// installed unit names, as tools/dev-deploy.sh reads it.
fn check_function(services: &[Service], extra: &[String]) -> String {
    let web = services
        .iter()
        .any(|service| service.name == SERVICES[0].name);
    let socket = if web {
        format!(
            "sock=$(systemctl cat {web} 2>/dev/null | sed -n 's/^ExecStart=.*--socket[= ]\\([^ ]*\\).*/\\1/p' | head -n 1)\n\
             case \"$sock\" in /*) ;; \"\") sock=/run/lanlord-web/web.sock ;; *) sock=\"/$sock\" ;; esac\n",
            web = SERVICES[0].name
        )
    } else {
        String::new()
    };
    let probe = if web {
        " && curl -fsS -m 3 --unix-socket \"$sock\" -o /dev/null http://localhost/ 2>/dev/null"
    } else {
        ""
    };
    format!(
        "{socket}check() {{\n\
         \x20 tries=0; good=0\n\
         \x20 while [ \"$tries\" -lt 30 ]; do\n\
         \x20   ok=1\n\
         \x20   for service in {names}; do\n\
         \x20     [ \"$(systemctl is-active \"$service\" 2>/dev/null)\" = active ] || ok=0\n\
         \x20   done\n\
         \x20   if [ \"$ok\" = 1 ]{probe}; then good=$((good + 1)); else good=0; fi\n\
         \x20   [ \"$good\" -ge 3 ] && return 0\n\
         \x20   tries=$((tries + 1))\n\
         \x20   sleep 1\n\
         \x20 done\n\
         \x20 for service in {names}; do echo \"$service is $(systemctl is-active \"$service\" 2>&1 | head -n 1)\" >&2; done\n\
         \x20 return 1\n\
         }}\n",
        names = names_with(services, extra)
    )
}

pub fn check_script(services: &[Service], extra: &[String]) -> String {
    format!(
        "# upgrade check\n{}check\n",
        check_function(services, extra)
    )
}

/// put the binary, the login helper and the web folder back from their `.prev` copies, restart
/// the services and check them. `root` is empty on a server; the tests point it at a folder.
/// with `guarded` nothing is done unless the swap had begun (`install_script` leaves a marker
/// until it has finished). exit 0: they answer again; 1: the previous version is back but they
/// still do not answer, or a move failed; 2: there was no previous version to put back; 3: the
/// swap had not begun, so nothing was moved.
pub fn rollback_script(root: &str, services: &[Service], guarded: bool) -> String {
    let guard = if guarded {
        "if [ ! -e \"$bin_dir/.placing\" ]; then echo \"the swap had not begun; nothing was moved\"; exit 3; fi\n"
    } else {
        ""
    };
    format!(
        "# upgrade rollback\n\
         bin_dir='{root}/opt/lanlord/bin'; web_dir='{root}/opt/lanlord/web'\n\
         {guard}found=0\n\
         for name in lanlord lanlord-auth-helper; do\n\
         \x20 if [ -f \"$bin_dir/$name.prev\" ]; then\n\
         \x20   if [ -f \"$bin_dir/$name\" ]; then mv -f \"$bin_dir/$name\" \"$bin_dir/$name.bad\" || exit 1; fi\n\
         \x20   mv -f \"$bin_dir/$name.prev\" \"$bin_dir/$name\" || exit 1\n\
         \x20   found=1\n\
         \x20 fi\n\
         done\n\
         if [ -d \"$web_dir.prev\" ]; then\n\
         \x20 if [ -d \"$web_dir\" ]; then rm -rf \"$web_dir.bad\"; mv \"$web_dir\" \"$web_dir.bad\" || exit 1; fi\n\
         \x20 mv \"$web_dir.prev\" \"$web_dir\" || exit 1\n\
         \x20 found=1\n\
         fi\n\
         rm -f \"$bin_dir/.placing\"\n\
         [ \"$found\" = 1 ] || exit 2\n\
         systemctl restart {names}\n\
         {function}check\n",
        names = names(services),
        function = check_function(services, &[])
    )
}

fn root_status(session: &Session, script: String, ask: AskPassword) -> Result<i32> {
    let admin = login_user(session)?;
    run_as_root(session, &admin.name, script, ask)
}

fn snapshot(session: &Session, ask: AskPassword) -> Result<Snapshot> {
    let admin = login_user(session)?;
    let text = capture_as_root(session, &admin.name, snapshot_script(), ask)?;
    Ok(Snapshot::parse(&text))
}

fn units(session: &Session, ask: AskPassword) -> Result<Vec<Unit>> {
    let admin = login_user(session)?;
    let text = capture_as_root(session, &admin.name, units_script(), ask)?;
    Ok(parse_units(&text))
}

/// run one step, reporting it; the error of a failed step is reported once, as `Failed`.
fn step<T>(
    events: &mut dyn FnMut(Event),
    step: Step,
    work: impl FnOnce(&mut dyn FnMut(&str)) -> Result<T>,
) -> Result<T> {
    events(Event::StepStarted(step));
    let result = work(&mut |text| events(Event::Line(text.to_string())));
    match result {
        Ok(value) => {
            events(Event::StepDone(step));
            Ok(value)
        }
        Err(error) => {
            let message = format!("{error:#}");
            events(Event::Failed {
                step,
                message: message.clone(),
            });
            bail!(message)
        }
    }
}

/// upgrade the lanlord on `host`; it must be installed. with `allow_unhealthy` the upgrade also
/// goes ahead when its services are already down before it starts (and then rolls nothing back).
pub fn run(
    host: &str,
    web: &WebSource,
    allow_unhealthy: bool,
    ask_password: AskPassword,
    events: &mut dyn FnMut(Event),
) -> Result<()> {
    run_with(
        &Session::new(host)?,
        &options_for(host),
        web,
        allow_unhealthy,
        ask_password,
        events,
        &mut |stamp, ask| {
            // the new service holds the key, so it migrates the files
            let operation = Operation::MigrateStored {
                stamp: stamp.to_string(),
            };
            match crate::operations::execute(host, &operation, ask)? {
                Reply::Lines(lines) => Ok(lines),
                _ => Ok(Vec::new()),
            }
        },
    )
}

/// how the stored files are migrated: given the stamp that names the backups, it answers with
/// what was done.
type Migrate<'a> = &'a mut dyn FnMut(&str, AskPassword) -> Result<Vec<String>>;

/// what the server was like before the upgrade.
struct Before {
    snapshot: Snapshot,
    /// whether its services were answering.
    healthy: bool,
}

/// refuse what cannot be upgraded, and read what the server runs now.
fn check(
    session: &Session,
    options: &Options,
    web: &WebSource,
    allow_unhealthy: bool,
    ask: AskPassword,
    on_line: &mut dyn FnMut(&str),
) -> Result<Before> {
    require_web_server(web)?;
    match status_with(session, &mut *ask)? {
        Status::Installed => {}
        Status::Partial(_) => bail!(
            "the install on {} is not finished; run `lanlord install` to finish it first",
            options.host
        ),
        Status::NotInstalled => bail!(
            "lanlord is not installed on {}; use `lanlord install`",
            options.host
        ),
    }
    on_line("lanlord is installed; nothing will be removed");
    let snapshot = snapshot(session, &mut *ask)?;
    let running = existing(&snapshot);
    let healthy = root_status(session, check_script(&running, &[]), ask)? == 0;
    if !healthy {
        if !allow_unhealthy {
            bail!(
                "{} not answering on {} before the upgrade, so a failure could not be blamed on it; \
                 look at the services first, or pass --even-if-down to upgrade anyway",
                names(&running),
                options.host
            );
        }
        on_line("the services are not answering before the upgrade; going ahead, with no rollback");
    }
    Ok(Before { snapshot, healthy })
}

/// why the migration failed, and the stamp its copies carry (when it got far enough to have one).
struct MigrateFailed {
    message: String,
    stamp: Option<String>,
}

/// ask the gateway for a stamp and have the stored files migrated under it.
fn migrate_stored(
    session: &Session,
    host: &str,
    ask: AskPassword,
    migrate: Migrate,
    on_line: &mut dyn FnMut(&str),
) -> std::result::Result<(), MigrateFailed> {
    let failed = |error: anyhow::Error, stamp: Option<String>| MigrateFailed {
        message: sealed_hint(&format!("{error:#}"), host),
        stamp,
    };
    let admin = login_user(session).map_err(|e| failed(e, None))?;
    let text = capture_as_root(session, &admin.name, stored::stamp_script(), &mut *ask)
        .map_err(|e| failed(e, None))?;
    let stamp = text.trim().to_string();
    if !lanlord_model::format::backup_stamp(&stamp) {
        return Err(failed(
            anyhow::anyhow!("the server's clock gave no usable stamp"),
            None,
        ));
    }
    let lines = migrate(&stamp, ask).map_err(|e| failed(e, Some(stamp.clone())))?;
    for line in lines {
        on_line(&line);
    }
    Ok(())
}

/// the command that moves the old client key to the gateway at `host` (given as `user@name` or
/// a bare name): it has to run from an administrator's own account, never root's.
pub fn move_command(host: &str) -> String {
    let name = host.rsplit_once('@').map_or(host, |(_, name)| name);
    format!("lanlord move-secrets <your-admin>@{name}")
}

/// the hint added when the migration finds the secrets sealed to another key.
fn sealed_hint(message: &str, host: &str) -> String {
    if message.contains("sealed to another key") {
        format!(
            "{message}; run `{}` and then upgrade again",
            move_command(host)
        )
    } else {
        message.to_string()
    }
}

const NOT_REVERTED: &str = "the units and settings that the upgrade applied were not reverted";

/// put the previous files back and say how that went.
fn roll_back(
    session: &Session,
    services: &[Service],
    guarded: bool,
    ask: AskPassword,
    on_line: &mut dyn FnMut(&str),
) -> Result<String> {
    on_line("putting the previous version back");
    let admin = login_user(session)?;
    let code = run_as_root_lines(
        session,
        &admin.name,
        rollback_script("", services, guarded),
        ask,
        on_line,
    )?;
    Ok(match code {
        0 => format!("the previous version is running again; {NOT_REVERTED}"),
        2 => "there is no previous version to put back".to_string(),
        3 => "nothing had been moved, so the previous version is still in place".to_string(),
        _ => format!(
            "the previous version is back, or a move failed, and the services still do not \
             answer; look at them on the server; {NOT_REVERTED}"
        ),
    })
}

pub(crate) fn run_with(
    session: &Session,
    options: &Options,
    web: &WebSource,
    allow_unhealthy: bool,
    ask_password: AskPassword,
    events: &mut dyn FnMut(Event),
    migrate: Migrate,
) -> Result<()> {
    let ask = ask_password;
    let before = step(events, Step::Check, |line| {
        check(session, options, web, allow_unhealthy, &mut *ask, line)
    })?;
    step(events, Step::Bootstrap, |line| {
        bootstrap_lines(session, options, &mut *ask, line)
    })?;
    // the stage leaves what was running alone; once the swap has begun, a failure in it puts the
    // previous files back (the script knows by a marker whether it had begun)
    step(events, Step::Files, |line| {
        webinstall::stage(session, web, &mut *ask, line)?;
        if let Err(error) = webinstall::place(session, &mut *ask, line) {
            let services = existing(&before.snapshot);
            let outcome = roll_back(session, &services, true, &mut *ask, line)?;
            bail!("{error:#}; {outcome}");
        }
        Ok(())
    })?;
    // a failed apply does not roll back by itself; what counts is whether the services answer
    let applied = step(events, Step::Apply, |line| {
        webinstall::converge(session, &options.host, &mut *ask, line)
    });
    let plan = step(events, Step::Restart, |line| {
        let plan = match snapshot(session, &mut *ask) {
            Ok(after) => Plan::new(&before.snapshot, &after),
            Err(error) => {
                line(&format!(
                    "could not read the files after the upgrade ({error:#}); restarting every service"
                ));
                Plan::fallback(&before.snapshot)
            }
        };
        // a unit that was stopped or failed before comes back too; not knowing them is not fatal
        let units = match units(session, &mut *ask) {
            Ok(units) => units,
            Err(error) => {
                line(&format!("could not list the lanlord units ({error:#})"));
                Vec::new()
            }
        };
        let plan = plan.with_units(&units);
        if plan.restart.is_empty() && plan.extra_start.is_empty() {
            line("no service runs a changed binary or unit");
        } else {
            line(&format!(
                "restarting {}",
                names_with(&plan.restart, &plan.extra_start)
            ));
            let code = root_status(
                session,
                restart_script(&plan.restart, &plan.extra_start),
                &mut *ask,
            )?;
            if code != 0 {
                line(&format!("the restart reported status {code}"));
            }
        }
        Ok(plan)
    })?;
    step(events, Step::Verify, |line| {
        if root_status(session, check_script(&plan.check, &[]), &mut *ask)? == 0 {
            line(&format!("{} answering", names(&plan.check)));
            // the other units do not decide whether the new binary is rolled back: the upgrade
            // stays, and what is still down is named
            if plan.extra_check.is_empty() {
                return Ok(());
            }
            let admin = login_user(session)?;
            let text = capture_as_root(
                session,
                &admin.name,
                extras_script(&plan.extra_check),
                &mut *ask,
            )?;
            let down: Vec<&str> = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect();
            if down.is_empty() {
                line(&format!("{} active", plan.extra_check.join(" ")));
                return Ok(());
            }
            bail!(
                "the upgrade is in place and {} answering, but {} still not active; nothing was \
                 rolled back; look at each with `systemctl status <unit>` or run `lanlord doctor`",
                names(&plan.check),
                down.join(", ")
            );
        }
        if !before.healthy {
            bail!(
                "the services still do not answer; they were down before the upgrade, \
                 so nothing was rolled back"
            );
        }
        line("the services did not come back");
        let outcome = roll_back(session, &plan.check, false, &mut *ask, line)?;
        bail!("the services did not come back after the upgrade; {outcome}")
    })?;
    // the new binary is answering: it migrates the stored files (a failed apply leaves them
    // as they are; the new binary reads the old formats all the same)
    if applied.is_ok() {
        step(events, Step::Migrate, |line| {
            let migrated = migrate_stored(session, &options.host, &mut *ask, migrate, line);
            let Err(error) = migrated else {
                return Ok(());
            };
            line("putting the old copies of the stored files back");
            let restored = match &error.stamp {
                Some(stamp) => root_status(session, stored::restore_script(stamp), &mut *ask),
                None => Ok(0),
            };
            let restored = match restored {
                Ok(0) => "the stored files are as they were".to_string(),
                _ => "the stored files could not be put back; their copies are beside them \
                      (settings.*.bak in /opt/lanlord/config)"
                    .to_string(),
            };
            let outcome = roll_back(session, &plan.check, false, &mut *ask, line)?;
            bail!("{}; {restored}; {outcome}", error.message)
        })?;
    }
    applied?;
    emit(
        Kind::Info,
        format!("lanlord on {} is upgraded", options.host),
    );
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use crate::bootstrap::AnsibleMode;
    use crate::bootstrap::Options;
    use crate::install::MARKER;
    use crate::ssh::Session;
    use crate::testgw::no_password;
    use crate::upgrade::{
        check_script, extras_script, has_web_server, parse_units, restart_script, rollback_script,
        run_with, snapshot_script, units_script, Event, Plan, Snapshot, Step, Unit, BINARY,
        NO_WEB_SERVER_REVERSED, SERVICES,
    };
    use crate::webinstall::{install_script, WebSource};

    const WEBUI: &str = SERVICES[0].unit;
    const OPERATIONS: &str = SERVICES[1].unit;

    fn snapshot(binary: &str, webui: &str, operations: &str) -> String {
        format!("{binary} {BINARY}\n{webui} {WEBUI}\n{operations} {OPERATIONS}\n")
    }

    /// a fake server: the marker and the snapshots are files in `dir`, and what the server does
    /// is logged there. the flag files `unhealthy` (down until rolled back), `down-before`,
    /// `still-broken`, `no-place-early`, `no-place` (fails after the move), `apply-fails` and
    /// `snapshot-fails` (after the swap) make it misbehave.
    struct Server {
        dir: std::path::PathBuf,
        session: Session,
        web: WebSource,
    }

    impl Server {
        fn new(name: &str, marker: Option<&str>) -> Self {
            let dir =
                std::env::temp_dir().join(format!("lanlord-upgrade-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("local/dist")).unwrap();
            std::fs::write(dir.join("local/lanlord"), "server with the web ui").unwrap();
            std::fs::write(dir.join("local/helper"), "helper").unwrap();
            std::fs::write(dir.join("local/dist/index.html"), "<html>").unwrap();
            std::fs::write(
                dir.join("snapshot-before"),
                snapshot("aaa", "unit1", "unit2"),
            )
            .unwrap();
            std::fs::write(
                dir.join("snapshot-after"),
                snapshot("bbb", "unit1", "unit2"),
            )
            .unwrap();
            if let Some(word) = marker {
                std::fs::write(dir.join("marker"), format!("{word}\n")).unwrap();
            }
            let web = WebSource::choose(
                Some(dir.join("local/lanlord")),
                Some(dir.join("local/dist")),
                Some(dir.join("local/helper")),
            )
            .unwrap();
            let script = r##"D='@DIR@'
for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sh -c 'id -un; id -u'") printf 'tester\n1000\n' ;;
  "sudo -n true") exit 0 ;;
  "uname -m") echo x86_64 ;;
  *"cat > /opt/lanlord/web-stage/lanlord'") echo upload >> "$D/log"; cat > "$D/up-server" ;;
  *"cat > /opt/lanlord/web-stage/lanlord-auth-helper'") echo upload >> "$D/log"; cat > "$D/up-helper" ;;
  *"cat > /opt/lanlord/web-stage/web.tar'") echo upload >> "$D/log"; cat > "$D/up-web" ;;
  "sh -c "*) exit 0 ;;
  *)
    input=$(cat)
    case "$input" in
      *lanlord_doctor*) printf 'virt_container=none\nvirt_vm=none\nnesting=yes\nkernel=6.1.0\n' ;;
      "cat @MARKER@"*) cat "$D/marker" 2>/dev/null || true ;;
      "# upgrade snapshot"*)
        if [ -f "$D/placed" ] && [ -f "$D/snapshot-fails" ]; then exit 1; fi
        if [ -f "$D/placed" ]; then cat "$D/snapshot-after"; else cat "$D/snapshot-before"; fi ;;
      "# upgrade restart"*)
        echo "$input" | grep '^systemctl restart' | sed 's/^systemctl //' >> "$D/log" ;;
      "# upgrade units"*) cat "$D/units" 2>/dev/null || true ;;
      "# upgrade extras"*)
        echo extras >> "$D/log"
        [ -f "$D/firewall-down" ] && echo lanlord-firewall.service; exit 0 ;;
      "# upgrade rollback"*)
        case "$input" in
          *"swap had not begun"*) [ -f "$D/moved" ] || { echo rollback-skipped >> "$D/log"; exit 3; } ;;
        esac
        echo rollback >> "$D/log"; touch "$D/rolled"; [ -f "$D/still-broken" ] && exit 1; exit 0 ;;
      "# stored stamp"*) echo 20260101T120000Z ;;
      "# stored restore"*) echo restore >> "$D/log" ;;
      "# upgrade check"*)
        echo check >> "$D/log"
        if [ -f "$D/down-before" ] && [ ! -f "$D/placed" ]; then exit 1; fi
        if [ -f "$D/unhealthy" ] && [ -f "$D/placed" ] && [ ! -f "$D/rolled" ]; then exit 1; fi; exit 0 ;;
      "set -eu
tar -C /opt/lanlord/web-stage/web -xf "*) ;;
      *"curl -fsSL"*) echo web-fetch >> "$D/log" ;;
      *'mkdir -p "$S/web"'*) echo web-prepare >> "$D/log" ;;
      "sha256sum "*)
        for file in up-server up-helper up-web; do
          printf '%s  x\n' "$({ sha256sum "$D/$file" 2>/dev/null || shasum -a 256 "$D/$file"; } | cut -d' ' -f1)"
        done ;;
      *"staged files are incomplete"*)
        echo web-install >> "$D/log"
        [ -f "$D/no-place-early" ] && exit 1
        touch "$D/moved"
        [ -f "$D/no-place" ] && exit 1
        touch "$D/placed" ;;
      *"--tags web-proxy,webui-login"*)
        echo web-apply >> "$D/log"; [ -f "$D/apply-fails" ] && exit 1; exit 0 ;;
      *"the web ui answers on https"*) echo web-verify >> "$D/log" ;;
      *) echo bootstrap >> "$D/log"; echo "setting up" ;;
    esac ;;
esac"##
                .replace("@DIR@", &dir.display().to_string())
                .replace("@MARKER@", MARKER);
            Server {
                session: Session::fake(&script),
                dir,
                web,
            }
        }

        fn installed(name: &str) -> Self {
            Self::new(name, Some("installed"))
        }

        fn flag(&self, name: &str) {
            std::fs::write(self.dir.join(name), "").unwrap();
        }

        fn log(&self) -> Vec<String> {
            std::fs::read_to_string(self.dir.join("log"))
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }

        fn upgrade(&self) -> (anyhow::Result<()>, Vec<Event>) {
            self.upgrade_with(&self.web, false)
        }

        fn upgrade_with(
            &self,
            web: &WebSource,
            allow_unhealthy: bool,
        ) -> (anyhow::Result<()>, Vec<Event>) {
            let options = Options {
                host: "gw".into(),
                ansible: AnsibleMode::Remote,
                passwordless_sudo: false,
            };
            let mut seen = Vec::new();
            let result = run_with(
                &self.session,
                &options,
                web,
                allow_unhealthy,
                &mut no_password,
                &mut |event| seen.push(event),
                &mut |stamp, _| {
                    use std::io::Write;
                    let mut log = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(self.dir.join("log"))
                        .unwrap();
                    writeln!(log, "migrate {stamp}").unwrap();
                    if self.dir.join("migrate-fails").exists() {
                        anyhow::bail!("settings.yml: written by LANlord 9.9 (format 9); upgrade LANlord to use it");
                    }
                    Ok(vec!["settings.yml: format 1 to 3".to_string()])
                },
            );
            (result, seen)
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    const FILES: [&str; 8] = [
        "check",
        "bootstrap",
        "web-prepare",
        "upload",
        "upload",
        "upload",
        "web-install",
        "web-apply",
    ];

    fn log_with(tail: &[&str]) -> Vec<String> {
        FILES
            .iter()
            .copied()
            .chain(["web-verify"])
            .chain(tail.iter().copied())
            .map(str::to_string)
            .collect()
    }

    fn outcomes(events: &[Event]) -> Vec<(Step, &'static str)> {
        events
            .iter()
            .filter_map(|event| match event {
                Event::StepStarted(step) => Some((*step, "started")),
                Event::StepDone(step) => Some((*step, "done")),
                Event::Failed { step, .. } => Some((*step, "failed")),
                Event::Line(_) => None,
            })
            .collect()
    }

    #[test]
    fn a_new_binary_restarts_both_services_and_checks_them() {
        let server = Server::installed("binary");
        let (result, events) = server.upgrade();
        result.unwrap();
        assert_eq!(
            server.log(),
            log_with(&[
                "restart lanlord-webui lanlord-operations",
                "check",
                "migrate 20260101T120000Z"
            ])
        );
        let done: Vec<_> = outcomes(&events)
            .into_iter()
            .filter(|(_, what)| *what == "done")
            .map(|(step, _)| step)
            .collect();
        assert_eq!(done, Step::ALL);
    }

    #[test]
    fn a_host_that_is_not_installed_is_refused_before_anything_changes() {
        for marker in [None, Some("nonsense")] {
            let server = Server::new("not-installed", marker);
            let (result, events) = server.upgrade();
            let message = format!("{:#}", result.unwrap_err());
            assert!(message.contains("not installed"), "{message}");
            assert!(message.contains("lanlord install"), "{message}");
            assert!(server.log().is_empty());
            assert_eq!(
                outcomes(&events),
                [(Step::Check, "started"), (Step::Check, "failed")]
            );
        }
    }

    #[test]
    fn a_half_finished_install_is_refused() {
        let server = Server::new("partial", Some("base"));
        let message = format!("{:#}", server.upgrade().0.unwrap_err());
        assert!(message.contains("not finished"), "{message}");
        assert!(server.log().is_empty());
    }

    #[test]
    fn a_server_binary_without_the_web_server_is_refused_before_the_upload() {
        let server = Server::installed("no-web");
        let marker: String = NO_WEB_SERVER_REVERSED.chars().rev().collect();
        std::fs::write(
            server.dir.join("local/lanlord"),
            format!("\u{7f}ELF...{marker}; build lanlord with --features webui"),
        )
        .unwrap();
        let (result, events) = server.upgrade();
        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("webui feature"), "{message}");
        assert!(server.log().is_empty());
        assert_eq!(
            outcomes(&events),
            [(Step::Check, "started"), (Step::Check, "failed")]
        );
    }

    #[test]
    fn a_sealed_secrets_failure_names_the_command_to_run() {
        assert_eq!(
            move_command("root@gw.example.net"),
            "lanlord move-secrets <your-admin>@gw.example.net"
        );
        assert_eq!(move_command("gw"), "lanlord move-secrets <your-admin>@gw");
        let said = sealed_hint(
            "the stored secrets are sealed to another key; move them to the gateway first",
            "root@192.0.2.7",
        );
        assert!(
            said.contains("`lanlord move-secrets <your-admin>@192.0.2.7`"),
            "{said}"
        );
        assert_eq!(sealed_hint("something else", "gw"), "something else");
    }

    #[test]
    fn a_failed_migration_puts_the_stored_files_back_and_rolls_the_upgrade_back() {
        let server = Server::installed("migrate-fails");
        server.flag("migrate-fails");
        let (result, events) = server.upgrade();
        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("upgrade LANlord to use it"), "{message}");
        assert!(
            message.contains("the stored files are as they were"),
            "{message}"
        );
        assert_eq!(
            server.log(),
            log_with(&[
                "restart lanlord-webui lanlord-operations",
                "check",
                "migrate 20260101T120000Z",
                "restore",
                "rollback"
            ])
        );
        assert_eq!(outcomes(&events).last(), Some(&(Step::Migrate, "failed")));
    }

    #[test]
    fn services_that_do_not_come_back_are_rolled_back() {
        let server = Server::installed("rollback");
        server.flag("unhealthy");
        let (result, events) = server.upgrade();
        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("did not come back"), "{message}");
        assert!(message.contains("running again"), "{message}");
        assert_eq!(
            server.log(),
            log_with(&[
                "restart lanlord-webui lanlord-operations",
                "check",
                "rollback"
            ])
        );
        assert_eq!(outcomes(&events).last(), Some(&(Step::Verify, "failed")));
    }

    #[test]
    fn a_rollback_that_does_not_help_says_so() {
        let server = Server::installed("rollback-fails");
        server.flag("unhealthy");
        server.flag("still-broken");
        let message = format!("{:#}", server.upgrade().0.unwrap_err());
        assert!(message.contains("still do not answer"), "{message}");
    }

    #[test]
    fn a_failed_apply_with_healthy_services_is_reported_without_a_rollback() {
        let server = Server::installed("apply-fails");
        server.flag("apply-fails");
        let (result, events) = server.upgrade();
        assert!(result.is_err());
        let log = server.log();
        assert!(log.contains(&"check".to_string()));
        assert!(!log.contains(&"rollback".to_string()));
        // the restart and the check still ran, so a half-applied upgrade is not left unchecked
        assert!(log.contains(&"restart lanlord-webui lanlord-operations".to_string()));
        assert!(outcomes(&events).contains(&(Step::Apply, "failed")));
        assert!(outcomes(&events).contains(&(Step::Verify, "done")));
    }

    #[test]
    fn a_failed_apply_with_services_down_is_rolled_back() {
        let server = Server::installed("apply-fails-down");
        server.flag("apply-fails");
        server.flag("unhealthy");
        assert!(server.upgrade().0.is_err());
        assert!(server.log().contains(&"rollback".to_string()));
    }

    #[test]
    fn a_swap_that_fails_after_a_move_is_rolled_back() {
        let server = Server::installed("no-place");
        server.flag("no-place");
        let (result, events) = server.upgrade();
        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("running again"), "{message}");
        assert!(message.contains("not reverted"), "{message}");
        let log = server.log();
        assert_eq!(log.last().map(String::as_str), Some("rollback"));
        assert!(!log.contains(&"web-apply".to_string()));
        assert_eq!(outcomes(&events).last(), Some(&(Step::Files, "failed")));
    }

    #[test]
    fn a_swap_that_fails_before_any_move_leaves_the_previous_copies_alone() {
        let server = Server::installed("no-place-early");
        server.flag("no-place-early");
        let message = format!("{:#}", server.upgrade().0.unwrap_err());
        assert!(message.contains("still in place"), "{message}");
        let log = server.log();
        assert!(log.contains(&"rollback-skipped".to_string()));
        assert!(!log.contains(&"rollback".to_string()));
    }

    #[test]
    fn services_that_are_down_before_the_upgrade_refuse_it() {
        let server = Server::installed("down-before");
        server.flag("down-before");
        let (result, events) = server.upgrade();
        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("before the upgrade"), "{message}");
        assert!(message.contains("--even-if-down"), "{message}");
        assert_eq!(server.log(), ["check"]);
        assert_eq!(outcomes(&events).last(), Some(&(Step::Check, "failed")));
    }

    #[test]
    fn services_that_were_down_are_not_blamed_on_the_upgrade() {
        let server = Server::installed("down-allowed");
        server.flag("down-before");
        server.flag("unhealthy");
        let message = format!(
            "{:#}",
            server.upgrade_with(&server.web, true).0.unwrap_err()
        );
        assert!(message.contains("nothing was rolled back"), "{message}");
        assert!(!server.log().contains(&"rollback".to_string()));
        // and when the upgrade brings them up, it simply succeeds
        let server = Server::installed("down-fixed");
        server.flag("down-before");
        server.upgrade_with(&server.web, true).0.unwrap();
        assert!(!server.log().contains(&"rollback".to_string()));
    }

    #[test]
    fn unreadable_files_after_the_swap_restart_every_service() {
        let server = Server::installed("snapshot-fails");
        server.flag("snapshot-fails");
        let (result, events) = server.upgrade();
        result.unwrap();
        assert!(server
            .log()
            .contains(&"restart lanlord-webui lanlord-operations".to_string()));
        assert!(events.iter().any(|event| matches!(
            event,
            Event::Line(line) if line.contains("restarting every service")
        )));
    }

    #[test]
    fn the_release_is_fetched_when_no_local_build_is_given() {
        let server = Server::installed("release");
        let (result, _) = server.upgrade_with(&WebSource::release(), false);
        result.unwrap();
        assert_eq!(
            server.log(),
            [
                "check",
                "bootstrap",
                "web-fetch",
                "web-install",
                "web-apply",
                "web-verify",
                "restart lanlord-webui lanlord-operations",
                "check",
                "migrate 20260101T120000Z"
            ]
        );
    }

    #[test]
    fn unchanged_files_restart_nothing_but_the_services_are_still_checked() {
        let server = Server::installed("unchanged");
        std::fs::copy(
            server.dir.join("snapshot-before"),
            server.dir.join("snapshot-after"),
        )
        .unwrap();
        server.upgrade().0.unwrap();
        let log = server.log();
        assert!(!log.iter().any(|line| line.starts_with("restart")));
        assert_eq!(
            log.last().map(String::as_str),
            Some("migrate 20260101T120000Z")
        );
        assert!(log.contains(&"check".to_string()));
    }

    #[test]
    fn a_stopped_unit_is_started_and_checked_even_when_no_file_changed() {
        let server = Server::installed("stopped-unit");
        std::fs::copy(
            server.dir.join("snapshot-before"),
            server.dir.join("snapshot-after"),
        )
        .unwrap();
        std::fs::write(
            server.dir.join("units"),
            "lanlord-webui.service active\nlanlord-firewall.service inactive\n",
        )
        .unwrap();
        server.upgrade().0.unwrap();
        let log = server.log();
        assert!(
            log.contains(&"restart lanlord-firewall.service".to_string()),
            "{log:?}"
        );
        assert_eq!(
            &log[log.len() - 3..],
            ["check", "extras", "migrate 20260101T120000Z"]
        );
    }

    #[test]
    fn a_unit_that_stays_down_fails_the_upgrade_without_a_rollback() {
        let server = Server::installed("firewall-down");
        std::fs::write(
            server.dir.join("units"),
            "lanlord-firewall.service failed\n",
        )
        .unwrap();
        server.flag("firewall-down");
        let (result, events) = server.upgrade();
        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("lanlord-firewall.service"), "{message}");
        assert!(message.contains("systemctl status"), "{message}");
        assert!(message.contains("nothing was rolled back"), "{message}");
        assert!(!message.contains("running again"), "{message}");
        assert_eq!(outcomes(&events).last(), Some(&(Step::Verify, "failed")));
        let log = server.log();
        assert!(!log.contains(&"rollback".to_string()), "{log:?}");
        assert!(log.contains(
            &"restart lanlord-webui lanlord-operations lanlord-firewall.service".to_string()
        ));
    }

    #[test]
    fn a_core_service_that_fails_is_still_rolled_back_with_a_healthy_extra_unit() {
        let server = Server::installed("core-down");
        std::fs::write(
            server.dir.join("units"),
            "lanlord-firewall.service active\n",
        )
        .unwrap();
        server.flag("unhealthy");
        let (result, _) = server.upgrade();
        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("running again"), "{message}");
        assert!(server.log().contains(&"rollback".to_string()));
    }

    #[test]
    fn the_units_are_read_from_systemctl_output() {
        let text = "lanlord-webui.service active\nlanlord-firewall.service inactive\n\
                    lanlord-dhcp.service failed\nlanlord-dhcp.socket active\n\
                    lanlord-sweep.timer inactive\nother.service active\ngarbage\n";
        let unit = |name: &str, active| Unit {
            name: name.into(),
            active,
        };
        assert_eq!(
            parse_units(text),
            [
                unit("lanlord-webui.service", true),
                unit("lanlord-firewall.service", false),
                unit("lanlord-dhcp.service", false),
                unit("lanlord-dhcp.socket", true),
                unit("lanlord-sweep.timer", false)
            ]
        );
        let script = units_script();
        assert!(script.contains("--state=enabled"));
        assert!(script.contains("'lanlord-*.socket' 'lanlord-*.timer'"));
        assert!(extras_script(&["lanlord-firewall.service".into()])
            .contains("in lanlord-firewall.service;"));
    }

    #[test]
    fn inactive_units_join_the_plan() {
        let same = snapshot("a", "u1", "u2");
        let plan = Plan::new(&Snapshot::parse(&same), &Snapshot::parse(&same));
        assert!(plan.restart.is_empty());
        let unit = |name: &str, active| Unit {
            name: name.into(),
            active,
        };
        let plan = plan.with_units(&[
            unit("lanlord-webui.service", false),
            unit("lanlord-operations.service", true),
            unit("lanlord-firewall.service", false),
            unit("lanlord-dhcp.socket", true),
        ]);
        assert_eq!(plan.restart, [SERVICES[0]]);
        assert_eq!(plan.extra_start, ["lanlord-firewall.service"]);
        assert_eq!(
            plan.extra_check,
            ["lanlord-firewall.service", "lanlord-dhcp.socket"]
        );
    }

    #[test]
    fn the_snapshot_reads_hashes_and_missing_files() {
        let snapshot = Snapshot::parse(&format!("abc {BINARY}\nnone {WEBUI}\n"));
        assert_eq!(snapshot.get(BINARY).map(String::as_str), Some("abc"));
        assert_eq!(snapshot.get(WEBUI), None);
        assert_eq!(snapshot.get(OPERATIONS), None);
        let script = snapshot_script();
        for path in [BINARY, WEBUI, OPERATIONS] {
            assert!(script.contains(path), "{path}");
        }
    }

    #[test]
    fn the_plan_restarts_what_runs_a_changed_binary_or_unit() {
        let before = Snapshot::parse(&snapshot("a", "u1", "u2"));
        let plan = |after: String| Plan::new(&before, &Snapshot::parse(&after));
        // a new binary: both
        assert_eq!(plan(snapshot("b", "u1", "u2")).restart, SERVICES);
        // a new unit only: that service
        assert_eq!(plan(snapshot("a", "u1", "u3")).restart, [SERVICES[1]]);
        assert_eq!(plan(snapshot("a", "u9", "u2")).restart, [SERVICES[0]]);
        // nothing new: nothing restarted, both checked
        let same = plan(snapshot("a", "u1", "u2"));
        assert!(same.restart.is_empty());
        assert_eq!(same.check, SERVICES);
        // a unit that appeared (the operations service on an older install) is restarted and checked
        let older = Snapshot::parse(&format!("a {BINARY}\nu1 {WEBUI}\nnone {OPERATIONS}\n"));
        let appeared = Plan::new(&older, &Snapshot::parse(&snapshot("a", "u1", "u2")));
        assert_eq!(appeared.restart, [SERVICES[1]]);
        // a unit that does not exist is neither restarted nor checked
        let missing = Plan::new(
            &older,
            &Snapshot::parse(&format!("b {BINARY}\nu1 {WEBUI}\nnone {OPERATIONS}\n")),
        );
        assert_eq!(missing.restart, [SERVICES[0]]);
        assert_eq!(missing.check, [SERVICES[0]]);
    }

    #[test]
    fn the_scripts_name_the_services_and_the_previous_copies() {
        assert!(restart_script(&SERVICES, &[])
            .ends_with("systemctl restart lanlord-webui lanlord-operations\n"));
        let check = check_script(&SERVICES, &[]);
        assert!(check.contains("--unix-socket"));
        assert!(check.contains("lanlord-operations"));
        // only the web ui has a socket to ask
        assert!(!check_script(&[SERVICES[1]], &[]).contains("--unix-socket"));
        let rollback = rollback_script("", &SERVICES, false);
        for part in [
            "$name.prev",
            "lanlord-auth-helper",
            "$web_dir.prev",
            "systemctl restart lanlord-webui",
        ] {
            assert!(rollback.contains(part), "{part}");
        }
    }

    #[test]
    fn the_web_server_marker_is_what_a_build_without_it_prints() {
        let main = include_str!("../../lanlord/src/main.rs");
        let message: String = NO_WEB_SERVER_REVERSED.chars().rev().collect();
        assert!(main.contains(&message), "{message}");
    }

    #[test]
    fn a_binary_is_checked_for_the_marker() {
        let dir =
            std::env::temp_dir().join(format!("lanlord-upgrade-marker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let with = dir.join("with");
        let without = dir.join("without");
        let message: String = NO_WEB_SERVER_REVERSED.chars().rev().collect();
        std::fs::write(&with, format!("x{message}y")).unwrap();
        std::fs::write(&without, "x this build has a web server y").unwrap();
        assert!(!has_web_server(&with).unwrap());
        assert!(has_web_server(&without).unwrap());
        assert!(has_web_server(&dir.join("missing")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// a folder that stands in for a server's root, with stubs for systemctl, curl and sleep.
    struct Root(std::path::PathBuf);

    impl Root {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "lanlord-upgrade-root-{}-{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("stubs")).unwrap();
            std::fs::create_dir_all(dir.join("opt/lanlord/bin")).unwrap();
            let stub = |name: &str, body: &str| {
                let path = dir.join("stubs").join(name);
                std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
                std::process::Command::new("chmod")
                    .arg("+x")
                    .arg(&path)
                    .status()
                    .unwrap();
            };
            stub(
                "systemctl",
                &format!(
                    "case \"$1\" in\n\
                     is-active) if [ -f '{0}/down' ]; then echo failed; else echo active; fi ;;\n\
                     cat) echo 'ExecStart=/opt/lanlord/bin/lanlord webui --socket /run/lanlord-web/web.sock' ;;\n\
                     restart) echo \"$*\" >> '{0}/log' ;;\n\
                     esac",
                    dir.display()
                ),
            );
            stub("curl", &format!("[ ! -f '{}/down' ]", dir.display()));
            stub("sleep", "exit 0");
            Root(dir)
        }

        fn path(&self, relative: &str) -> std::path::PathBuf {
            self.0.join(relative)
        }

        fn write(&self, relative: &str, text: &str) {
            let path = self.path(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        fn read(&self, relative: &str) -> String {
            std::fs::read_to_string(self.path(relative)).unwrap_or_default()
        }

        /// both generations of everything, as an upgrade leaves them.
        fn with_previous(&self) {
            self.write("opt/lanlord/bin/lanlord", "new");
            self.write("opt/lanlord/bin/lanlord.prev", "old");
            self.write("opt/lanlord/bin/lanlord-auth-helper", "new");
            self.write("opt/lanlord/bin/lanlord-auth-helper.prev", "old");
            self.write("opt/lanlord/web/index.html", "new");
            self.write("opt/lanlord/web.prev/index.html", "old");
        }

        /// run `script` with sh, the stubs first on the path; the exit status and what it printed.
        fn run(&self, script: &str) -> (i32, String) {
            use std::io::Write;
            let mut child = std::process::Command::new("sh")
                .arg("-s")
                .env(
                    "PATH",
                    format!("{}:/usr/bin:/bin", self.path("stubs").display()),
                )
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(script.as_bytes())
                .unwrap();
            let out = child.wait_with_output().unwrap();
            (
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stderr).into_owned()
                    + &String::from_utf8_lossy(&out.stdout),
            )
        }

        fn root(&self) -> String {
            self.0.display().to_string()
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_rollback_puts_every_previous_copy_back_and_restarts() {
        let root = Root::new("rollback");
        root.with_previous();
        let (code, output) = root.run(&rollback_script(&root.root(), &SERVICES, false));
        assert_eq!(code, 0, "{output}");
        for (relative, want) in [
            ("opt/lanlord/bin/lanlord", "old"),
            ("opt/lanlord/bin/lanlord-auth-helper", "old"),
            ("opt/lanlord/web/index.html", "old"),
            ("opt/lanlord/bin/lanlord.bad", "new"),
            ("opt/lanlord/bin/lanlord-auth-helper.bad", "new"),
        ] {
            assert_eq!(root.read(relative), want, "{relative}");
        }
        assert!(!root.path("opt/lanlord/web.prev").exists());
        assert_eq!(
            root.read("log").trim(),
            "restart lanlord-webui lanlord-operations"
        );
    }

    #[test]
    fn the_rollback_without_a_previous_copy_changes_nothing_and_exits_2() {
        let root = Root::new("no-previous");
        root.write("opt/lanlord/bin/lanlord", "new");
        root.write("opt/lanlord/web/index.html", "new");
        let (code, output) = root.run(&rollback_script(&root.root(), &SERVICES, false));
        assert_eq!(code, 2, "{output}");
        assert_eq!(root.read("opt/lanlord/bin/lanlord"), "new");
        assert_eq!(root.read("opt/lanlord/web/index.html"), "new");
        assert_eq!(root.read("log"), "");
    }

    #[test]
    fn the_rollback_restores_a_binary_that_is_missing_after_a_failed_swap() {
        let root = Root::new("missing");
        root.write("opt/lanlord/bin/lanlord.prev", "old");
        let (code, output) = root.run(&rollback_script(&root.root(), &SERVICES, false));
        assert_eq!(code, 0, "{output}");
        assert_eq!(root.read("opt/lanlord/bin/lanlord"), "old");
    }

    #[test]
    fn the_rollback_exits_1_when_the_services_still_do_not_answer() {
        let root = Root::new("still-down");
        root.with_previous();
        root.write("down", "");
        let (code, output) = root.run(&rollback_script(&root.root(), &SERVICES, false));
        assert_eq!(code, 1, "{output}");
        assert!(output.contains("lanlord-webui is failed"), "{output}");
        assert_eq!(root.read("opt/lanlord/bin/lanlord"), "old");
    }

    #[test]
    fn the_rollback_stops_when_a_move_fails() {
        let root = Root::new("move-fails");
        // a folder cannot be moved onto a file, so putting the web folder back fails
        root.write("opt/lanlord/web", "not a folder");
        root.write("opt/lanlord/web.prev/index.html", "old");
        let (code, _) = root.run(&rollback_script(&root.root(), &SERVICES, false));
        assert_eq!(code, 1);
        // it did not go on to the restart
        assert_eq!(root.read("log"), "");
    }

    #[test]
    fn a_guarded_rollback_waits_for_the_marker_of_a_begun_swap() {
        let root = Root::new("guarded");
        root.with_previous();
        let script = rollback_script(&root.root(), &SERVICES, true);
        let (code, output) = root.run(&script);
        assert_eq!(code, 3, "{output}");
        assert_eq!(root.read("opt/lanlord/bin/lanlord"), "new");
        root.write("opt/lanlord/bin/.placing", "");
        let (code, output) = root.run(&script);
        assert_eq!(code, 0, "{output}");
        assert_eq!(root.read("opt/lanlord/bin/lanlord"), "old");
        assert!(!root.path("opt/lanlord/bin/.placing").exists());
    }

    #[test]
    fn the_check_script_passes_for_answering_services_and_names_the_ones_that_do_not() {
        let root = Root::new("check");
        let (code, output) = root.run(&check_script(&SERVICES, &[]));
        assert_eq!(code, 0, "{output}");
        root.write("down", "");
        let (code, output) = root.run(&check_script(&SERVICES, &[]));
        assert_eq!(code, 1);
        assert!(output.contains("lanlord-operations is failed"), "{output}");
    }

    #[test]
    fn a_swap_that_fails_between_items_cannot_bring_back_older_backups() {
        let root = Root::new("stale");
        // an earlier upgrade left backups, and a web folder that cannot be moved into place
        root.write("opt/lanlord/bin/lanlord", "current");
        root.write("opt/lanlord/bin/lanlord.prev", "stale");
        root.write("opt/lanlord/bin/lanlord-auth-helper.prev", "stale helper");
        root.write("opt/lanlord/web.prev/index.html", "stale page");
        root.write("opt/lanlord/web", "not a folder");
        root.write("opt/lanlord/web-stage/lanlord", "new");
        root.write("opt/lanlord/web-stage/lanlord-auth-helper", "new helper");
        root.write("opt/lanlord/web-stage/web/index.html", "new page");
        std::fs::write(root.path("stubs/chown"), "#!/bin/sh\nexit 0\n").unwrap();
        std::process::Command::new("chmod")
            .arg("+x")
            .arg(root.path("stubs/chown"))
            .status()
            .unwrap();
        let (code, _) = root.run(&install_script(&root.root()));
        assert_ne!(code, 0);
        // the binary was swapped, the web folder was not, and the old backups are gone
        assert_eq!(root.read("opt/lanlord/bin/lanlord"), "new");
        assert_eq!(root.read("opt/lanlord/bin/lanlord.prev"), "current");
        assert!(!root
            .path("opt/lanlord/bin/lanlord-auth-helper.prev")
            .exists());
        assert!(!root.path("opt/lanlord/web.prev").exists());
        let (code, output) = root.run(&rollback_script(&root.root(), &SERVICES, true));
        assert_eq!(code, 0, "{output}");
        assert_eq!(root.read("opt/lanlord/bin/lanlord"), "current");
        // nothing older came back, and the unswapped items were left alone
        assert!(!root.path("opt/lanlord/web.prev").exists());
        assert_eq!(root.read("opt/lanlord/web"), "not a folder");
        assert_eq!(
            root.read("opt/lanlord/bin/lanlord-auth-helper"),
            "new helper"
        );
    }
}
