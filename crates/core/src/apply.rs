use anyhow::{bail, Result};

use lanlord_model::apply::{self, TaskStatus};

pub use lanlord_model::apply::ApplyEvent;

use crate::bootstrap::{run_as_root_lines, AskPassword};
use crate::config::{wrap, INSTALLED_GUARD, NOT_INSTALLED_MESSAGE, NOT_INSTALLED_STATUS};
use crate::log::{emit, Kind};
use crate::modules;
use crate::payload;
use crate::settings;
use crate::ssh::Session;

/// the shell script that unpacks the playbooks and runs them as lanlord;
/// `settings` is decrypted yaml that lives in /run only for the duration.
fn script(settings: Option<&[u8]>, tags: &[&str]) -> Result<String> {
    // the playbooks get the settings only after the shared checks, with every string marked
    // unsafe so ansible never runs a value through jinja
    let checked = match settings {
        Some(plain) => {
            Some(lanlord_model::validate::for_ansible(std::str::from_utf8(plain)?)?.into_bytes())
        }
        None => None,
    };
    let (stage, vars) = match checked.as_deref() {
        Some(plain) => (
            format!(
                r#"install -d -o lanlord -g lanlord -m 0700 /run/lanlord
trap 'rm -f /run/lanlord/settings.yml' EXIT
base64 -d <<'LANLORD_SETTINGS' | sudo -n -u lanlord tee /run/lanlord/settings.yml >/dev/null
{}
LANLORD_SETTINGS
chmod 0600 /run/lanlord/settings.yml
"#,
                wrap(plain)
            ),
            " -e @/run/lanlord/settings.yml",
        ),
        None => (String::new(), ""),
    };
    // after the settings, so they cannot override it: the playbooks pin the images to this version
    let vars = format!(
        "{vars} -e lanlord_image_version={}",
        env!("LANLORD_IMAGE_VERSION")
    );
    let tags = if tags.is_empty() {
        String::new()
    } else {
        format!(" --tags {}", tags.join(","))
    };
    Ok(format!(
        r#"set -eu
{guard}if [ ! -x /opt/lanlord/venv/bin/ansible-playbook ]; then
  echo "ansible is not installed on the gateway; run lanlord bootstrap --ansible remote first" >&2
  exit 1
fi
install -d -o lanlord -g lanlord -m 0750 /opt/lanlord/.ansible /opt/lanlord/.ansible/tmp /opt/lanlord/playbooks
base64 -d <<'LANLORD_PAYLOAD' | sudo -n -u lanlord tar -x -C /opt/lanlord/playbooks
{payload}
LANLORD_PAYLOAD
{stage}cd /opt/lanlord
sudo -n -u lanlord env HOME=/opt/lanlord PYTHONDONTWRITEBYTECODE=1 ANSIBLE_NOCOLOR=1 \
  ANSIBLE_STDOUT_CALLBACK=lanlord_progress ANSIBLE_CALLBACK_PLUGINS=/opt/lanlord/playbooks/callback_plugins \
  ANSIBLE_PYTHON_INTERPRETER=auto_silent ANSIBLE_LOCAL_TEMP=/opt/lanlord/.ansible/tmp \
  ANSIBLE_REMOTE_TEMP=/opt/lanlord/.ansible/tmp ANSIBLE_HOME=/opt/lanlord/.ansible \
  /opt/lanlord/venv/bin/ansible-playbook -i localhost,{vars}{tags} /opt/lanlord/playbooks/site.yml
"#,
        payload = payload::archive_base64()?,
        guard = INSTALLED_GUARD
    ))
}

/// push the playbooks to `host` and run them there as the lanlord user,
/// with your settings when the gateway has any and you have a key.
pub fn apply(
    host: &str,
    identity: Option<&age::x25519::Identity>,
    modules: &[String],
    ask_password: AskPassword,
) -> Result<()> {
    apply_with(&Session::new(host)?, host, identity, modules, ask_password)
}

pub(crate) fn apply_with(
    session: &Session,
    host: &str,
    identity: Option<&age::x25519::Identity>,
    modules: &[String],
    ask_password: AskPassword,
) -> Result<()> {
    apply_lines(session, host, identity, modules, ask_password, &mut |_| {})
}

/// like `apply_with`; every line the server prints also goes to `on_line` as it arrives.
pub(crate) fn apply_lines(
    session: &Session,
    host: &str,
    identity: Option<&age::x25519::Identity>,
    modules: &[String],
    ask_password: AskPassword,
    on_line: &mut dyn FnMut(&str),
) -> Result<()> {
    // only known module ids reach the shell script
    let mut tags = Vec::new();
    for id in modules {
        match modules::find(id) {
            Some(module) => tags.push(module.id),
            None => bail!("unknown module: {id}"),
        }
    }
    if tags.is_empty() {
        emit(Kind::Info, format!("applying everything to {host}"));
    } else {
        emit(
            Kind::Info,
            format!("applying {} to {host}", tags.join(", ")),
        );
    }
    let admin = session.capture(&["id".into(), "-un".into()])?;
    let admin = admin.trim();
    // the exec account's password is made before the playbooks need it
    if let Some(identity) = identity {
        if tags.is_empty() || tags.contains(&crate::nt4_exec::MODULE) {
            crate::nt4_exec::ensure_password(session, host, identity, ask_password)?;
        }
    }
    let settings = settings::merged_with(session, identity, ask_password)?;
    dispatch(session, admin, &tags, settings, ask_password, on_line)
}

