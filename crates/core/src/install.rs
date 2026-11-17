//! install lanlord on a server, and reinstall it after a purge.
//!
//! an install is four steps: check what the server can do, bootstrap it, apply the baseline
//! (docker and the firewall; networks and components are added later by the admin) and finish.
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
    Finish,
}

impl Step {
    pub const ALL: [Step; 4] = [Step::Check, Step::Bootstrap, Step::Base, Step::Finish];

    pub fn title(self) -> &'static str {
        match self {
            Step::Check => "Checking the server",
            Step::Bootstrap => "Setting up the gateway",
            Step::Base => "Installing Docker and the firewall",
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
        Step::Finish => "installed",
    }
}

fn parse_marker(text: &str) -> Status {
    match text.trim() {
        "bootstrap" => Status::Partial(Step::Bootstrap),
        "base" => Status::Partial(Step::Base),
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

fn options_for(host: &str) -> Options {
    Options {
        host: host.to_string(),
        ansible: AnsibleMode::detect(),
        passwordless_sudo: false,
    }
}

/// install lanlord on `host`, resuming a half-finished install. an installed server is left
/// alone; use [`reinstall`] to wipe it first.
pub fn run(host: &str, ask_password: AskPassword, events: &mut dyn FnMut(Event)) -> Result<()> {
    run_with(
        &Session::new(host)?,
        &options_for(host),
        ask_password,
        events,
    )
}

pub(crate) fn run_with(
    session: &Session,
    options: &Options,
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
        // the check always runs; it is cheap and the server may have changed since
        if step != Step::Check && done.is_some_and(|d| step <= d) {
            events(Event::Line("already done, skipping".into()));
            events(Event::StepDone(step));
            continue;
        }
        match do_step(session, options, step, &mut *ask_password, events) {
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
    step: Step,
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
            record(session, Step::Bootstrap, ask_password)
        }
        Step::Base => {
            let modules: Vec<String> = BASE_MODULES.iter().map(|m| m.to_string()).collect();
            apply::apply_lines(
                session,
                &options.host,
                None,
                &modules,
                &mut *ask_password,
                &mut line,
            )?;
            record(session, Step::Base, ask_password)
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
    ask_password: AskPassword,
    events: &mut dyn FnMut(Event),
) -> Result<()> {
    check_confirm(host, confirm)?;
    reinstall_with(
        &Session::new(host)?,
        &options_for(host),
        ask_password,
        events,
    )
}

pub(crate) fn reinstall_with(
    session: &Session,
    options: &Options,
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
    run_with(session, options, ask_password, events)
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
            std::fs::create_dir_all(&dir).unwrap();
            if let Some(word) = marker {
                std::fs::write(dir.join("marker"), format!("{word}\n")).unwrap();
            }
            let script = r#"D='@DIR@'
for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sh -c 'id -un; id -u'") printf 'tester\n1000\n' ;;
  "sudo -n true") exit 0 ;;
  "sh -c "*) exit 0 ;;
  *)
    input=$(cat)
    case "$input" in
      *lanlord_doctor*) printf '%s' '@PROBE@' ;;
      "cat @MARKER@"*) cat "$D/marker" 2>/dev/null || true ;;
      "printf '%s\n' "*)
        word=$(printf '%s' "$input" | head -n 1 | cut -d' ' -f3)
        printf '%s\n' "$word" > "$D/marker"
        echo "$word" >> "$D/log" ;;
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

    fn options() -> Options {
        Options {
            host: "gw".into(),
            ansible: AnsibleMode::Remote,
            passwordless_sudo: false,
        }
    }

    fn install(server: &Server) -> (Result<()>, Vec<Event>) {
        let mut seen = Vec::new();
        let result = run_with(&server.session, &options(), &mut no_password, &mut |e| {
            seen.push(e)
        });
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
        for step in [Step::Bootstrap, Step::Base] {
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
            ["bootstrap", "bootstrap", "apply", "base", "installed"]
        );
        assert_eq!(server.marker(), "installed");
    }

    #[test]
    fn a_partial_install_resumes_after_the_step_it_finished() {
        let server = Server::new("resume", Some("bootstrap"), FINE);
        let (result, events) = install(&server);
        result.unwrap();
        // the bootstrap is not run again; the baseline and the finish are
        assert_eq!(server.log(), ["apply", "base", "installed"]);
        assert!(events.contains(&Event::Line("already done, skipping".into())));
        assert_eq!(steps(&events).len(), 8);
        assert_eq!(server.marker(), "installed");

        let server = Server::new("resume-base", Some("base"), FINE);
        install(&server).0.unwrap();
        assert_eq!(server.log(), ["installed"]);
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
    fn reinstall_refuses_without_the_exact_host_name() {
        for wrong in ["", "GW", "gw ", "other", "gw.example"] {
            let mut seen = Vec::new();
            let err = reinstall("gw", wrong, &mut no_password, &mut |e| seen.push(e))
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
        reinstall_with(&server.session, &options(), &mut no_password, &mut |e| {
            seen.push(e)
        })
        .unwrap();
        assert_eq!(
            server.log(),
            [
                "purge",
                "bootstrap",
                "bootstrap",
                "apply",
                "base",
                "installed"
            ]
        );
        // what the purge said comes first, then the four steps
        assert_eq!(seen[0], Event::Line("remove /opt/lanlord".into()));
        assert_eq!(seen[1], Event::StepStarted(Step::Check));
        assert_eq!(server.marker(), "installed");
    }

    #[test]
    fn reinstall_of_a_server_that_fails_the_check_removes_nothing() {
        let server = Server::new("reinstall-blocked", Some("installed"), NO_NESTING);
        let mut seen = Vec::new();
        let err = reinstall_with(&server.session, &options(), &mut no_password, &mut |e| {
            seen.push(e)
        })
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
