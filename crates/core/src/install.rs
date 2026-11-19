//! install lanlord on a server, and reinstall it after a purge.
//!
//! an install is five steps: check what the server can do, bootstrap it, apply the baseline
//! (docker and the firewall; networks and components are added later by the admin), set up the
//! web ui (see [`crate::webinstall`]) and finish.
//! after the bootstrap and the baseline a marker on the server records how far it got, so an
//! interrupted install resumes where it stopped. the purge removes the marker with the rest of
//! `/opt/lanlord`.

use anyhow::{anyhow, bail, Result};

use crate::account::login_user;
use crate::bootstrap::{
    bootstrap_lines, capture_as_root, run_as_root, AnsibleMode, AskPassword, Options,
};
use crate::config;
use crate::firstadmin::{self, Decide};
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
    /// what to do once the install is finished, ready to print.
    NextSteps(Vec<String>),
    StepDone(Step),
    Failed {
        step: Step,
        message: String,
    },
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
pub(crate) fn options_for(host: &str) -> Options {
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
    decide: Decide,
    events: &mut dyn FnMut(Event),
) -> Result<()> {
    run_with(
        &Session::new(host)?,
        &options_for(host),
        web,
        ask_password,
        decide,
        events,
    )
}

pub(crate) fn run_with(
    session: &Session,
    options: &Options,
    web: &WebSource,
    ask_password: AskPassword,
    decide: Decide,
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
    // the first administrator: the install is done either way, so a refusal is reported after
    // the next steps rather than instead of them
    let admin = firstadmin::run(session, &options.host, decide, &mut *ask_password);
    // advice only: an install that worked is not undone by a failed look
    let facts = login_user(session)
        .and_then(|admin| {
            capture_as_root(session, &admin.name, NEXT_FACTS.to_string(), ask_password)
        })
        .unwrap_or_default();
    // the address the user typed to reach the server is the best one to show, when it is one
    let facts = match reachable_address(&options.host) {
        Some(address) => format!("ip={address}\n{facts}"),
        None => facts,
    };
    let name = admin.as_ref().ok().and_then(|name| name.as_deref());
    events(Event::NextSteps(next_steps(&facts, name)));
    admin.map(|_| ()).map_err(|err| {
        anyhow!(
            "{err:#}\nlanlord is installed on {host}, so do not reinstall; to set up the first \
             administrator run `lanlord admin {host} add USER` or `lanlord admin {host} create \
             NAME`, or sign in as root in the web UI",
            host = options.host
        )
    })
}

/// the host the user gave, when a browser could use it too: an address or a dotted name, not
/// localhost and not an ssh alias.
fn reachable_address(host: &str) -> Option<String> {
    let host = host.rsplit('@').next()?.trim();
    let host = host.rsplit_once(':').map_or(host, |(name, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) {
            name
        } else {
            host
        }
    });
    let local =
        host.is_empty() || host.starts_with("127.") || host.eq_ignore_ascii_case("localhost");
    (!local && host.contains('.') && !host.contains(char::is_whitespace)).then(|| host.to_string())
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
        Step::Finish => {
            // the web step applied its own settings only for the web modules; a first
            // `networks add` or `apply` needs a settings file, so give it the same defaults
            config::push_plain_if_missing_with(
                session,
                &options.host,
                webinstall::SETTINGS.as_bytes(),
                &mut *ask_password,
            )?;
            record(session, Step::Finish, ask_password)
        }
    }
}

/// asks the server for its address and what state root's password is in.
const NEXT_FACTS: &str = r#"# the address the server reaches others with: the first global one may be docker's bridge
ip=$(ip -4 route get 192.0.2.1 2>/dev/null | sed -n 's/.* src \([0-9.]*\).*/\1/p' | head -n 1)
[ -n "$ip" ] || ip=$(ip -o -4 addr show scope global 2>/dev/null | awk '$2 !~ /^(docker|br-|veth|virbr|lxcbr|incusbr)/ {print $4}' | cut -d/ -f1 | head -n 1)
[ -n "$ip" ] || ip=$(hostname -I 2>/dev/null | awk '{for (i = 1; i <= NF; i++) if ($i !~ /^172\.17\./) {print $i; exit}}')
echo "ip=$ip"
echo "root=$(passwd -S root 2>/dev/null | awk '{print $2}')"
"#;

