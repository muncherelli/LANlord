//! privileged accounts on a gateway: who is signed in, and creating an administrator, which
//! is what lanlord recommends instead of working as root.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;

use crate::admins::ADMINS_GROUP;
use crate::bootstrap::{install_sudoers, run_as_root, valid_user, AskPassword, ROOT_ADVICE};
use crate::log::{emit, Kind};
use crate::ssh::Session;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// the gid of `lanlord-admins`, as bootstrap creates it.
const ADMINS_ID: u32 = 9998;

/// the longest public key line accepted.
const MAX_KEY: usize = 8192;

/// who the ssh session is signed in as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginUser {
    pub name: String,
    pub is_root: bool,
}

impl LoginUser {
    /// what front ends say to someone signed in as root.
    pub fn advice(&self) -> Option<&'static str> {
        self.is_root.then_some(ROOT_ADVICE)
    }
}

/// the user the ssh session is signed in as, and whether that is root.
pub fn login_user(session: &Session) -> Result<LoginUser> {
    let out = session.capture(&["sh".into(), "-c".into(), "id -un; id -u".into()])?;
    let mut lines = out.lines().map(str::trim);
    let name = lines.next().filter(|n| !n.is_empty());
    let uid = lines.next();
    match (name, uid) {
        (Some(name), Some(uid)) => Ok(LoginUser {
            name: name.to_string(),
            is_root: uid == "0",
        }),
        _ => bail!("the gateway did not say who is signed in"),
    }
}

/// a name for a new administrator: a plain login name that is neither root nor lanlord's own.
fn check_name(name: &str) -> Result<()> {
    if !valid_user(name) || name.len() > 32 {
        bail!("not a valid user name: {name:?} (lowercase letters, digits, - and _, up to 32)");
    }
    if matches!(name, "root" | "lanlord") {
        bail!("{name} cannot be created as an administrator account");
    }
    Ok(())
}

/// a password for chpasswd: one line, nothing that would end it early.
fn check_password(password: &str) -> Result<()> {
    if password.is_empty() {
        bail!("the password is empty");
    }
    if password
        .chars()
        .any(|c| c == '\n' || c == '\r' || c == '\0')
    {
        bail!("the password cannot contain line breaks");
    }
    Ok(())
}

/// one public key line, such as `ssh-ed25519 AAAA... comment`.
fn check_key(key: &str) -> Result<()> {
    let ok_type = ["ssh-", "ecdsa-", "sk-ssh-", "sk-ecdsa-"]
        .iter()
        .any(|p| key.starts_with(p));
    let data = key.split_whitespace().nth(1).unwrap_or("");
    let base64 = !data.is_empty()
        && data
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='));
    if !ok_type || !base64 || key.len() > MAX_KEY || key.chars().any(char::is_control) {
        bail!("that does not look like one ssh public key line");
    }
    Ok(())
}

/// read a public key from a file, or from standard input for `-`; one line, trimmed.
pub fn read_key(source: &str) -> Result<String> {
    let text = if source == "-" {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
        text
    } else {
        std::fs::read_to_string(source).with_context(|| format!("could not read {source}"))?
    };
    let key = text.trim().to_string();
    if key.lines().count() != 1 {
        bail!("expected exactly one public key");
    }
    check_key(&key)?;
    Ok(key)
}