/// apply `tags` with exactly `settings` instead of the ones stored on the gateway, for the
/// installer's own modules. the tags are the caller's constants, not user input.
pub(crate) fn apply_with_settings(
    session: &Session,
    host: &str,
    tags: &[&str],
    settings: &str,
    ask_password: AskPassword,
    on_line: &mut dyn FnMut(&str),
) -> Result<()> {
    emit(
        Kind::Info,
        format!("applying {} to {host}", tags.join(", ")),
    );
    let admin = session.capture(&["id".into(), "-un".into()])?;
    let settings = Some(settings.as_bytes().to_vec());
    run_tags(session, admin.trim(), tags, settings, ask_password, on_line)
}

// what this thread published, for the tests: the log reaches every subscriber, so the events of
// tests that run side by side would mix.
#[cfg(test)]
thread_local! {
    static PUBLISHED: std::cell::RefCell<Vec<ApplyEvent>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// send an event to the progress dialogs, and the readable part of it to the log.
fn publish(event: &ApplyEvent) {
    #[cfg(test)]
    PUBLISHED.with(|p| p.borrow_mut().push(event.clone()));
    emit(
        Kind::Event,
        serde_json::to_string(event).unwrap_or_default(),
    );
    match event {
        ApplyEvent::TaskDone {
            name,
            status,
            changed,
            msg,
        } => {
            let how = match (status, changed) {
                (TaskStatus::Skipped, _) => "skipped",
                (TaskStatus::Failed, _) => "FAILED",
                (TaskStatus::Unreachable, _) => "UNREACHABLE",
                (TaskStatus::Ok, true) => "changed",
                (TaskStatus::Ok, false) => "ok",
            };
            if msg.is_empty() {
                emit(Kind::Info, format!("{how}: {name}"));
            } else {
                emit(Kind::Info, format!("{how}: {name}: {msg}"));
            }
        }
        ApplyEvent::Log { text } => emit(Kind::Out, text.as_str()),
        ApplyEvent::Engine {
            module,
            engine,
            reason,
        } => {
            if reason.is_empty() {
                emit(Kind::Info, format!("{module}: {engine} engine"));
            } else {
                emit(Kind::Info, format!("{module}: {engine} engine ({reason})"));
            }
        }
        _ => {}
    }
}

/// where an apply's events are handed as they happen.
pub type Emit<'a> = &'a (dyn Fn(&ApplyEvent) + Sync);

/// sets the flag when dropped, whether the work ended or unwound.
struct Stop<'a>(&'a std::sync::atomic::AtomicBool);

impl Drop for Stop<'_> {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// run `work` and pass the apply events it publishes to `emit` as they happen. the log reaches
/// every thread's lines, so only events are taken; errors and notes go to standard error.
pub fn with_events<T>(emit: Emit, work: impl FnOnce() -> T) -> T {
    use std::sync::atomic::{AtomicBool, Ordering};
    let lines = crate::log::subscribe();
    let done = AtomicBool::new(false);
    let forward = |line: crate::log::Line| match line.kind {
        Kind::Event => {
            if let Ok(event) = serde_json::from_str::<ApplyEvent>(&line.text) {
                emit(&event);
            }
        }
        Kind::Err | Kind::Info => eprintln!("{}", line.text),
        _ => {}
    };
    std::thread::scope(|scope| {
        let (done, forward) = (&done, &forward);
        let forwarder = scope.spawn(move || {
            while !done.load(Ordering::SeqCst) {
                if let Ok(line) = lines.recv_timeout(std::time::Duration::from_millis(50)) {
                    forward(line);
                }
            }
            lines.try_iter().for_each(forward);
        });
        // set even when the work panics, or the forwarder would never stop and the scope never end
        let result = {
            let _stop = Stop(done);
            work()
        };
        let _ = forwarder.join();
        result
    })
}

/// one line of an operation's answer: an apply event, as json.
pub fn event_line(event: &ApplyEvent) -> Option<Vec<u8>> {
    let mut line = serde_json::to_vec(&serde_json::json!({ "event": event })).ok()?;
    line.push(b'\n');
    Some(line)
}

/// one apply at a time: the dialogs share one progress state, so two runs would mix their
/// events. the flag is released when the guard drops, on every way out. (under test each
/// thread has its own flag, because the tests run side by side.)
#[cfg(not(test))]
static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
thread_local! {
    static BUSY: &'static std::sync::atomic::AtomicBool =
        Box::leak(Box::new(std::sync::atomic::AtomicBool::new(false)));
}

