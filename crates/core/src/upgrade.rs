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

use crate::bootstrap::{
    bootstrap_lines, capture_as_root, run_as_root, run_as_root_lines, AskPassword, Options,
};
use crate::install::{options_for, status_with, Status};
use crate::log::{emit, Kind};
use crate::ssh::Session;
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
}

impl Step {
    pub const ALL: [Step; 6] = [
        Step::Check,
        Step::Bootstrap,
        Step::Files,
        Step::Apply,
        Step::Restart,
        Step::Verify,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Step::Check => "Checking the server",
            Step::Bootstrap => "Checking the gateway account",
            Step::Files => "Replacing the server binary and the browser build",
            Step::Apply => "Applying the web UI modules",
            Step::Restart => "Restarting the changed services",
            Step::Verify => "Checking that the services came back",
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
         for f in {}; do\n\
         \x20 if [ -f \"$f\" ]; then echo \"$(sha256sum \"$f\" | cut -d' ' -f1) $f\"; else echo \"none $f\"; fi\n\
         done\n",
        files.join(" ")
    )
}

/// which services are restarted and which are checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub restart: Vec<Service>,
    pub check: Vec<Service>,
}

impl Plan {
    /// a new binary restarts every service; a new unit restarts that service. every service whose
    /// unit exists is checked, restarted or not. a new unit is restarted too, though the playbook
    /// has usually started it already.
    pub fn new(before: &Snapshot, after: &Snapshot) -> Self {
        let binary_changed = before.get(BINARY) != after.get(BINARY);
        let check: Vec<Service> = SERVICES
            .into_iter()
            .filter(|service| after.get(service.unit).is_some())
            .collect();
        let restart = check
            .iter()
            .copied()
            .filter(|service| binary_changed || before.get(service.unit) != after.get(service.unit))
            .collect();
        Plan { restart, check }
    }
}

fn names(services: &[Service]) -> String {
    services
        .iter()
        .map(|service| service.name)
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn restart_script(services: &[Service]) -> String {
    format!("# upgrade restart\nsystemctl restart {}\n", names(services))
}

/// the shell function `check`: every service active, and the web ui answering on its socket,
/// three times in a row a second apart, within thirty seconds. the socket is the one the
/// installed unit names, as tools/dev-deploy.sh reads it.
fn check_function(services: &[Service]) -> String {
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
         \x20   for s in {names}; do\n\
         \x20     [ \"$(systemctl is-active \"$s\" 2>/dev/null)\" = active ] || ok=0\n\
         \x20   done\n\
         \x20   if [ \"$ok\" = 1 ]{probe}; then good=$((good + 1)); else good=0; fi\n\
         \x20   [ \"$good\" -ge 3 ] && return 0\n\
         \x20   tries=$((tries + 1))\n\
         \x20   sleep 1\n\
         \x20 done\n\
         \x20 for s in {names}; do echo \"$s is $(systemctl is-active \"$s\" 2>&1 | head -n 1)\" >&2; done\n\
         \x20 return 1\n\
         }}\n",
        names = names(services)
    )
}

pub fn check_script(services: &[Service]) -> String {
    format!("# upgrade check\n{}check\n", check_function(services))
}

/// put the binary and the web folder back from `lanlord.prev` and `web.prev`, restart the
/// services and check them. exit 0: they answer again; 1: the previous version is back but they
/// still do not answer; 2: there was no previous version to put back.
pub fn rollback_script(services: &[Service]) -> String {
    format!(
        "# upgrade rollback\n\
         B=/opt/lanlord/bin; W=/opt/lanlord/web\n\
         found=0\n\
         if [ -f \"$B/lanlord.prev\" ]; then mv -f \"$B/lanlord\" \"$B/lanlord.bad\"; mv -f \"$B/lanlord.prev\" \"$B/lanlord\"; found=1; fi\n\
         if [ -d \"$W.prev\" ]; then rm -rf \"$W.bad\"; mv \"$W\" \"$W.bad\"; mv \"$W.prev\" \"$W\"; found=1; fi\n\
         [ \"$found\" = 1 ] || exit 2\n\
         systemctl restart {names}\n\
         {function}check\n",
        names = names(services),
        function = check_function(services)
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
        Err(err) => {
            let message = format!("{err:#}");
            events(Event::Failed {
                step,
                message: message.clone(),
            });
            bail!(message)
        }
    }
}

/// upgrade the lanlord on `host`; it must be installed.
pub fn run(
    host: &str,
    web: &WebSource,
    ask_password: AskPassword,
    events: &mut dyn FnMut(Event),
) -> Result<()> {
    run_with(
        &Session::new(host)?,
        &options_for(host),
        web,
        ask_password,
        events,
    )
}

/// refuse what cannot be upgraded, and read what the server runs now.
fn check(
    session: &Session,
    options: &Options,
    web: &WebSource,
    ask: AskPassword,
    on_line: &mut dyn FnMut(&str),
) -> Result<Snapshot> {
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
    snapshot(session, ask)
}

/// put the previous binary and web folder back and say how that went.
fn roll_back(
    session: &Session,
    plan: &Plan,
    ask: AskPassword,
    on_line: &mut dyn FnMut(&str),
) -> Result<String> {
    on_line("the services did not come back; putting the previous version back");
    let admin = login_user(session)?;
    let code = run_as_root_lines(
        session,
        &admin.name,
        rollback_script(&plan.check),
        ask,
        on_line,
    )?;
    Ok(match code {
        0 => "the previous version is running again".to_string(),
        2 => "there is no previous version to put back".to_string(),
        _ => "the previous version is back but the services still do not answer; \
              look at them on the server"
            .to_string(),
    })
}

pub(crate) fn run_with(
    session: &Session,
    options: &Options,
    web: &WebSource,
    ask_password: AskPassword,
    events: &mut dyn FnMut(Event),
) -> Result<()> {
    let ask = ask_password;
    let before = step(events, Step::Check, |line| {
        check(session, options, web, &mut *ask, line)
    })?;
    step(events, Step::Bootstrap, |line| {
        bootstrap_lines(session, options, &mut *ask, line)
    })?;
    // the stage and the swap: a failure here leaves what was running before running
    step(events, Step::Files, |line| {
        webinstall::stage(session, web, &mut *ask, line)?;
        webinstall::place(session, &mut *ask, line)
    })?;
    // a failed apply does not roll back by itself; what counts is whether the services answer
    let applied = step(events, Step::Apply, |line| {
        webinstall::converge(session, &options.host, &mut *ask, line)
    });
    let plan = step(events, Step::Restart, |line| {
        let plan = Plan::new(&before, &snapshot(session, &mut *ask)?);
        if plan.restart.is_empty() {
            line("no service runs a changed binary or unit");
        } else {
            line(&format!("restarting {}", names(&plan.restart)));
            let code = root_status(session, restart_script(&plan.restart), &mut *ask)?;
            if code != 0 {
                line(&format!("the restart reported status {code}"));
            }
        }
        Ok(plan)
    })?;
    step(events, Step::Verify, |line| {
        if root_status(session, check_script(&plan.check), &mut *ask)? == 0 {
            line(&format!("{} answering", names(&plan.check)));
            return Ok(());
        }
        let outcome = roll_back(session, &plan, &mut *ask, line)?;
        bail!("the services did not come back after the upgrade; {outcome}")
    })?;
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
        check_script, has_web_server, restart_script, rollback_script, run_with, snapshot_script,
        Event, Plan, Snapshot, Step, BINARY, NO_WEB_SERVER_REVERSED, SERVICES,
    };
    use crate::webinstall::WebSource;

    const WEBUI: &str = SERVICES[0].unit;
    const OPERATIONS: &str = SERVICES[1].unit;

    fn snapshot(binary: &str, webui: &str, operations: &str) -> String {
        format!("{binary} {BINARY}\n{webui} {WEBUI}\n{operations} {OPERATIONS}\n")
    }

    /// a fake server: the marker and the snapshots are files in `dir`, and what the server does
    /// is logged there. the flag files `unhealthy`, `still-broken`, `no-place` and `apply-fails`
    /// make it misbehave.
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
        if [ -f "$D/placed" ]; then cat "$D/snapshot-after"; else cat "$D/snapshot-before"; fi ;;
      "# upgrade restart"*)
        echo "$input" | grep '^systemctl restart' | sed 's/^systemctl //' >> "$D/log" ;;
      "# upgrade rollback"*)
        echo rollback >> "$D/log"; touch "$D/rolled"; [ -f "$D/still-broken" ] && exit 1; exit 0 ;;
      "# upgrade check"*)
        echo check >> "$D/log"
        if [ -f "$D/unhealthy" ] && [ ! -f "$D/rolled" ]; then exit 1; fi; exit 0 ;;
      "set -eu