/// the web ui's address on the server, as `NEXT_FACTS` found it.
pub(crate) fn web_url(facts: &str) -> Option<String> {
    let ip = facts.lines().find_map(|l| l.strip_prefix("ip="))?.trim();
    (!ip.is_empty()).then(|| format!("https://{ip}:{}/", webinstall::PORT))
}

/// the closing advice: where the web ui is, and how root gets in. `passwd -S` says `L` (locked)
/// or `NP` (no password) when root cannot sign in yet; lanlord never sets a password itself.
pub(crate) fn next_steps(facts: &str, admin: Option<&str>) -> Vec<String> {
    let open = match web_url(facts) {
        Some(url) => format!("open the web UI at {url}"),
        None => format!("open the web UI on port {} of the server", webinstall::PORT),
    };
    let mut lines = vec![
        "next steps".to_string(),
        format!("  1. {open} (the certificate is the gateway's own, so the browser warns once)"),
    ];
    if let Some(admin) = admin {
        lines.push(format!(
            "  2. sign in as {admin}, your LANlord administrator"
        ));
        return lines;
    }
    lines.push("  2. sign in as root and create your admin account".to_string());
    let root = facts
        .lines()
        .find_map(|l| l.strip_prefix("root="))
        .map(str::trim);
    if matches!(root, Some("L" | "NP")) {
        lines.push(
            "     root has no usable password on this server yet, so it cannot sign in: \
             run `passwd` on the server to set one first"
                .to_string(),
        );
    }
    lines
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
    decide: Decide,
    events: &mut dyn FnMut(Event),
) -> Result<()> {
    check_confirm(host, confirm)?;
    reinstall_with(
        &Session::new(host)?,
        &options_for(host),
        web,
        ask_password,
        decide,
        events,
    )
}

