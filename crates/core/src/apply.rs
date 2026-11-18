use anyhow::{bail, Result};

use lanlord_model::apply::{self, ApplyEvent, TaskStatus};

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
    let (stage, vars) = match settings {
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
    let settings = settings::merged_with(session, identity, ask_password)?;
    run_tags(session, admin, &tags, settings, ask_password, on_line)
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
        _ => {}
    }
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
        assert!(s.contains("ansible-playbook -i localhost, /opt/lanlord/playbooks/site.yml"));
        assert_eq!(s.matches("LANLORD_PAYLOAD").count(), 2);
        assert!(!s.contains("settings.yml"), "no settings, no extra vars");
    }

    #[test]
    fn script_stages_settings_in_run_and_removes_them() {
        let s = script(Some(b"motd: hi\n"), &[]).unwrap();
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
    fn scripts_are_valid_sh() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        for settings in [None, Some(&b"a: 1\n"[..])] {
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
        let sealed = crypto::encrypt(&id, b"motd: hello\n").unwrap();
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
        assert_eq!(plain, b"motd: hello\n");
        let _ = std::fs::remove_file(record);
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
        assert!(s.contains("-i localhost, --tags gateway,docker /opt/lanlord/playbooks/site.yml"));
        let with_settings = script(Some(b"a: 1\n"), &["gateway"]).unwrap();
        assert!(with_settings.contains("-e @/run/lanlord/settings.yml --tags gateway"));
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
}
