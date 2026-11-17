//! install lanlord on a server, and reinstall it after a purge.
//!
//! an install is five steps: check what the server can do, bootstrap it, apply the baseline
//! (docker and the firewall; networks and components are added later by the admin), set up the
//! web ui (see [`crate::webinstall`]) and finish.
//! after the bootstrap and the baseline a marker on the server records how far it got, so an
//! interrupted install resumes where it stopped. the purge removes the marker with the rest of
//! `/opt/lanlord`.

use anyhow::{bail, Result};

use crate::account::login_user;
use crate::bootstrap::{
    bootstrap_lines, capture_as_root, run_as_root, AnsibleMode, AskPassword, Options,
};
use crate::log::{emit, Kind};
use crate::purge::{self, PurgeOptions};
use crate::ssh::Session;
use crate::webinstall::{self, WebSource};
use crate::{apply, doctor, env};

/// where the progress marker lives; it holds one word, the last step that finished.
pub const MARKER: &str = "/opt/lanlord/install-state";

/// the modules the baseline applies: the gateway record, and docker with the firewall.
pub const BASE_MODULES: &[&str] = &["gateway", "docker"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    Check,
    Bootstrap,
    Base,
    Web,
    Finish,
}

impl Step {
    pub const ALL: [Step; 5] = [
        Step::Check,
        Step::Bootstrap,
        Step::Base,
        Step::Web,
        Step::Finish,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Step::Check => "Checking the server",
            Step::Bootstrap => "Setting up the gateway",
            Step::Base => "Installing Docker and the firewall",
            Step::Web => "Installing the web UI",
            Step::Finish => "Finishing",
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    NotInstalled,
    /// stopped after this step; `run` carries on from the next one.
    Partial(Step),
    Installed,
}

fn marker_word(step: Step) -> &'static str {
    match step {
        Step::Check => "check",
        Step::Bootstrap => "bootstrap",
        Step::Base => "base",
        Step::Web => "web",
        Step::Finish => "installed",
    }
}

fn parse_marker(text: &str) -> Status {
    match text.trim() {
        "bootstrap" => Status::Partial(Step::Bootstrap),
        "base" => Status::Partial(Step::Base),
        "web" => Status::Partial(Step::Web),
        "installed" => Status::Installed,
        _ => Status::NotInstalled,
    }
}

/// what is on `host`: nothing, a half-finished install, or a finished one.
pub fn status(host: &str, ask_password: AskPassword) -> Result<Status> {
    status_with(&Session::new(host)?, ask_password)
}

pub(crate) fn status_with(session: &Session, ask_password: AskPassword) -> Result<Status> {
    let admin = login_user(session)?;
    let script = format!("cat {MARKER} 2>/dev/null || true\n");
    let out = capture_as_root(session, &admin.name, script, ask_password)?;
    Ok(parse_marker(&out))
}

fn record(session: &Session, step: Step, ask_password: AskPassword) -> Result<()> {
    let admin = login_user(session)?;
    let script = format!(
        "printf '%s\\n' {} > {MARKER}.tmp && chmod 0644 {MARKER}.tmp && mv -f {MARKER}.tmp {MARKER}\n",
        marker_word(step)
    );
    match run_as_root(session, &admin.name, script, ask_password)? {
        0 => Ok(()),
        code => bail!("could not record the progress on the server (status {code})"),
    }
}

/// fail unless the docker engine answers on the server.
fn verify_docker(session: &Session, ask_password: AskPassword) -> Result<()> {
    let admin = login_user(session)?;
    let script = "docker version --format '{{.Server.Version}}' >/dev/null 2>&1\n".to_string();
    match run_as_root(session, &admin.name, script, ask_password)? {
        0 => Ok(()),
        _ => bail!(
            "the baseline finished but docker does not answer on the server (`docker version` failed); \
             check that the docker service is running and rerun the install"
        ),
    }
}

/// the bootstrap options the installer uses. ansible always goes onto the gateway: apply runs it
/// there, so an ansible on this machine must not make bootstrap skip the gateway's venv.
fn options_for(host: &str) -> Options {
    Options {
        host: host.to_string(),
        ansible: AnsibleMode::Remote,
        passwordless_sudo: false,
    }
}

/// install lanlord on `host`, resuming a half-finished install. an installed server is left
/// alone; use [`reinstall`] to wipe it first.
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

