use anyhow::{bail, Result};

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

fn run_tags(
    session: &Session,
    admin: &str,
    tags: &[&str],
    settings: Option<Vec<u8>>,
    ask_password: AskPassword,
    on_line: &mut dyn FnMut(&str),
) -> Result<()> {
    // refuse early on a machine that definitely cannot run docker, with the fix; the settings
    // say which modules a plain apply runs
    crate::env::gate(session, admin, tags, settings.as_deref(), ask_password)?;
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
    match run_as_root_lines(
        session,
        admin,
        script(settings.as_deref(), tags)?,
        ask_password,
        on_line,
    )? {
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
        assert!(s.contains(&format!(
            "ansible-playbook -i localhost, -e lanlord_image_version={} /opt/lanlord/playbooks/site.yml",
            env!("LANLORD_IMAGE_VERSION")
        )));
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
    fn the_image_version_is_x_y_z() {
        let parts: Vec<_> = env!("LANLORD_IMAGE_VERSION").split('.').collect();
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| p.parse::<u32>().is_ok()));
    }

    #[test]
    fn script_passes_the_version_after_the_settings() {
        let s = script(Some(b"a: 1\n"), &[]).unwrap();
        let settings = s.find("-e @/run/lanlord/settings.yml").unwrap();
        let version = s.find("-e lanlord_image_version=").unwrap();
        assert!(settings < version, "settings must not override the version");
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
        let with_settings = script(Some(b"a: 1\n"), &["gateway"]).unwrap();
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
}