tar -C /opt/lanlord/web-stage/web -xf "*) ;;
      *'mkdir -p "$S/web"'*) echo web-prepare >> "$D/log" ;;
      "sha256sum "*)
        for f in up-server up-helper up-web; do
          printf '%s  x\n' "$({ sha256sum "$D/$f" 2>/dev/null || shasum -a 256 "$D/$f"; } | cut -d' ' -f1)"
        done ;;
      *"staged files are incomplete"*)
        echo web-install >> "$D/log"; [ -f "$D/no-place" ] && exit 1; touch "$D/placed" ;;
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
            let options = Options {
                host: "gw".into(),
                ansible: AnsibleMode::Remote,
                passwordless_sudo: false,
            };
            let mut seen = Vec::new();
            let result = run_with(
                &self.session,
                &options,
                &self.web,
                &mut no_password,
                &mut |e| seen.push(e),
            );
            (result, seen)
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    const FILES: [&str; 7] = [
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
            log_with(&["restart lanlord-webui lanlord-operations", "check"])
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
    fn a_failed_swap_stops_without_a_rollback() {
        let server = Server::installed("no-place");
        server.flag("no-place");
        let (result, events) = server.upgrade();
        assert!(result.is_err());
        let log = server.log();
        assert!(!log.contains(&"rollback".to_string()));
        assert!(!log.contains(&"web-apply".to_string()));
        assert_eq!(outcomes(&events).last(), Some(&(Step::Files, "failed")));
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
        assert_eq!(log.last().map(String::as_str), Some("check"));
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
        assert!(restart_script(&SERVICES)
            .ends_with("systemctl restart lanlord-webui lanlord-operations\n"));
        let check = check_script(&SERVICES);
        assert!(check.contains("--unix-socket"));
        assert!(check.contains("lanlord-operations"));
        // only the web ui has a socket to ask
        assert!(!check_script(&[SERVICES[1]]).contains("--unix-socket"));
        let rollback = rollback_script(&SERVICES);
        for part in ["lanlord.prev", "$W.prev", "systemctl restart lanlord-webui"] {
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
}