/// the sh script (run as root) that creates the administrator `name`. the password and the
/// key travel base64 encoded inside the script, never as an argument, and the password
/// reaches chpasswd on its stdin.
fn create_script(
    name: &str,
    password: &str,
    ssh_public_key: Option<&str>,
    passwordless_sudo: bool,
) -> Result<String> {
    check_name(name)?;
    check_password(password)?;
    if let Some(key) = ssh_public_key {
        check_key(key)?;
    }
    let admins = ADMINS_GROUP;
    let pw = B64.encode(password);
    let mut s = format!(
        r#"set -eu
if id -u {name} >/dev/null 2>&1; then
  echo "the user {name} already exists" >&2; exit 1
fi
getent group {admins} >/dev/null || groupadd --gid {ADMINS_ID} {admins}
groups={admins}
for g in sudo wheel; do
  if getent group "$g" >/dev/null; then groups="$groups,$g"; break; fi
done
useradd --create-home --shell "$(command -v bash || echo /bin/sh)" --groups "$groups" {name}
pw=$(printf %s '{pw}' | base64 -d)
printf '%s:%s\n' {name} "$pw" | chpasswd
unset pw
"#
    );
    if let Some(key) = ssh_public_key {
        let key = B64.encode(key);
        s.push_str(&format!(
            r#"home=$(getent passwd {name} | cut -d: -f6)
install -d -o {name} -g {name} -m 0700 "$home/.ssh"
tmp=$(mktemp)
printf '%s\n' "$(printf %s '{key}' | base64 -d)" > "$tmp"
install -o {name} -g {name} -m 0600 "$tmp" "$home/.ssh/authorized_keys"
rm -f "$tmp"
"#
        ));
    }
    if passwordless_sudo {
        // named to sort after lanlord-admins, so this rule wins
        s.push_str(&install_sudoers(
            &format!("lanlord-nopasswd-{name}"),
            &format!("{name} ALL=(ALL) NOPASSWD: ALL\n"),
        ));
    }
    s.push_str(&format!("echo \"created {name}\"\n"));
    Ok(s)
}