struct Busy(&'static std::sync::atomic::AtomicBool);

impl Busy {
    fn take() -> Result<Self> {
        #[cfg(not(test))]
        let flag: &'static std::sync::atomic::AtomicBool = &BUSY;
        #[cfg(test)]
        let flag: &'static std::sync::atomic::AtomicBool = BUSY.with(|f| *f);
        if flag.swap(true, std::sync::atomic::Ordering::SeqCst) {
            bail!("an apply is already running");
        }
        Ok(Self(flag))
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

use lanlord_model::engine::{mode_for, Mode, ModulePlan};

/// one stretch of an apply: modules for Ansible together, or one module for the engine.
#[derive(Debug, PartialEq)]
enum Part<'a> {
    Ansible(Vec<&'a str>),
    /// the engine looks at a module Ansible just applied.
    Shadow(&'a str),
    Native(&'a str),
}

/// the parts of an apply in module order: neighbours for Ansible share a run, a shadowed module is
/// Ansible's run followed by the engine's look, a native module stands alone.
fn split<'a>(wanted: &[&'a str], mode_of: impl Fn(&str) -> Mode) -> Vec<Part<'a>> {
    let mut parts: Vec<Part> = Vec::new();
    for &module in wanted {
        match mode_of(module) {
            Mode::Native => parts.push(Part::Native(module)),
            Mode::Shadow => {
                parts.push(Part::Ansible(vec![module]));
                parts.push(Part::Shadow(module));
            }
            Mode::Ansible => match parts.last_mut() {
                Some(Part::Ansible(batch)) => batch.push(module),
                _ => parts.push(Part::Ansible(vec![module])),
            },
        }
    }
    parts
}

/// pick the engine per module. when every module is Ansible's (the default) this is the same
/// single run as ever; otherwise the apply is split in parts that run in order.
fn dispatch(
    session: &Session,
    admin: &str,
    tags: &[&str],
    settings: Option<Vec<u8>>,
    ask_password: AskPassword,
    on_line: &mut dyn FnMut(&str),
) -> Result<()> {
    let parsed = settings
        .as_deref()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|text| lanlord_model::settings::Settings::parse(text).ok());
    let wanted: Vec<&str> = if tags.is_empty() {
        crate::engine::modules::ORDER.to_vec()
    } else {
        tags.to_vec()
    };
    let mode_of = |module: &str| {
        if session.is_local() && crate::engine::modules::has_native(module) {
            mode_for(parsed.as_ref(), module)
        } else {
            Mode::Ansible
        }
    };
    if wanted.iter().all(|m| mode_of(m) == Mode::Ansible) {
        return run_tags(session, admin, tags, settings, ask_password, on_line);
    }
    let parts = split(&wanted, mode_of);
    run_parts(
        session,
        admin,
        tags,
        &parts,
        settings,
        ask_password,
        on_line,
    )
}

fn add_stats(total: &mut apply::Stats, more: apply::Stats) {
    total.ok += more.ok;
    total.changed += more.changed;
    total.failed += more.failed;
    total.skipped += more.skipped;
    total.unreachable += more.unreachable;
}

fn run_parts(
    session: &Session,
    admin: &str,
    tags: &[&str],
    parts: &[Part],
    settings: Option<Vec<u8>>,
    ask_password: AskPassword,
    on_line: &mut dyn FnMut(&str),
) -> Result<()> {
    let _busy = Busy::take()?;
    crate::env::gate(session, admin, tags, settings.as_deref(), ask_password)?;
    // the settings are checked before anything runs, as for a single run
    script(settings.as_deref(), tags)?;
    emit(
        Kind::Info,
        format!(
            "using your settings ({} bytes)",
            settings.as_ref().map_or(0, Vec::len)
        ),
    );
    let doc: serde_yaml_ng::Value = match &settings {
        Some(plain) => serde_yaml_ng::from_slice(plain)?,
        None => serde_yaml_ng::Value::Null,
    };
    publish(&ApplyEvent::Started);
    let total = std::cell::RefCell::new(apply::Stats::default());
    // one run of the playbooks for `tags`; its own start and end are folded into this apply's
    let mut ansible = |tags: &[&str], on_line: &mut dyn FnMut(&str)| -> Result<()> {
        let script = script(settings.as_deref(), tags)?;
        let ended = std::cell::Cell::new(false);
        let mut sink = |line: &str| match apply::parse_line(line) {
            Some(ApplyEvent::Started) => {}
            Some(ApplyEvent::Finished { stats }) => {
                ended.set(true);
                add_stats(&mut total.borrow_mut(), stats);
            }
            Some(event) => publish(&event),
            None => on_line(line),
        };
        let ran = run_as_root_lines(session, admin, script, ask_password, &mut sink);
        if !ended.get() && !matches!(ran, Ok(0)) {
            total.borrow_mut().failed += 1;
        }
        match ran? {
            0 => Ok(()),
            NOT_INSTALLED_STATUS => bail!(NOT_INSTALLED_MESSAGE),
            code => bail!("apply failed with status {code}"),
        }
    };
    let run_id = format!(
        "{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        std::process::id()
    );
    let host = crate::engine::host::RealHost;
    let mut outcome: Result<()> = Ok(());
    for part in parts {
        let step = match part {
            Part::Ansible(tags) => ansible(tags, on_line),
            Part::Shadow(module) => {
                publish(&ApplyEvent::Engine {
                    module: (*module).into(),
                    engine: Mode::Shadow.as_str().into(),
                    reason: "Ansible applied it; the engine compares".into(),
                });
                match crate::engine::modules::plan(
                    module,
                    &host,
                    &doc,
                    crate::engine::STATE_DIR,
                    true,
                ) {
                    Ok(left) => {
                        for event in crate::engine::modules::shadow_lines(module, &left) {
                            publish(&event);
                        }
                    }
                    Err(error) => emit(Kind::Info, format!("engine shadow: {module}: {error}")),
                }
                Ok(())
            }
            Part::Native(module) => {
                publish(&ApplyEvent::Engine {
                    module: (*module).into(),
                    engine: Mode::Native.as_str().into(),
                    reason: String::new(),
                });
                let done = crate::engine::modules::apply(
                    module,
                    &host,
                    &doc,
                    crate::engine::STATE_DIR,
                    &run_id,
                    &mut |event| publish(&event),
                );
                match done {
                    Ok(tally) => {
                        add_stats(&mut total.borrow_mut(), tally.stats());
                        Ok(())
                    }
                    Err(error) => match error.downcast_ref::<crate::engine::Unsupported>() {
                        Some(why) => {
                            publish(&ApplyEvent::Engine {
                                module: (*module).into(),
                                engine: Mode::Ansible.as_str().into(),
                                reason: why.to_string(),
                            });
                            ansible(&[module], on_line)
                        }
                        None => {
                            total.borrow_mut().failed += 1;
                            Err(error)
                        }
                    },
                }
            }
        };
        if step.is_err() {
            outcome = step;
            break;
        }
    }
    publish(&ApplyEvent::Finished {
        stats: total.into_inner(),
    });
    outcome?;
    emit(Kind::Info, "apply finished");
    Ok(())
}

/// what an apply of `modules` would do, per module, from the settings on this gateway. only the
/// engine can say; a module that runs with Ansible is listed as such.
pub fn plan(
    session: &Session,
    identity: Option<&age::x25519::Identity>,
    modules: &[String],
    ask_password: AskPassword,
) -> Result<Vec<ModulePlan>> {
    if !session.is_local() {
        bail!("a preview is made on the gateway itself");
    }
    let plain = settings::merged_with(session, identity, ask_password)?;
    let parsed = plain
        .as_deref()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|text| lanlord_model::settings::Settings::parse(text).ok());
    let doc: serde_yaml_ng::Value = match &plain {
        Some(bytes) => serde_yaml_ng::from_slice(bytes)?,
        None => serde_yaml_ng::Value::Null,
    };
    let wanted: Vec<String> = if modules.is_empty() {
        crate::engine::modules::ORDER
            .iter()
            .filter(|m| crate::engine::modules::has_native(m))
            .map(|m| m.to_string())
            .collect()
    } else {
        modules.to_vec()
    };
    let host = crate::engine::host::RealHost;
    let mut out = Vec::new();
    for module in wanted {
        if self::modules::find(&module).is_none() {
            bail!("unknown module: {module}");
        }
        let mode = if crate::engine::modules::has_native(&module) {
            mode_for(parsed.as_ref(), &module)
        } else {
            Mode::Ansible
        };
        let mut entry = ModulePlan {
            module: module.clone(),
            engine: Mode::Ansible.as_str().into(),
            reason: "runs with Ansible; preview not available".into(),
            changes: Vec::new(),
        };
        if mode != Mode::Ansible {
            match crate::engine::modules::plan(
                &module,
                &host,
                &doc,
                crate::engine::STATE_DIR,
                false,
            ) {
                Ok(changes) => {
                    entry.engine = mode.as_str().into();
                    entry.reason = String::new();
                    entry.changes = changes;
                }
                Err(error) => match error.downcast_ref::<crate::engine::Unsupported>() {
                    Some(why) => entry.reason = format!("runs with Ansible: {why}"),
                    None => return Err(error),
                },
            }
        }
        out.push(entry);
    }
    Ok(out)
}

