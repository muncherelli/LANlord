//! the encrypted settings stored on a gateway. it only ever holds
//! ciphertext; the key stays on the client.

use anyhow::{bail, Context, Result};
use base64::Engine;

use crate::admins::ADMINS_GROUP;
use crate::bootstrap::{capture_as_root, run_as_root, AskPassword};
use crate::crypto;
use crate::log::{emit, Kind};
use crate::ssh::Session;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

pub(crate) fn wrap(bytes: &[u8]) -> String {
    B64.encode(bytes)
        .as_bytes()
        .chunks(76)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

/// the config directory belongs to the admins group when that group exists, so
/// admins can read the plain settings; the age file inside stays lanlord only.
fn config_dir() -> String {
    format!(
        r#"group=lanlord
if getent group {ADMINS_GROUP} >/dev/null 2>&1; then group={ADMINS_GROUP}; fi
install -d -o lanlord -g "$group" -m 0750 /opt/lanlord/config
"#
    )
}

fn push_script(ciphertext: &[u8]) -> String {
    let config_dir = config_dir();
    format!(
        r#"set -eu
{config_dir}base64 -d <<'LANLORD_PAYLOAD' | sudo -n -u lanlord tee /opt/lanlord/config/settings.age >/dev/null
{}
LANLORD_PAYLOAD
chmod 0640 /opt/lanlord/config/settings.age
"#,
        wrap(ciphertext)
    )
}

fn push_plain_script(plaintext: &[u8]) -> String {
    let config_dir = config_dir();
    format!(
        r#"set -eu
{config_dir}base64 -d <<'LANLORD_PAYLOAD' | sudo -n -u lanlord tee /opt/lanlord/config/settings.yml >/dev/null
{}
LANLORD_PAYLOAD
chgrp "$group" /opt/lanlord/config/settings.yml
chmod 0640 /opt/lanlord/config/settings.yml
"#,
        wrap(plaintext)
    )
}

/// prints `present` and then the file, or nothing when there is no plain file.
/// a directory that cannot be searched is a failure, not an absent file, so the
/// caller falls back to sudo.
const PULL_PLAIN_SCRIPT: &str = r#"if [ -f /opt/lanlord/config/settings.yml ]; then
  echo present
  cat /opt/lanlord/config/settings.yml
else
  err=$(ls /opt/lanlord/config 2>&1 >/dev/null) || case "$err" in
    *"No such file"*) ;;
    *) echo "$err" >&2; exit 1 ;;
  esac
fi
"#;

const PULL_SCRIPT: &str = r#"set -eu
# prints nothing when no settings are stored yet
if [ -f /opt/lanlord/config/settings.age ]; then
  base64 < /opt/lanlord/config/settings.age
fi
"#;

pub(crate) fn admin(session: &Session) -> Result<String> {
    Ok(session
        .capture(&["id".into(), "-un".into()])?
        .trim()
        .to_string())
}

/// encrypt `plaintext` with `identity` and store it on the gateway.
pub fn push(
    host: &str,
    identity: &age::x25519::Identity,
    plaintext: &[u8],
    ask_password: AskPassword,
) -> Result<()> {
    push_with(
        &Session::new(host)?,
        host,
        identity,
        plaintext,
        ask_password,
    )
}

pub(crate) fn push_with(
    session: &Session,
    host: &str,
    identity: &age::x25519::Identity,
    plaintext: &[u8],
    ask_password: AskPassword,
) -> Result<()> {
    if plaintext.iter().all(u8::is_ascii_whitespace) {
        bail!("the settings file is empty");
    }
    let sealed = crypto::encrypt(identity, plaintext)?;
    emit(Kind::Info, format!("storing encrypted settings on {host}"));
    let admin = admin(session)?;
    match run_as_root(session, &admin, push_script(&sealed), ask_password)? {
        0 => {
            emit(Kind::Info, "settings stored");
            Ok(())
        }
        code => bail!("storing the settings failed with status {code}"),
    }
}

/// fetch the gateway's settings and decrypt them here; `None` when nothing is stored.
pub fn pull(
    host: &str,
    identity: &age::x25519::Identity,
    ask_password: AskPassword,
) -> Result<Option<Vec<u8>>> {
    pull_with(&Session::new(host)?, identity, ask_password)
}

