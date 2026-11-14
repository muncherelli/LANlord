use anyhow::{bail, Result};

use crate::bootstrap::{run_as_root, AskPassword};
use crate::log::{emit, Kind};
use crate::payload;
use crate::ssh::Session;

/// the shell script that unpacks the playbooks and runs them as lanlord.
fn script() -> Result<String> {
    Ok(format!(
        r#"set -eu
if [ ! -x /opt/lanlord/venv/bin/ansible-playbook ]; then
  echo "ansible is not installed on the gateway; run lanlord bootstrap --ansible remote first" >&2
  exit 1
fi
install -d -o lanlord -g lanlord -m 0750 /opt/lanlord/playbooks
base64 -d <<'LANLORD_PAYLOAD' | sudo -n -u lanlord tar -x -C /opt/lanlord/playbooks
{payload}
LANLORD_PAYLOAD
cd /opt/lanlord
sudo -n -u lanlord env HOME=/opt/lanlord ANSIBLE_NOCOLOR=1 \
  ANSIBLE_PYTHON_INTERPRETER=auto_silent ANSIBLE_LOCAL_TEMP=/opt/lanlord/.ansible/tmp \
  /opt/lanlord/venv/bin/ansible-playbook -i localhost, /opt/lanlord/playbooks/site.yml
"#,
        payload = payload::archive_base64()?
    ))
}

/// push the playbooks to `host` and run them there as the lanlord user.
pub fn apply(host: &str, ask_password: AskPassword) -> Result<()> {
    apply_with(&Session::new(host)?, host, ask_password)
}

fn apply_with(session: &Session, host: &str, ask_password: AskPassword) -> Result<()> {
    emit(Kind::Info, format!("applying to {host}"));
    let admin = session.capture(&["id".into(), "-un".into()])?;
    match run_as_root(session, admin.trim(), script()?, ask_password)? {
        0 => {
            emit(Kind::Info, "apply finished");
            Ok(())
        }
        code => bail!("apply failed with status {code}"),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("lanlord-apply-{}-{name}", std::process::id()))
    }

    #[test]
    fn script_unpacks_and_runs_the_playbook_as_lanlord() {
        let s = script().unwrap();
        assert!(s.contains("base64 -d <<'LANLORD_PAYLOAD'"));
        assert!(s.contains("sudo -n -u lanlord tar -x"));
        assert!(s.contains("ansible-playbook -i localhost, /opt/lanlord/playbooks/site.yml"));
        assert_eq!(s.matches("LANLORD_PAYLOAD").count(), 2);
    }

    #[test]
    fn script_is_valid_sh() {
        use std::io::Write;
        use std::process::{Command, Stdio};
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
            .write_all(script().unwrap().as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn apply_sends_the_script_as_root() {
        let record = temp("ok");
        let mut ask = |_: &str| Ok(String::new());
        apply_with(&Session::fake_gateway(true, &record, 0), "gw", &mut ask).unwrap();
        let sent = std::fs::read_to_string(&record).unwrap();
        assert!(sent.starts_with("set -eu"));
        assert!(sent.contains("LANLORD_PAYLOAD"));
        let _ = std::fs::remove_file(record);
    }

    #[test]
    fn apply_asks_for_the_sudo_password_when_needed() {
        let record = temp("pass");
        let mut ask = |_: &str| Ok("hunter2".to_string());
        apply_with(&Session::fake_gateway(false, &record, 0), "gw", &mut ask).unwrap();
        assert!(std::fs::read_to_string(&record)
            .unwrap()
            .starts_with("hunter2\nset -eu"));
        let _ = std::fs::remove_file(record);
    }

    #[test]
    fn a_failing_playbook_is_an_error() {
        let record = temp("fail");
        let mut ask = |_: &str| Ok(String::new());
        let err = apply_with(&Session::fake_gateway(true, &record, 2), "gw", &mut ask).unwrap_err();
        assert!(err.to_string().contains("status 2"), "{err}");
        let _ = std::fs::remove_file(record);
    }
}