fn run_tags(
    session: &Session,
    admin: &str,
    tags: &[&str],
    settings: Option<Vec<u8>>,
    ask_password: AskPassword,
    on_line: &mut dyn FnMut(&str),
) -> Result<()> {
    let _busy = Busy::take()?;
    // refuse early on a machine that definitely cannot run docker, with the fix; the settings
    // say which modules a plain apply runs
    crate::env::gate(session, admin, tags, settings.as_deref(), ask_password)?;
    // everything that can fail before the run starts is done before the dialog opens
    let script = script(settings.as_deref(), tags)?;
    match &settings {
        Some(plain) => emit(
            Kind::Info,
            format!("using your settings ({} bytes)", plain.len()),
        ),
        None => emit(
            Kind::Info,
            "no settings to apply; running the baseline only",
        ),
    }
    publish(&ApplyEvent::Started);
    // whether the run printed its own end
    let ended = std::cell::Cell::new(false);
    let mut sink = |line: &str| match apply::parse_line(line) {
        Some(event) => {
            ended.set(ended.get() || matches!(event, ApplyEvent::Finished { .. }));
            publish(&event)
        }
        None => on_line(line),
    };
    let ran = run_as_root_lines(session, admin, script, ask_password, &mut sink);
    if !ended.get() {
        // the run stopped without its own summary (it never started, ssh failed, it was
        // killed), so close the dialog; no exit status at all counts as failed too
        let failed = u32::from(!matches!(ran, Ok(0)));
        publish(&ApplyEvent::Finished {
            stats: apply::Stats {
                failed,
                ..Default::default()
            },
        });
    }
    match ran? {
        0 => {
            emit(Kind::Info, "apply finished");
            Ok(())
        }
        NOT_INSTALLED_STATUS => bail!(NOT_INSTALLED_MESSAGE),
        code => bail!("apply failed with status {code}"),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::crypto;
    use base64::Engine;

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("lanlord-apply-{}-{name}", std::process::id()))
    }

    fn identity(name: &str) -> age::x25519::Identity {
        let path = temp(name).join("identity");
        crypto::generate(&path).unwrap();
        let identity = crypto::load(&path).unwrap();
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        identity
    }

    fn no_password(_: &str) -> Result<String> {
        anyhow::bail!("no password expected")
    }

    /// a fake gateway that serves `cipher` (base64) to the settings fetch
    /// and records the apply script it is sent.
    fn gateway_with_settings(cipher: &str, record: &std::path::Path, exit: i32) -> Session {
        Session::fake(&format!(
            r#"for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sudo -n true") exit 0 ;;
  *)
    input=$(cat)
    case "$input" in
      *"base64 < /opt/lanlord/config/settings.age"*) printf '%s\n' '{cipher}' ;;
      *) printf '%s' "$input" > '{path}'; exit {exit} ;;
    esac ;;
