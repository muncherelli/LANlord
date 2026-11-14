//! the encrypted settings stored on a gateway. it only ever holds
//! ciphertext; the key stays on the client.

use anyhow::{bail, Context, Result};
use base64::Engine;

use crate::bootstrap::{capture_as_root, run_as_root, AskPassword};
use crate::crypto;
use crate::log::{emit, Kind};
use crate::ssh::Session;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

fn wrap(bytes: &[u8]) -> String {
    B64.encode(bytes)
        .as_bytes()
        .chunks(76)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

fn push_script(ciphertext: &[u8]) -> String {
    format!(
        r#"set -eu
install -d -o lanlord -g lanlord -m 0750 /opt/lanlord/config
base64 -d <<'LANLORD_PAYLOAD' | sudo -n -u lanlord tee /opt/lanlord/config/settings.age >/dev/null
{}
LANLORD_PAYLOAD
chmod 0640 /opt/lanlord/config/settings.age
"#,
        wrap(ciphertext)
    )
}

const PULL_SCRIPT: &str = r#"set -eu
if [ ! -f /opt/lanlord/config/settings.age ]; then
  echo "no settings on this gateway; run lanlord config push first" >&2
  exit 3
fi
base64 < /opt/lanlord/config/settings.age
"#;

fn admin(session: &Session) -> Result<String> {
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

fn push_with(
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

/// fetch the gateway's settings and decrypt them here.
pub fn pull(
    host: &str,
    identity: &age::x25519::Identity,
    ask_password: AskPassword,
) -> Result<Vec<u8>> {
    pull_with(&Session::new(host)?, identity, ask_password)
}

fn pull_with(
    session: &Session,
    identity: &age::x25519::Identity,
    ask_password: AskPassword,
) -> Result<Vec<u8>> {
    let admin = admin(session)?;
    let encoded = capture_as_root(session, &admin, PULL_SCRIPT.to_string(), ask_password)?;
    let compact: String = encoded.split_whitespace().collect();
    let sealed = B64
        .decode(compact)
        .context("the gateway returned damaged settings")?;
    crypto::decrypt(identity, &sealed)
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
            b"motd: hi\n"
        );
    }

    #[test]
    fn pull_explains_when_nothing_is_stored() {
        let id = identity("none");
        let session = serving("echo 'no settings on this gateway' >&2", 3);
        let err = pull_with(&session, &id, &mut no_password).unwrap_err();
        assert!(err.to_string().contains("no settings"), "{err}");
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
}