pub(crate) fn run_with(
    session: &Session,
    options: &Options,
    web: &WebSource,
    ask_password: AskPassword,
    events: &mut dyn FnMut(Event),
) -> Result<()> {
    let done = match status_with(session, &mut *ask_password)? {
        Status::Installed => bail!(
            "lanlord is already installed on {}; use `lanlord reinstall` to wipe it and start over",
            options.host
        ),
        Status::Partial(step) => Some(step),
        Status::NotInstalled => None,
    };
    for step in Step::ALL {
        events(Event::StepStarted(step));
        // the check and the bootstrap always run: the check is cheap and the server may have
        // changed since, and the bootstrap is idempotent and repairs an old lanlord account
        if !matches!(step, Step::Check | Step::Bootstrap) && done.is_some_and(|d| step <= d) {
            events(Event::Line("already done, skipping".into()));
            events(Event::StepDone(step));
            continue;
        }
        match do_step(
            session,
            options,
            web,
            step,
            done,
            &mut *ask_password,
            events,
        ) {
            Ok(()) => events(Event::StepDone(step)),
            Err(err) => {
                let message = format!("{err:#}");
                events(Event::Failed {
                    step,
                    message: message.clone(),
                });
                bail!(message);
            }
        }
    }
    emit(
        Kind::Info,
        format!("lanlord is installed on {}", options.host),
    );
    Ok(())
}

fn do_step(
    session: &Session,
    options: &Options,
    web: &WebSource,
    step: Step,
    done: Option<Step>,
    ask_password: AskPassword,
    events: &mut dyn FnMut(Event),
) -> Result<()> {
    let mut line = |text: &str| events(Event::Line(text.to_string()));
    match step {
        Step::Check => {
            let report = doctor::run_with(session, &mut *ask_password)?;
            line(&format!(
                "this server is a {}",
                report.environment.describe()
            ));
            match env::blocking(&report, BASE_MODULES) {
                Some(why) => bail!(why),
                None => Ok(()),
            }
        }
        Step::Bootstrap => {
            bootstrap_lines(session, options, &mut *ask_password, &mut line)?;
            // a rerun must not move a later marker back
            if done.is_none_or(|d| d < Step::Bootstrap) {
                record(session, Step::Bootstrap, ask_password)
            } else {
                Ok(())
            }
        }
        Step::Base => {
            let modules: Vec<String> = BASE_MODULES.iter().map(ToString::to_string).collect();
            apply::apply_lines(
                session,
                &options.host,
                None,
                &modules,
                &mut *ask_password,
                &mut line,
            )?;
            // a module that was skipped still reports done, so look for the engine itself
            verify_docker(session, &mut *ask_password)?;
            record(session, Step::Base, ask_password)
        }
        Step::Web => {
            webinstall::run_step(session, &options.host, web, &mut *ask_password, &mut line)?;
            record(session, Step::Web, ask_password)
        }
        Step::Finish => record(session, Step::Finish, ask_password),
    }
}

fn check_confirm(host: &str, confirm: &str) -> Result<()> {
    if confirm != host {
        bail!("the host name does not match; nothing was removed");
    }
    Ok(())
}

/// wipe `host` with a purge and install again. `confirm` must be the host name, exactly, as
/// the typed confirmation of the purge; anything else refuses before anything is touched.
pub fn reinstall(
    host: &str,
    confirm: &str,
    web: &WebSource,
    ask_password: AskPassword,
    events: &mut dyn FnMut(Event),
) -> Result<()> {
    check_confirm(host, confirm)?;
    reinstall_with(
        &Session::new(host)?,
        &options_for(host),
        web,
        ask_password,
        events,
    )
}