pub(crate) fn pull_with(
    session: &Session,
    identity: &age::x25519::Identity,
    ask_password: AskPassword,
) -> Result<Option<Vec<u8>>> {
    let admin = admin(session)?;
    let encoded = capture_as_root(session, &admin, PULL_SCRIPT.to_string(), ask_password)?;
    let compact: String = encoded.split_whitespace().collect();
    if compact.is_empty() {
        return Ok(None);
    }
    let sealed = B64
        .decode(compact)
        .context("the gateway returned damaged settings")?;
    crypto::decrypt(identity, &sealed).map(Some)
}

/// store the plain (unencrypted, readable) settings on the gateway.
pub fn push_plain(host: &str, plaintext: &[u8], ask_password: AskPassword) -> Result<()> {
    push_plain_with(&Session::new(host)?, host, plaintext, ask_password)
}

pub(crate) fn push_plain_with(
    session: &Session,
    host: &str,
    plaintext: &[u8],
    ask_password: AskPassword,
) -> Result<()> {
    emit(Kind::Info, format!("storing plain settings on {host}"));
    let admin = admin(session)?;
    match run_as_root(session, &admin, push_plain_script(plaintext), ask_password)? {
        0 => Ok(()),
        code => bail!("storing the plain settings failed with status {code}"),
    }
}

/// the plain settings on the gateway; `None` when there are none.
pub fn pull_plain(host: &str, ask_password: AskPassword) -> Result<Option<Vec<u8>>> {
    pull_plain_with(&Session::new(host)?, ask_password)
}