/// create the administrator `name` on `host`: a login with `password`, a member of
/// `lanlord-admins` and of the distro's sudo group, with `ssh_public_key` as an authorized
/// key when given. passwordless sudo is only set up when `passwordless_sudo` asks for it.
pub fn create_admin(
    session: &Session,
    host: &str,
    name: &str,
    password: &str,
    ssh_public_key: Option<&str>,
    passwordless_sudo: bool,
    ask_password: AskPassword,
) -> Result<()> {
    let script = create_script(name, password, ssh_public_key, passwordless_sudo)?;
    let login = login_user(session)?;
    if let Some(advice) = login.advice() {
        emit(Kind::Info, advice);
    }
    emit(
        Kind::Info,
        format!("creating the administrator {name} on {host}"),
    );
    match run_as_root(session, &login.name, script, ask_password)? {
        0 => {
            emit(Kind::Info, format!("created the administrator {name}"));
            Ok(())
        }
        code => Err(anyhow!("creating {name} failed with status {code}")),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("lanlord-account-{}-{name}", std::process::id()))
    }

    fn valid_sh(script: &str) {
        use std::io::Write;
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
            .write_all(script.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGoZ4HMhG4uZ+G4cDf8iUKvmCZ8F8RAV90ZwPN1B0bJTT me@laptop";

    #[test]
    fn the_script_creates_the_account_and_hides_the_password() {
        let s = create_script("olive", "pa'ss$word", Some(KEY), false).unwrap();
        assert!(s.contains("useradd --create-home"));
        assert!(s.contains("groups=lanlord-admins"));
        assert!(s.contains("groupadd --gid 9998 lanlord-admins"));
        assert!(s.contains("for g in sudo wheel"));
        assert!(s.contains("chpasswd"));
        assert!(
            !s.contains("pa'ss$word"),
            "the password is only sent encoded"
        );
        assert!(!s.contains("me@laptop"), "the key is only sent encoded");
        assert!(s.contains("-m 0700") && s.contains("-m 0600"));
        assert!(!s.contains("NOPASSWD"), "passwordless sudo is opt in");
        valid_sh(&s);
    }

    #[test]
    fn passwordless_sudo_and_the_key_are_optional() {
        let plain = create_script("olive", "x", None, false).unwrap();
        assert!(!plain.contains("authorized_keys") && !plain.contains("sudoers.d"));
        let nopass = create_script("olive", "x", None, true).unwrap();
        assert!(nopass.contains("/etc/sudoers.d/lanlord-nopasswd-olive"));
        assert!(nopass.contains("olive ALL=(ALL) NOPASSWD: ALL"));
        assert!(nopass.contains("visudo -cf"));
        valid_sh(&nopass);
    }

    #[test]
    fn unsafe_input_is_refused() {
        for bad in ["", "Root", "root", "lanlord", "a b", "a;rm", "1abc", "x'y"] {
            assert!(create_script(bad, "x", None, false).is_err(), "{bad}");
        }
        assert!(create_script(&"a".repeat(33), "x", None, false).is_err());
        assert!(create_script("olive", "", None, false).is_err());
        assert!(create_script("olive", "a\nb", None, false).is_err());
        for bad in [
            "",
            "hello world",
            "ssh-ed25519",
            "ssh-ed25519 AAAA\nssh-rsa BBBB",
            "ssh-ed25519 AAAA'; rm -rf /",
        ] {
            assert!(
                create_script("olive", "x", Some(bad), false).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn keys_are_read_from_a_file() {
        let path = temp("key");
        std::fs::write(&path, format!("{KEY}\n")).unwrap();
        assert_eq!(read_key(path.to_str().unwrap()).unwrap(), KEY);
        std::fs::write(&path, format!("{KEY}\n{KEY}\n")).unwrap();
        assert!(read_key(path.to_str().unwrap()).is_err());
        std::fs::write(&path, "not a key\n").unwrap();
        assert!(read_key(path.to_str().unwrap()).is_err());
        let _ = std::fs::remove_file(&path);
        assert!(read_key(path.to_str().unwrap()).is_err());
    }

    fn who(name: &str, uid: &str) -> Session {
        Session::fake(&format!(
            r#"for a; do last=$a; done
case "$last" in
  "sh -c 'id -un; id -u'") printf '%s\n%s\n' {name} {uid} ;;
  "sudo -n true") exit 0 ;;
  *) cat >/dev/null ;;
esac"#
        ))
    }

    #[test]
    fn login_user_knows_root() {
        let root = login_user(&who("root", "0")).unwrap();
        assert_eq!(
            root,
            LoginUser {
                name: "root".into(),
                is_root: true
            }
        );
        assert!(root.advice().unwrap().contains("recommend creating"));
        let admin = login_user(&who("olive", "1000")).unwrap();
        assert!(!admin.is_root && admin.advice().is_none());
        let empty = Session::fake("exit 0");
        assert!(login_user(&empty).is_err());
    }

    fn recording(user: &str, uid: &str, record: &std::path::Path, exit: i32) -> Session {
        Session::fake(&format!(
            r#"for a; do last=$a; done
case "$last" in
  "sh -c 'id -un; id -u'") printf '%s\n%s\n' {user} {uid} ;;
  "sudo -n true") exit 0 ;;
  *) cat > '{path}'; exit {exit} ;;
esac"#,
            path = record.display()
        ))
    }

    #[test]
    fn create_admin_runs_the_script_as_root() {
        let record = temp("create");
        let mut ask = |_: &str| anyhow::bail!("no password expected");
        let session = recording("tester", "1000", &record, 0);
        create_admin(&session, "gw", "olive", "pw", Some(KEY), true, &mut ask).unwrap();
        let sent = std::fs::read_to_string(&record).unwrap();
        assert!(sent.contains("useradd --create-home") && sent.contains("lanlord-nopasswd-olive"));
        let _ = std::fs::remove_file(&record);

        // as root nothing goes through sudo
        let root = recording("root", "0", &record, 0);
        create_admin(&root, "gw", "olive", "pw", None, false, &mut ask).unwrap();
        assert!(std::fs::read_to_string(&record)
            .unwrap()
            .starts_with("set -eu"));
        let _ = std::fs::remove_file(&record);

        let failing = recording("tester", "1000", &record, 3);
        let err = create_admin(&failing, "gw", "olive", "pw", None, false, &mut ask).unwrap_err();
        assert!(err.to_string().contains("status 3"), "{err}");
        let _ = std::fs::remove_file(&record);

        // bad input never reaches the gateway
        assert!(create_admin(&session, "gw", "root", "pw", None, false, &mut ask).is_err());
        assert!(!record.exists());
    }
}