pub(crate) fn reinstall_with(
    session: &Session,
    options: &Options,
    web: &WebSource,
    ask_password: AskPassword,
    events: &mut dyn FnMut(Event),
) -> Result<()> {
    // refuse before anything is removed when the server would fail the install's check
    let report = doctor::run_with(session, &mut *ask_password)?;
    if let Some(why) = env::blocking(&report, BASE_MODULES) {
        bail!("{why}\nnothing was removed");
    }
    let lines = purge::purge(
        session,
        &options.host,
        PurgeOptions::default(),
        &mut *ask_password,
    )?;
    for line in lines {
        events(Event::Line(line));
    }
    run_with(session, options, web, ask_password, events)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::testgw::no_password;

    const FINE: &str = "virt_container=none\nvirt_vm=none\nnesting=yes\nkernel=6.1.0\n";
    const NO_NESTING: &str =
        "virt_container=lxc\nvirt_vm=none\nuid_map=0 100000 65536\nnesting=no\nkeyctl=no\n";

    /// a fake server: the marker is a file in `dir`, and what the server does is logged there.
    struct Server {
        dir: std::path::PathBuf,
        session: Session,
        web: WebSource,
    }

    /// what the web ui step does on a fresh install: stage, three uploads, put in place, apply, check.
    const WEB_LOG: [&str; 7] = [
        "web-prepare",
        "upload",
        "upload",
        "upload",
        "web-install",
        "web-apply",
        "web-verify",
    ];

    fn with_web(mut log: Vec<&'static str>, tail: &[&'static str]) -> Vec<&'static str> {
        log.extend(WEB_LOG);
        log.extend(tail);
        log
    }

    impl Server {
        fn new(name: &str, marker: Option<&str>, probe: &str) -> Self {
            Self::build(name, marker, probe, "echo play: baseline")
        }

        /// `apply` is what the server does when it is given the baseline.
        fn build(name: &str, marker: Option<&str>, probe: &str, apply: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("lanlord-install-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("local/dist")).unwrap();
            std::fs::write(dir.join("local/lanlord"), "server").unwrap();
            std::fs::write(dir.join("local/helper"), "helper").unwrap();
            std::fs::write(dir.join("local/dist/index.html"), "<html>").unwrap();
            let web = WebSource::choose(
                Some(dir.join("local/lanlord")),
                Some(dir.join("local/dist")),
                Some(dir.join("local/helper")),
            )
            .unwrap();
            if let Some(word) = marker {
                std::fs::write(dir.join("marker"), format!("{word}\n")).unwrap();
            }
            let script = r#"D='@DIR@'
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
      *lanlord_doctor*) printf '%s' '@PROBE@' ;;
      *"docker version"*) [ -f "$D/no-docker" ] && exit 1; exit 0 ;;
      "cat @MARKER@"*) cat "$D/marker" 2>/dev/null || true ;;
      "printf '%s\n' "*)
        word=$(printf '%s' "$input" | head -n 1 | cut -d' ' -f3)
        printf '%s\n' "$word" > "$D/marker"
        echo "$word" >> "$D/log" ;;
      "set -eu
tar -C /opt/lanlord/web-stage/web -xf "*) ;;
      *'mkdir -p "$S/web"'*) echo web-prepare >> "$D/log" ;;
      "sha256sum "*)
        for f in up-server up-helper up-web; do
          printf '%s  x\n' "$({ sha256sum "$D/$f" 2>/dev/null || shasum -a 256 "$D/$f"; } | cut -d' ' -f1)"
        done
        if [ -f "$D/bad-upload" ]; then echo "0000  x"; fi ;;
      "rm -rf /opt/lanlord/web-stage"*|"rm -rf '/opt/lanlord/web-stage'"*) echo web-clean >> "$D/log" ;;
      *"staged files are incomplete"*) echo web-install >> "$D/log" ;;
      *"--tags web-proxy,webui-login"*) echo web-apply >> "$D/log" ;;
      *"the web ui answers on https"*) echo web-verify >> "$D/log"; [ -f "$D/no-web" ] && { echo "nothing answered" >&2; exit 1; }; exit 0 ;;
      *"DRY=0"*) echo purge >> "$D/log"; rm -f "$D/marker"; echo "remove /opt/lanlord" ;;
      *"base64 -d <<'LANLORD_PAYLOAD'"*) echo apply >> "$D/log"; @APPLY@ ;;
      *) echo bootstrap >> "$D/log"; echo "setting up" ;;
    esac ;;