/// admins read the file directly; sudo is the fallback for accounts not in the group yet.
pub(crate) fn pull_plain_with(
    session: &Session,
    ask_password: AskPassword,
) -> Result<Option<Vec<u8>>> {
    let direct = session.capture(&["sh".into(), "-c".into(), PULL_PLAIN_SCRIPT.into()]);
    let output = match direct {
        Ok(output) => output,
        Err(_) => {
            let admin = admin(session)?;
            capture_as_root(session, &admin, PULL_PLAIN_SCRIPT.to_string(), ask_password)?
        }
    };
    Ok(output
        .strip_prefix("present\n")
        .map(|body| body.as_bytes().to_vec()))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn identity(name: &str) -> age::x25519::Identity {
        let path = std::env::temp_dir()
            .join(format!("lanlord-config-{}-{name}", std::process::id()))
            .join("identity");
        crypto::generate(&path).unwrap();
        let identity = crypto::load(&path).unwrap();
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        identity
    }

    fn no_password(_: &str) -> Result<String> {
        bail!("no password expected")
    }

    /// the base64 payload inside a pushed script, decoded.
    fn payload(script: &str) -> Vec<u8> {
        let body: String = script
            .lines()
            .skip_while(|l| !l.contains("<<'LANLORD_PAYLOAD'"))
            .skip(1)
            .take_while(|l| *l != "LANLORD_PAYLOAD")
            .collect();
        B64.decode(body).unwrap()
    }

    #[test]
    fn push_stores_only_ciphertext() {
        let id = identity("push");
        let record =
            std::env::temp_dir().join(format!("lanlord-config-push-{}", std::process::id()));
        let session = Session::fake_gateway(true, &record, 0);
        push_with(&session, "gw", &id, b"motd: hello\n", &mut no_password).unwrap();
        let sent = std::fs::read_to_string(&record).unwrap();
        assert!(!sent.contains("motd"), "plaintext leaked into the script");
        assert!(sent.contains("/opt/lanlord/config/settings.age"));
        assert_eq!(
            crypto::decrypt(&id, &payload(&sent)).unwrap(),
            b"motd: hello\n"
        );
        let _ = std::fs::remove_file(record);
    }

    #[test]
    fn push_refuses_an_empty_file() {
        let id = identity("empty");
        let record =
            std::env::temp_dir().join(format!("lanlord-config-empty-{}", std::process::id()));
        let session = Session::fake_gateway(true, &record, 0);
        assert!(push_with(&session, "gw", &id, b" \n", &mut no_password).is_err());
        assert!(!record.exists());
    }

    #[test]
    fn push_reports_a_failed_script() {
        let id = identity("fail");
        let record =
            std::env::temp_dir().join(format!("lanlord-config-fail-{}", std::process::id()));
        let session = Session::fake_gateway(true, &record, 5);
        let err = push_with(&session, "gw", &id, b"a: 1\n", &mut no_password).unwrap_err();
        assert!(err.to_string().contains("status 5"), "{err}");
        let _ = std::fs::remove_file(record);
    }

    fn serving(body: &str, exit: i32) -> Session {
        Session::fake(&format!(
            r#"for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sudo -n true") exit 0 ;;
  *) cat >/dev/null; {body}; exit {exit} ;;
esac"#
        ))
    }

    #[test]
    fn pull_decrypts_what_the_gateway_returns() {
        let id = identity("pull");
        let sealed = crypto::encrypt(&id, b"motd: hi\n").unwrap();
        let session = serving(
            &format!("printf '%s\\n' '{}'", wrap(&sealed).replace('\n', "' '")),
            0,
        );
        assert_eq!(
            pull_with(&session, &id, &mut no_password).unwrap(),
            Some(b"motd: hi\n".to_vec())
        );
    }

    #[test]
    fn pull_returns_none_when_nothing_is_stored() {
        let id = identity("none");
        let session = serving("true", 0);
        assert!(pull_with(&session, &id, &mut no_password)
            .unwrap()
            .is_none());
    }

    #[test]
    fn pull_rejects_damaged_data() {
        let id = identity("damaged");
        let session = serving("echo '!!!not base64!!!'", 0);
        assert!(pull_with(&session, &id, &mut no_password).is_err());
    }

    #[test]
    fn pull_needs_the_right_key() {
        let (mine, other) = (identity("mine"), identity("other"));
        let sealed = crypto::encrypt(&other, b"secret: 1\n").unwrap();
        let session = serving(&format!("printf '%s\\n' '{}'", B64.encode(&sealed)), 0);
        assert!(pull_with(&session, &mine, &mut no_password).is_err());
    }

    #[test]
    fn push_plain_sends_readable_settings_for_the_admins_group() {
        let record =
            std::env::temp_dir().join(format!("lanlord-config-plain-{}", std::process::id()));
        let session = Session::fake_gateway(true, &record, 0);
        push_plain_with(&session, "gw", b"dns: {}\n", &mut no_password).unwrap();
        let sent = std::fs::read_to_string(&record).unwrap();
        assert!(sent.contains("tee /opt/lanlord/config/settings.yml"));
        assert!(sent.contains("chmod 0640 /opt/lanlord/config/settings.yml"));
        assert!(sent.contains("admins"));
        assert_eq!(payload(&sent), b"dns: {}\n");
        let _ = std::fs::remove_file(&record);
        let failing = Session::fake_gateway(true, &record, 4);
        let err = push_plain_with(&failing, "gw", b"a: 1\n", &mut no_password).unwrap_err();
        assert!(err.to_string().contains("status 4"), "{err}");
        let _ = std::fs::remove_file(&record);
    }

    #[test]
    fn pull_plain_reads_the_file_or_nothing() {
        let present = serving("printf 'present\\nmotd: hi\\n'", 0);
        assert_eq!(
            pull_plain_with(&present, &mut no_password).unwrap(),
            Some(b"motd: hi\n".to_vec())
        );
        assert!(pull_plain_with(&serving("true", 0), &mut no_password)
            .unwrap()
            .is_none());
    }

    #[test]
    fn pull_plain_falls_back_to_sudo_when_the_direct_read_fails() {
        // the direct `sh -c` read is refused; the sudo run answers
        let session = Session::fake(
            r#"for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sudo -n true") exit 0 ;;
  sh*) echo denied >&2; exit 1 ;;
  *) cat >/dev/null; printf 'present\nviasudo: 1\n' ;;
esac"#,
        );
        assert_eq!(
            pull_plain_with(&session, &mut no_password).unwrap(),
            Some(b"viasudo: 1\n".to_vec())
        );
    }

    #[test]
    fn pull_plain_script_fails_when_the_directory_cannot_be_searched() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("lanlord-plain-{}", std::process::id()));
        let script = PULL_PLAIN_SCRIPT.replace("/opt/lanlord/config", dir.to_str().unwrap());
        let run = || Command::new("sh").arg("-c").arg(&script).output().unwrap();
        assert!(
            run().status.success(),
            "a missing directory is just no file"
        );
        std::fs::create_dir_all(&dir).unwrap();
        assert!(run().status.success() && run().stdout.is_empty());
        std::fs::write(dir.join("settings.yml"), "a: 1\n").unwrap();
        assert_eq!(run().stdout, b"present\na: 1\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