esac"#,
            path = record.display(),
        ))
    }

    #[test]
    fn script_unpacks_and_runs_the_playbook_as_lanlord() {
        let s = script(None, &[]).unwrap();
        assert!(s.contains("base64 -d <<'LANLORD_PAYLOAD'"));
        assert!(s.contains("sudo -n -u lanlord tar -x"));
        assert!(
            s.contains("PYTHONDONTWRITEBYTECODE=1"),
            "the venv is root-owned"
        );
        assert!(s.contains("ANSIBLE_STDOUT_CALLBACK=lanlord_progress"));
        assert!(s.contains("ANSIBLE_CALLBACK_PLUGINS=/opt/lanlord/playbooks/callback_plugins"));
        assert!(s.contains(&format!(
            "ansible-playbook -i localhost, -e lanlord_image_version={} /opt/lanlord/playbooks/site.yml",
            env!("LANLORD_IMAGE_VERSION")
        )));
        assert_eq!(s.matches("LANLORD_PAYLOAD").count(), 2);
        assert!(!s.contains("settings.yml"), "no settings, no extra vars");
    }

    #[test]
    fn script_stages_settings_in_run_and_removes_them() {
        let s = script(Some(b"gateway:\n  motd: hi\n"), &[]).unwrap();
        assert!(s.contains("install -d -o lanlord -g lanlord -m 0700 /run/lanlord"));
        assert!(s.contains("trap 'rm -f /run/lanlord/settings.yml' EXIT"));
        assert!(s.contains("chmod 0600 /run/lanlord/settings.yml"));
        assert!(s.contains("-e @/run/lanlord/settings.yml"));
        assert!(
            !s.contains("motd: hi"),
            "settings are base64, not inline text"
        );
    }

    #[test]
    fn the_staged_settings_mark_strings_unsafe_and_refuse_jinja_and_bad_values() {
        use base64::Engine;
        let staged = |settings: &[u8]| -> Result<String> {
            let s = script(Some(settings), &[])?;
            let body: String = s
                .lines()
                .skip_while(|l| !l.contains("<<'LANLORD_SETTINGS'"))
                .skip(1)
                .take_while(|l| *l != "LANLORD_SETTINGS")
                .collect();
            let bytes = base64::engine::general_purpose::STANDARD.decode(body)?;
            Ok(String::from_utf8(bytes)?)
        };
        let written = staged(b"gateway:\n  motd: \"{{ lookup('pipe', 'id') }}\"\n").unwrap();
        assert!(written.contains("!unsafe"), "{written}");
        for bad in [
            &b"gateway:\n  timezone: ../../etc\n"[..],
            b"image: evil\n",
            b"nt4-domain:\n  users:\n    - name: \"x\\ny\"\n",
            b"\xff\xfe",
        ] {
            assert!(script(Some(bad), &[]).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_image_version_is_x_y_z() {
        let parts: Vec<_> = env!("LANLORD_IMAGE_VERSION").split('.').collect();
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| p.parse::<u32>().is_ok()));
    }

    #[test]
    fn script_passes_the_version_after_the_settings() {
        let s = script(Some(b"gateway:\n  ntp_enabled: true\n"), &[]).unwrap();
        let settings = s.find("-e @/run/lanlord/settings.yml").unwrap();
        let version = s.find("-e lanlord_image_version=").unwrap();
        assert!(settings < version, "settings must not override the version");
    }

    #[test]
    fn scripts_are_valid_sh() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        for settings in [None, Some(&b"gateway:\n  ntp_enabled: true\n"[..])] {
            let mut child = Command::new("sh")
                .arg("-n")
                .stdin(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(script(settings, &[]).unwrap().as_bytes())
                .unwrap();
            let out = child.wait_with_output().unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    #[test]
    fn apply_without_a_key_runs_the_baseline() {
        let record = temp("nokey");
        apply_with(
            &Session::fake_gateway(true, &record, 0),
            "gw",
            None,
            &[],
            &mut no_password,
        )
        .unwrap();
        let sent = std::fs::read_to_string(&record).unwrap();
        assert!(sent.starts_with("set -eu"));
        assert!(!sent.contains("LANLORD_SETTINGS"));
        let _ = std::fs::remove_file(record);
    }

    #[test]
    fn apply_with_a_key_and_no_stored_settings_runs_the_baseline() {
        let id = identity("empty");
        let record = temp("nosettings");
        let session = gateway_with_settings("", &record, 0);
        apply_with(&session, "gw", Some(&id), &[], &mut no_password).unwrap();
        assert!(!std::fs::read_to_string(&record)
            .unwrap()
            .contains("LANLORD_SETTINGS"));
        let _ = std::fs::remove_file(record);
    }

    #[test]
    fn apply_sends_the_decrypted_settings_for_the_run() {
        let id = identity("settings");
        let sealed = crypto::encrypt(&id, b"gateway:\n  motd: hello\n").unwrap();
        let cipher = base64::engine::general_purpose::STANDARD.encode(sealed);
        let record = temp("withsettings");
        let session = gateway_with_settings(&cipher, &record, 0);
        apply_with(&session, "gw", Some(&id), &[], &mut no_password).unwrap();
        let sent = std::fs::read_to_string(&record).unwrap();
        assert!(sent.contains("-e @/run/lanlord/settings.yml"));
        let body: String = sent
            .lines()
            .skip_while(|l| !l.contains("<<'LANLORD_SETTINGS'"))
            .skip(1)
            .take_while(|l| *l != "LANLORD_SETTINGS")
            .collect();
        let plain = base64::engine::general_purpose::STANDARD
            .decode(body)
            .unwrap();
        let plain = String::from_utf8(plain).unwrap();
        assert_eq!(plain, "gateway:\n  motd: !unsafe hello\n");
        let _ = std::fs::remove_file(record);
    }

    #[test]
    fn apply_makes_the_exec_password_first_for_a_domain_and_not_for_other_modules() {
        use crate::testgw::Gateway;
        let id = identity("execpw");
        let gw = Gateway::new("execpw");
        gw.seed_plain("nt4-servers:\n  - {name: a, role: pdc, domain: LAB, netbios_name: ONE, network: lab}\ndomains:\n  LAB: {}\n");
        apply_with(
            &gw.session,
            "gw",
            Some(&id),
            &["dns".into()],
            &mut no_password,
        )
        .unwrap();
        assert!(
            gw.sealed(&id).is_none(),
            "a dns apply leaves the account alone"
        );
        apply_with(&gw.session, "gw", Some(&id), &[], &mut no_password).unwrap();
        let sealed = gw.sealed(&id).unwrap();
        assert!(sealed.contains("exec_password"));
        apply_with(&gw.session, "gw", Some(&id), &[], &mut no_password).unwrap();
        assert_eq!(gw.sealed(&id).unwrap(), sealed, "the password is kept");
    }

    #[test]
    fn plugin_lines_become_events_and_the_rest_stays_output() {
        let record = temp("events");
        let events = [
            r#"LANLORD_EVENT {"event":"total","n":1}"#,
            r#"LANLORD_EVENT {"event":"task_done","name":"t","status":"ok","changed":true,"msg":""}"#,
        ];
        let rx = crate::log::subscribe();
        let session = Session::fake(&format!(
            r#"for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sudo -n true") exit 0 ;;
  "sh -c "*) ;;
  *) cat >'{path}'; echo '{a}'; echo 'plain output'; echo '{b}' ;;
esac"#,
            path = record.display(),
            a = events[0],
            b = events[1],
        ));
        let mut raw = Vec::new();
        apply_lines(&session, "gw", None, &[], &mut no_password, &mut |l| {
            raw.push(l.to_string())
        })
        .unwrap();
        assert_eq!(raw, ["plain output"], "events do not reach the plain sink");
        let seen: Vec<_> = rx
            .try_iter()
            .filter(|l| l.kind == Kind::Event)
            .map(|l| l.text)
            .collect();
        assert!(seen.iter().any(|t| t.contains(r#""event":"total""#)));
        assert!(seen.iter().any(|t| t.contains(r#""event":"task_done""#)));
        let _ = std::fs::remove_file(record);
    }

    #[test]
    fn apply_asks_for_the_sudo_password_when_needed() {
        let record = temp("pass");
        let mut ask = |_: &str| Ok("hunter2".to_string());
        apply_with(
            &Session::fake_gateway(false, &record, 0),
            "gw",
            None,
            &[],
            &mut ask,
        )
        .unwrap();
        assert!(std::fs::read_to_string(&record)
            .unwrap()
            .starts_with("hunter2\nset -eu"));
        let _ = std::fs::remove_file(record);
    }

    #[test]
    fn a_failing_playbook_is_an_error() {
        let record = temp("fail");
        let err = apply_with(
            &Session::fake_gateway(true, &record, 2),
            "gw",
            None,
            &[],
            &mut no_password,
        )
        .unwrap_err();
        assert!(err.to_string().contains("status 2"), "{err}");
        let _ = std::fs::remove_file(record);
    }

    #[test]
    fn modules_become_tags() {
        let s = script(None, &["gateway", "docker"]).unwrap();
        assert!(s.contains(" --tags gateway,docker /opt/lanlord/playbooks/site.yml"));
        let with_settings = script(Some(b"gateway:\n  ntp_enabled: true\n"), &["gateway"]).unwrap();
        assert!(with_settings.contains(" --tags gateway /opt/lanlord/playbooks/site.yml"));
    }

    #[test]
    fn apply_sends_only_the_chosen_module() {
        let record = temp("tags");
        apply_with(
            &Session::fake_gateway(true, &record, 0),
            "gw",
            None,
            &["gateway".to_string()],
            &mut no_password,
        )
        .unwrap();
        assert!(std::fs::read_to_string(&record)
            .unwrap()
            .contains("--tags gateway"));
        let _ = std::fs::remove_file(record);
    }

    #[test]
    fn unknown_modules_are_refused_before_connecting() {
        let record = temp("unknown");
        let err = apply_with(
            &Session::fake_gateway(true, &record, 0),
            "gw",
            None,
            &["x; rm -rf /".to_string()],
            &mut no_password,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown module"), "{err}");
        assert!(!record.exists());
    }

    #[test]
    fn apply_stops_plainly_when_lanlord_is_not_installed() {
        assert!(script(None, &[]).unwrap().contains("exit 66"));
        // only the apply script itself stops with the guard's status
        let session = Session::fake(
            r#"for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sudo -n true") exit 0 ;;
  "sh -c "*) ;;
  *) cat >/dev/null; exit 66 ;;
esac"#,
        );
        let err = apply_with(&session, "gw", None, &[], &mut no_password).unwrap_err();
        assert_eq!(
            err.to_string(),
            "LANlord is not installed on this server yet"
        );
    }

    fn done(name: &str, status: TaskStatus, changed: bool, msg: &str) -> ApplyEvent {
        ApplyEvent::TaskDone {
            name: name.into(),
            status,
            changed,
            msg: msg.into(),
        }
    }

    #[test]
    fn every_task_result_reads_plainly_in_the_log() {
        let rx = crate::log::subscribe();
        for event in [
            done("pub-ok", TaskStatus::Ok, false, ""),
            done("pub-changed", TaskStatus::Ok, true, ""),
            done("pub-skipped", TaskStatus::Skipped, false, ""),
            done("pub-failed", TaskStatus::Failed, false, "no space"),
            done("pub-down", TaskStatus::Unreachable, false, ""),
            ApplyEvent::Log {
                text: "pub-said".into(),
            },
            ApplyEvent::Total { n: 1 },
        ] {
            publish(&event);
        }
        let seen: Vec<(Kind, String)> = rx
            .try_iter()
            .filter(|l| l.text.contains("pub-"))
            .map(|l| (l.kind, l.text))
            .collect();
        let info = |text: &str| (Kind::Info, text.to_string());
        for expected in [
            info("ok: pub-ok"),
            info("changed: pub-changed"),
            info("skipped: pub-skipped"),
            info("FAILED: pub-failed: no space"),
            info("UNREACHABLE: pub-down"),
            (Kind::Out, "pub-said".to_string()),
        ] {
            assert!(seen.contains(&expected), "{expected:?} in {seen:?}");
        }
        let events = seen.iter().filter(|(k, _)| *k == Kind::Event).count();
        assert_eq!(events, 6, "the total has no pub- in it; the others do");
    }

    /// a gateway whose apply prints `lines` and exits with `code`.
    fn gateway_printing(lines: &[&str], code: i32) -> Session {
        let echoes: String = lines.iter().map(|l| format!("echo '{l}'\n")).collect();
        Session::fake(&format!(
            r#"for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sudo -n true") exit 0 ;;
  "sh -c "*) ;;
  *) cat >/dev/null; {echoes} exit {code} ;;
esac"#
        ))
    }

    /// the ends this thread published since the last call.
    fn finished_events() -> Vec<ApplyEvent> {
        PUBLISHED
            .with(|p| std::mem::take(&mut *p.borrow_mut()))
            .into_iter()
            .filter(|e| matches!(e, ApplyEvent::Finished { .. }))
            .collect()
    }

    /// one test for all three ends, because subscribers see every thread's events.
    #[test]
    fn a_run_ends_once_with_its_own_summary_or_a_synthetic_one() {
        let total = r#"LANLORD_EVENT {"event":"total","n":2}"#;
        let end = r#"LANLORD_EVENT {"event":"finished","stats":{"ok":1,"changed":0,"failed":0,"skipped":0,"unreachable":0}}"#;
        // cut short: the dialog is closed with a failure
        finished_events();
        let err = apply_with(
            &gateway_printing(&[total], 2),
            "gw",
            None,
            &[],
            &mut no_password,
        )
        .unwrap_err();
        assert!(err.to_string().contains("status 2"), "{err}");
        let cut = finished_events();
        assert!(matches!(
            cut.as_slice(),
            [ApplyEvent::Finished { stats }] if stats.failed == 1
        ));
        // no exit status at all (ssh status 255): closed with a failure as well
        finished_events();
        assert!(apply_with(
            &gateway_printing(&[total], 255),
            "gw",
            None,
            &[],
            &mut no_password,
        )
        .is_err());
        assert!(matches!(
            finished_events().as_slice(),
            [ApplyEvent::Finished { stats }] if stats.failed == 1
        ));
        // its own end: not closed a second time
        finished_events();
        apply_with(
            &gateway_printing(&[total, end], 0),
            "gw",
            None,
            &[],
            &mut no_password,
        )
        .unwrap();
        let own = finished_events();
        assert!(matches!(
            own.as_slice(),
            [ApplyEvent::Finished { stats }] if stats.failed == 0 && stats.ok == 1
        ));
        // no events at all: the dialog was opened, so it is closed all the same
        finished_events();
        apply_with(
            &gateway_printing(&["plain output"], 0),
            "gw",
            None,
            &[],
            &mut no_password,
        )
        .unwrap();
        assert!(matches!(
            finished_events().as_slice(),
            [ApplyEvent::Finished { stats }] if stats.failed == 0
        ));
        // not installed: closed as failed
        finished_events();
        assert!(apply_with(
            &gateway_printing(&[], NOT_INSTALLED_STATUS),
            "gw",
            None,
            &[],
            &mut no_password,
        )
        .is_err());
        assert!(matches!(
            finished_events().as_slice(),
            [ApplyEvent::Finished { stats }] if stats.failed == 1
        ));
    }

    #[test]
    fn a_second_apply_is_refused_while_one_runs_and_allowed_after() {
        let held = Busy::take().unwrap();
        let err =
            apply_with(&gateway_printing(&[], 0), "gw", None, &[], &mut no_password).unwrap_err();
        assert!(err.to_string().contains("already running"), "{err}");
        drop(held);
        apply_with(&gateway_printing(&[], 0), "gw", None, &[], &mut no_password).unwrap();
        // and the flag is let go after a failed run too
        assert!(apply_with(&gateway_printing(&[], 3), "gw", None, &[], &mut no_password).is_err());
        drop(Busy::take().expect("released after the failed run"));
    }

    #[test]
    fn an_apply_is_split_so_each_engine_runs_in_module_order() {
        let mode = |m: &str| match m {
            "nt4-domain" => Mode::Native,
            "dns" => Mode::Shadow,
            _ => Mode::Ansible,
        };
        assert_eq!(
            split(
                &["docker", "network", "dns", "dhcp", "nt4-domain", "time"],
                mode
            ),
            [
                Part::Ansible(vec!["docker", "network"]),
                Part::Ansible(vec!["dns"]),
                Part::Shadow("dns"),
                Part::Ansible(vec!["dhcp"]),
                Part::Native("nt4-domain"),
                Part::Ansible(vec!["time"]),
            ]
        );
        assert_eq!(
            split(&["a", "b"], |_| Mode::Ansible),
            [Part::Ansible(vec!["a", "b"])]
        );
    }
}