esac"#
                .replace("@DIR@", &dir.display().to_string())
                .replace("@MARKER@", MARKER)
                .replace("@PROBE@", probe)
                .replace("@APPLY@", apply);
            Server {
                session: Session::fake(&script),
                dir,
                web,
            }
        }

        fn log(&self) -> Vec<String> {
            std::fs::read_to_string(self.dir.join("log"))
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }

        fn marker(&self) -> String {
            std::fs::read_to_string(self.dir.join("marker"))
                .unwrap_or_default()
                .trim()
                .to_string()
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn the_installer_always_puts_ansible_on_the_gateway() {
        assert!(matches!(options_for("gw").ansible, AnsibleMode::Remote));
    }

    fn options() -> Options {
        Options {
            host: "gw".into(),
            ansible: AnsibleMode::Remote,
            passwordless_sudo: false,
        }
    }

    fn install(server: &Server) -> (Result<()>, Vec<Event>) {
        let mut seen = Vec::new();
        let result = run_with(
            &server.session,
            &options(),
            &server.web,
            &mut no_password,
            &mut |e| seen.push(e),
        );
        (result, seen)
    }

    fn steps(events: &[Event]) -> Vec<(Step, &'static str)> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::StepStarted(s) => Some((*s, "started")),
                Event::StepDone(s) => Some((*s, "done")),
                Event::Failed { step, .. } => Some((*step, "failed")),
                Event::Line(_) => None,
            })
            .collect()
    }

    #[test]
    fn the_marker_reads_back_as_a_status() {
        assert_eq!(parse_marker(""), Status::NotInstalled);
        assert_eq!(parse_marker("garbage"), Status::NotInstalled);
        for step in [Step::Bootstrap, Step::Base, Step::Web] {
            assert_eq!(parse_marker(marker_word(step)), Status::Partial(step));
        }
        assert_eq!(parse_marker("installed\n"), Status::Installed);
        assert_eq!(parse_marker(marker_word(Step::Finish)), Status::Installed);
    }

    #[test]
    fn status_reads_the_marker_on_the_server() {
        for (marker, want) in [
            (None, Status::NotInstalled),
            (Some("base"), Status::Partial(Step::Base)),
            (Some("web"), Status::Partial(Step::Web)),
            (Some("installed"), Status::Installed),
        ] {
            let server = Server::new("status", marker, FINE);
            assert_eq!(
                status_with(&server.session, &mut no_password).unwrap(),
                want
            );
        }
    }

    #[test]
    fn a_fresh_install_runs_the_steps_in_order_and_marks_it_installed() {
        let server = Server::new("fresh", None, FINE);
        let (result, events) = install(&server);
        result.unwrap();
        let mut want = Vec::new();
        for step in Step::ALL {
            want.push((step, "started"));
            want.push((step, "done"));
        }
        assert_eq!(steps(&events), want);
        // the server's output is streamed as lines inside its step
        let at = |e: Event| events.iter().position(|x| *x == e).unwrap();
        let setup = at(Event::Line("setting up".into()));
        assert!(at(Event::StepStarted(Step::Bootstrap)) < setup);
        assert!(setup < at(Event::StepStarted(Step::Base)));
        let play = at(Event::Line("play: baseline".into()));
        assert!(at(Event::StepStarted(Step::Base)) < play);
        assert!(play < at(Event::StepStarted(Step::Finish)));
        // progress is recorded after each step
        assert_eq!(
            server.log(),
            with_web(
                vec!["bootstrap", "bootstrap", "apply", "base"],
                &["web", "installed"]
            )
        );
        assert_eq!(server.marker(), "installed");
    }

    #[test]
    fn the_servers_lines_reach_the_log_only_as_debug_so_they_print_once() {
        let server = Server::new("once", None, FINE);
        let log = crate::log::subscribe();
        let (result, _) = install(&server);
        result.unwrap();
        let shown: Vec<_> = log
            .try_iter()
            .filter(|l| l.text == "play: baseline" || l.text == "setting up")
            .collect();
        assert!(!shown.is_empty());
        assert!(shown.iter().all(|l| l.kind == Kind::Debug), "{shown:?}");
    }

    #[test]
    fn a_partial_install_resumes_after_the_step_it_finished() {
        let server = Server::new("resume", Some("bootstrap"), FINE);
        let (result, events) = install(&server);
        result.unwrap();
        // the bootstrap runs again (it is idempotent and repairs an old account), then the
        // baseline and the finish
        assert_eq!(
            server.log(),
            with_web(vec!["bootstrap", "apply", "base"], &["web", "installed"])
        );
        assert!(!events.contains(&Event::Line("already done, skipping".into())));
        assert_eq!(steps(&events).len(), 10);
        assert_eq!(server.marker(), "installed");

        let server = Server::new("resume-base", Some("base"), FINE);
        let (result, events) = install(&server);
        result.unwrap();
        // a finished baseline is skipped, but the bootstrap still runs without moving the marker
        assert_eq!(
            server.log(),
            with_web(vec!["bootstrap"], &["web", "installed"])
        );
        assert!(events.contains(&Event::Line("already done, skipping".into())));

        // a finished web ui step is skipped too
        let server = Server::new("resume-web", Some("web"), FINE);
        let (result, events) = install(&server);
        result.unwrap();
        assert_eq!(server.log(), ["bootstrap", "installed"]);
        let skipped = events
            .iter()
            .filter(|e| **e == Event::Line("already done, skipping".into()))
            .count();
        assert_eq!(skipped, 2);
    }

    #[test]
    fn an_installed_server_is_left_alone() {
        let server = Server::new("done", Some("installed"), FINE);
        let (result, events) = install(&server);
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("already installed"));
        assert!(events.is_empty());
        assert!(server.log().is_empty());
    }

    #[test]
    fn a_check_that_fails_stops_with_the_fix_before_anything_changes() {
        let server = Server::new("blocked", None, NO_NESTING);
        let (result, events) = install(&server);
        let err = result.unwrap_err().to_string();
        assert!(err.contains("pct set"), "{err}");
        assert_eq!(
            steps(&events),
            [(Step::Check, "started"), (Step::Check, "failed")]
        );
        let Some(Event::Failed { message, .. }) = events.last() else {
            panic!("no failure event");
        };
        assert_eq!(*message, err);
        assert!(server.log().is_empty());
        assert_eq!(server.marker(), "");
    }

    #[test]
    fn a_failing_step_is_reported_and_keeps_the_earlier_progress() {
        let server = Server::build("fails", None, FINE, "echo boom >&2; exit 1");
        let (result, events) = install(&server);
        assert!(result.is_err());
        assert_eq!(steps(&events).last(), Some(&(Step::Base, "failed")));
        assert!(events.contains(&Event::Line("boom".into())));
        // the bootstrap's marker stays, so the next run resumes at the baseline
        assert_eq!(server.marker(), "bootstrap");
    }

    #[test]
    fn a_baseline_that_leaves_no_working_docker_fails_the_step() {
        let server = Server::new("nodocker", None, FINE);
        std::fs::write(server.dir.join("no-docker"), "").unwrap();
        let (result, events) = install(&server);
        let err = result.unwrap_err().to_string();
        assert!(err.contains("docker does not answer"), "{err}");
        assert_eq!(steps(&events).last(), Some(&(Step::Base, "failed")));
        // the marker stays at the bootstrap, so the baseline runs again
        assert_eq!(server.marker(), "bootstrap");
    }

    #[test]
    fn a_web_ui_that_does_not_answer_fails_the_step_and_keeps_the_baseline() {
        let server = Server::new("noweb", None, FINE);
        std::fs::write(server.dir.join("no-web"), "").unwrap();
        let (result, events) = install(&server);
        let err = result.unwrap_err().to_string();
        assert!(err.contains("does not answer on https port 8443"), "{err}");
        assert_eq!(steps(&events).last(), Some(&(Step::Web, "failed")));
        // the next run resumes at the web ui step
        assert_eq!(server.marker(), "base");
    }

    #[test]
    fn files_that_arrive_damaged_are_not_installed() {
        let server = Server::new("badupload", None, FINE);
        std::fs::write(server.dir.join("bad-upload"), "").unwrap();
        let (result, _) = install(&server);
        let err = result.unwrap_err().to_string();
        assert!(err.contains("do not match"), "{err}");
        let log = server.log();
        assert!(log.contains(&"web-clean".to_string()));
        assert!(!log.contains(&"web-install".to_string()));
        assert_eq!(server.marker(), "base");
    }

    #[test]
    fn reinstall_refuses_without_the_exact_host_name() {
        for wrong in ["", "GW", "gw ", "other", "gw.example"] {
            let mut seen = Vec::new();
            let err = reinstall(
                "gw",
                wrong,
                &WebSource::release(),
                &mut no_password,
                &mut |e| seen.push(e),
            )
            .unwrap_err()
            .to_string();
            assert!(err.contains("does not match"), "{wrong:?}: {err}");
            assert!(seen.is_empty());
        }
        assert!(check_confirm("gw", "gw").is_ok());
    }

    #[test]
    fn reinstall_purges_and_then_installs_from_scratch() {
        let server = Server::new("reinstall", Some("installed"), FINE);
        let mut seen = Vec::new();
        reinstall_with(
            &server.session,
            &options(),
            &server.web,
            &mut no_password,
            &mut |e| seen.push(e),
        )
        .unwrap();
        assert_eq!(
            server.log(),
            with_web(
                vec!["purge", "bootstrap", "bootstrap", "apply", "base"],
                &["web", "installed"]
            )
        );
        // what the purge said comes first, then the five steps
        assert_eq!(seen[0], Event::Line("remove /opt/lanlord".into()));
        assert_eq!(seen[1], Event::StepStarted(Step::Check));
        assert_eq!(server.marker(), "installed");
    }

    #[test]
    fn reinstall_of_a_server_that_fails_the_check_removes_nothing() {
        let server = Server::new("reinstall-blocked", Some("installed"), NO_NESTING);
        let mut seen = Vec::new();
        let err = reinstall_with(
            &server.session,
            &options(),
            &server.web,
            &mut no_password,
            &mut |e| seen.push(e),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("pct set") && err.contains("nothing was removed"),
            "{err}"
        );
        assert!(server.log().is_empty());
        assert_eq!(server.marker(), "installed");
        assert!(seen.is_empty());
    }

    #[test]
    fn the_baseline_is_docker_and_the_firewall_only() {
        assert_eq!(BASE_MODULES, ["gateway", "docker"]);
    }
}