pub(crate) fn reinstall_with(
    session: &Session,
    options: &Options,
    web: &WebSource,
    ask_password: AskPassword,
    decide: Decide,
    events: &mut dyn FnMut(Event),
) -> Result<()> {
    // refuse before anything is removed when the server would fail the install's check
    let report = doctor::run_with(session, &mut *ask_password)?;
    if let Some(why) = env::blocking(&report, BASE_MODULES) {
        bail!("{why}\nnothing was removed");
    }
    // an ssh session: the account it signs in as is the one asking
    let lines = purge::purge(
        session,
        &login_user(session)?.name,
        &options.host,
        PurgeOptions::default(),
        &mut *ask_password,
    )?;
    for line in lines {
        events(Event::Line(line));
    }
    run_with(session, options, web, ask_password, decide, events)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::firstadmin::Choice;
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
      *"is already in admins"*) echo adopted >> "$D/log"; echo "added" ;;
      *"does not exist; run lanlord bootstrap first"*) cat "$D/admins" 2>/dev/null || true ;;
      *"useradd --create-home"*) echo created >> "$D/log"; echo "created" ;;
      *"sudo -l -U"*) cat "$D/sudo-answer" 2>/dev/null || echo yes ;;
      *"default settings, only if missing"*)
        if [ -f "$D/settings" ]; then echo kept; else printf '%s\n' "$input" | sed -n '/^base64 -d/,/^LANLORD_PAYLOAD/p' | sed '1d;$d' | base64 -d > "$D/settings"; echo settings >> "$D/log"; echo created; fi ;;
      *"passwd -S root"*) echo "ip=192.0.2.10"; echo "root=$(cat "$D/root-state" 2>/dev/null || echo P)" ;;
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
        install_choosing(server, Choice::Skip)
    }

    fn install_choosing(server: &Server, choice: Choice) -> (Result<()>, Vec<Event>) {
        let mut seen = Vec::new();
        let result = run_with(
            &server.session,
            &options(),
            &server.web,
            &mut no_password,
            &mut |_| Ok(choice.clone()),
            &mut |e| seen.push(e),
        );
        (result, seen)
    }

    fn next_text(events: &[Event]) -> String {
        events
            .iter()
            .find_map(|e| match e {
                Event::NextSteps(lines) => Some(lines.join("\n")),
                _ => None,
            })
            .unwrap_or_default()
    }

    #[test]
    fn an_existing_account_with_sudo_is_adopted_as_the_first_administrator() {
        let server = Server::new("adopt", None, FINE);
        let (result, events) = install_choosing(&server, Choice::Adopt("olive".into()));
        result.unwrap();
        assert!(server.log().contains(&"adopted".to_string()));
        assert!(next_text(&events).contains("sign in as olive"));
        assert_eq!(server.marker(), "installed");
    }

    #[test]
    fn a_new_administrator_is_created_through_the_account_operation() {
        let server = Server::new("create", None, FINE);
        let choice = Choice::Create {
            name: "olive".into(),
            password: zeroize::Zeroizing::new(["fixture", "1"].join("-")),
            ssh_public_key: None,
        };
        let (result, events) = install_choosing(&server, choice);
        result.unwrap();
        assert!(server.log().contains(&"created".to_string()));
        assert!(next_text(&events).contains("sign in as olive"));
    }

    #[test]
    fn an_account_that_cannot_use_sudo_is_refused_and_nothing_is_changed() {
        let server = Server::new("nosudo", None, FINE);
        std::fs::write(server.dir.join("sudo-answer"), "no\n").unwrap();
        let (result, events) = install_choosing(&server, Choice::Adopt("olive".into()));
        let err = result.unwrap_err().to_string();
        assert!(err.contains("cannot become root"), "{err}");
        assert!(!server.log().contains(&"adopted".to_string()));
        // the install itself is done, and its next steps still point at root's web setup
        assert_eq!(server.marker(), "installed");
        assert!(next_text(&events).contains("sign in as root"));
        let other = Server::new("adopt-root", None, FINE);
        let (result, _) = install_choosing(&other, Choice::Adopt("root".into()));
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("root is not an account"));
    }

    #[test]
    fn with_no_choice_made_the_first_run_in_the_web_ui_stays_the_way() {
        let server = Server::new("skip", None, FINE);
        let (result, events) = install_choosing(&server, Choice::Skip);
        result.unwrap();
        assert!(!server
            .log()
            .iter()
            .any(|l| l == "adopted" || l == "created"));
        assert!(next_text(&events).contains("sign in as root"));
    }

    #[test]
    fn nobody_is_asked_when_an_administrator_already_exists() {
        let server = Server::new("has-admin", None, FINE);
        std::fs::write(server.dir.join("admins"), "olive\n").unwrap();
        let mut asked = false;
        let mut seen = Vec::new();
        run_with(
            &server.session,
            &options(),
            &server.web,
            &mut no_password,
            &mut |_| {
                asked = true;
                Ok(Choice::Skip)
            },
            &mut |e| seen.push(e),
        )
        .unwrap();
        assert!(!asked);
    }

    fn steps(events: &[Event]) -> Vec<(Step, &'static str)> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::StepStarted(s) => Some((*s, "started")),
                Event::StepDone(s) => Some((*s, "done")),
                Event::Failed { step, .. } => Some((*step, "failed")),
                Event::Line(_) | Event::NextSteps(_) => None,
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
                &["web", "settings", "installed"]
            )
        );
        assert_eq!(server.marker(), "installed");
    }

    #[test]
    fn default_settings_are_created_when_missing_and_left_alone_when_present() {
        let server = Server::new("settings-new", None, FINE);
        install(&server).0.unwrap();
        let stored = std::fs::read_to_string(server.dir.join("settings")).unwrap();
        // the file the install stores is the web ui's defaults, which are valid settings
        assert!(stored.contains(webinstall::SETTINGS), "{stored}");
        lanlord_model::validate::settings_document(webinstall::SETTINGS).unwrap();

        let server = Server::new("settings-kept", None, FINE);
        std::fs::write(server.dir.join("settings"), "mine: true\n").unwrap();
        install(&server).0.unwrap();
        assert_eq!(
            std::fs::read_to_string(server.dir.join("settings")).unwrap(),
            "mine: true\n"
        );
        assert!(!server.log().contains(&"settings".to_string()));
    }

    #[test]
    fn a_finished_install_ends_with_the_next_steps() {
        let server = Server::new("next", None, FINE);
        let (result, events) = install(&server);
        result.unwrap();
        let Some(Event::NextSteps(lines)) = events.last() else {
            panic!("{events:?}")
        };
        let text = lines.join("\n");
        assert!(text.contains("https://192.0.2.10:8443/"), "{text}");
        assert!(text.contains("sign in as root"));
        assert!(!text.contains("passwd"));

        let server = Server::new("next-locked", None, FINE);
        std::fs::write(server.dir.join("root-state"), "L\n").unwrap();
        let (_, events) = install(&server);
        let Some(Event::NextSteps(lines)) = events.last() else {
            panic!("{events:?}")
        };
        assert!(lines.join("\n").contains("run `passwd` on the server"));
    }

    #[test]
    fn the_address_fallbacks_skip_the_bridges_docker_and_friends_make() {
        let dir = std::env::temp_dir().join(format!("lanlord-nextfacts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // a stub `ip`: no route answer, and docker0 listed before the real interface
        let stub = dir.join("ip");
        std::fs::write(
            &stub,
            "#!/bin/sh\n\
             case \"$*\" in\n\
             *route*) ;;\n\
             *) echo '3: docker0    inet 172.17.0.1/16 brd 172.17.255.255 scope global docker0'\n\
                echo '4: br-1a2b3c    inet 172.18.0.1/16 scope global br-1a2b3c'\n\
                echo '2: eth0    inet 192.0.2.10/24 brd 192.0.2.255 scope global eth0' ;;\n\
             esac\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!("{}:/usr/bin:/bin", dir.display());
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(NEXT_FACTS)
            .env("PATH", path)
            .output()
            .unwrap();
        let facts = String::from_utf8_lossy(&out.stdout);
        assert_eq!(
            web_url(&facts).as_deref(),
            Some("https://192.0.2.10:8443/"),
            "{facts}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_host_the_user_gave_is_shown_when_a_browser_can_use_it() {
        assert_eq!(
            reachable_address("root@192.0.2.10").as_deref(),
            Some("192.0.2.10")
        );
        assert_eq!(
            reachable_address("gw.example.net:22").as_deref(),
            Some("gw.example.net")
        );
        for host in ["localhost", "root@127.0.0.1", "gw", ""] {
            assert_eq!(reachable_address(host), None, "{host}");
        }
    }

    #[test]
    fn a_failed_first_administrator_says_how_to_finish_without_reinstalling() {
        let server = Server::new("finish", None, FINE);
        std::fs::write(server.dir.join("sudo-answer"), "no\n").unwrap();
        let (result, _) = install_choosing(&server, Choice::Adopt("olive".into()));
        let err = format!("{:#}", result.unwrap_err());
        assert!(err.contains("do not reinstall"), "{err}");
        assert!(err.contains("lanlord admin gw add USER"), "{err}");
        assert!(err.contains("web UI"), "{err}");
        assert_eq!(server.marker(), "installed");
    }

    #[test]
    fn the_next_steps_name_the_address_and_a_root_without_a_password() {
        assert_eq!(
            web_url("ip=198.51.100.7\nroot=P\n").as_deref(),
            Some("https://198.51.100.7:8443/")
        );
        assert_eq!(web_url("ip=\nroot=P\n"), None);
        for state in ["L", "NP"] {
            let text = next_steps(&format!("ip=198.51.100.7\nroot={state}\n"), None).join("\n");
            assert!(text.contains("`passwd`"), "{text}");
        }
        let text = next_steps("ip=198.51.100.7\nroot=P\n", None).join("\n");
        assert!(!text.contains("passwd"), "{text}");
        // no address and no facts still give the steps, without a made-up url
        let text = next_steps("", None).join("\n");
        assert!(
            text.contains("port 8443") && !text.contains("https://"),
            "{text}"
        );
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
            with_web(
                vec!["bootstrap", "apply", "base"],
                &["web", "settings", "installed"]
            )
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
            with_web(vec!["bootstrap"], &["web", "settings", "installed"])
        );
        assert!(events.contains(&Event::Line("already done, skipping".into())));

        // a finished web ui step is skipped too
        let server = Server::new("resume-web", Some("web"), FINE);
        let (result, events) = install(&server);
        result.unwrap();
        assert_eq!(server.log(), ["bootstrap", "settings", "installed"]);
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
                &mut |_| Ok(Choice::Skip),
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
            &mut |_| Ok(Choice::Skip),
            &mut |e| seen.push(e),
        )
        .unwrap();
        assert_eq!(
            server.log(),
            with_web(
                vec!["purge", "bootstrap", "bootstrap", "apply", "base"],
                &["web", "settings", "installed"]
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
            &mut |_| Ok(Choice::Skip),
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
