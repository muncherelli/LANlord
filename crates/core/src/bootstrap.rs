use std::process::Command as Local;

use anyhow::{bail, Result};

use crate::ssh::Session;

/// where ansible runs for this gateway.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnsibleMode {
    /// use ansible installed on this machine, over our own ssh.
    Local,
    /// install ansible in a venv on the gateway and run it there.
    Remote,
}

impl AnsibleMode {
    /// local when ansible-playbook is on the path, otherwise remote.
    pub fn detect() -> Self {
        let found = Local::new("ansible-playbook")
            .arg("--version")
            .output()
            .is_ok_and(|out| out.status.success());
        if found {
            Self::Local
        } else {
            Self::Remote
        }
    }
}

pub struct Options {
    pub host: String,
    pub ansible: AnsibleMode,
    /// let the management user run sudo without a password.
    pub passwordless_sudo: bool,
}

/// the unix account that owns /opt/lanlord and runs ansible on the gateway.
const SERVICE_ID: u32 = 9999;

/// a unix account name safe to put in a sudoers file and a shell script.
fn valid_user(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// the posix sh script that sets up the gateway; run as root.
pub fn script(opts: &Options, admin: &str) -> Result<String> {
    if !valid_user(admin) {
        bail!("unexpected management user name: {admin:?}");
    }
    let id = SERVICE_ID;
    let mut s = format!(
        r#"set -eu
. /etc/os-release
case " $ID $ID_LIKE " in
  *" arch "*) pm=pacman ;;
  *" debian "*|*" ubuntu "*) pm=apt ;;
  *) echo "unsupported distro: $ID" >&2; exit 1 ;;
esac

# refuse to continue if {id} belongs to someone else
owner=$(getent group {id} | cut -d: -f1 || true)
if [ -n "$owner" ] && [ "$owner" != lanlord ]; then
  echo "gid {id} is already used by group $owner" >&2; exit 1
fi
owner=$(getent passwd {id} | cut -d: -f1 || true)
if [ -n "$owner" ] && [ "$owner" != lanlord ]; then
  echo "uid {id} is already used by user $owner" >&2; exit 1
fi

if [ "$pm" = pacman ]; then
  pacman -S --needed --noconfirm python python-pip sudo
else
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y -qq python3 python3-venv python3-pip sudo
fi

getent group lanlord >/dev/null || groupadd --system --gid {id} lanlord
getent passwd lanlord >/dev/null || useradd --system --uid {id} --gid {id} \
  --home-dir /opt/lanlord --no-create-home \
  --shell "$(command -v nologin || echo /usr/sbin/nologin)" lanlord
install -d -o lanlord -g lanlord -m 0750 /opt/lanlord /opt/lanlord/config

# lanlord runs ansible, which needs root; it has no password and no login
tmp=$(mktemp)
echo 'lanlord ALL=(ALL) NOPASSWD: ALL' > "$tmp"
visudo -cf "$tmp" >/dev/null
install -m 0440 "$tmp" /etc/sudoers.d/lanlord
rm -f "$tmp"
"#
    );
    if opts.passwordless_sudo {
        s.push_str(&format!(
            r#"
tmp=$(mktemp)
echo '{admin} ALL=(ALL) NOPASSWD: ALL' > "$tmp"
visudo -cf "$tmp" >/dev/null
install -m 0440 "$tmp" /etc/sudoers.d/lanlord-admin
rm -f "$tmp"
"#
        ));
    }
    if opts.ansible == AnsibleMode::Remote {
        s.push_str(
            r#"
if [ ! -x /opt/lanlord/venv/bin/ansible-playbook ]; then
  sudo -u lanlord python3 -m venv /opt/lanlord/venv
  sudo -u lanlord /opt/lanlord/venv/bin/pip install --quiet ansible-core
fi
"#,
        );
    }
    s.push_str("echo lanlord bootstrap complete\n");
    Ok(s)
}

/// `sudo` followed by `args`, as a remote command.
fn sudo(args: &[&str]) -> Vec<String> {
    std::iter::once("sudo")
        .chain(args.iter().copied())
        .map(ToString::to_string)
        .collect()
}

/// the remote command and stdin that run `script` as root; with a password,
/// sudo reads it from the first line of stdin.
fn plan(script: String, password: Option<&str>) -> (Vec<String>, String) {
    match password {
        None => (sudo(&["-n", "sh", "-s"]), script),
        Some(password) => (
            sudo(&["-S", "-p", "", "sh", "-s"]),
            format!("{password}\n{script}"),
        ),
    }
}

/// set up the gateway on `opts.host`; the ssh user needs sudo.
pub fn bootstrap(opts: &Options) -> Result<()> {
    let session = Session::new(&opts.host)?;
    let admin = session.capture(&["id".into(), "-un".into()])?;
    let admin = admin.trim();
    let script = script(opts, admin)?;

    // sudo -n fails when a password is needed; then read it once and feed it to sudo -S
    let password = if session.capture(&sudo(&["-n", "true"])).is_ok() {
        None
    } else {
        Some(rpassword::prompt_password(format!(
            "sudo password for {admin}: "
        ))?)
    };
    let (remote, input) = plan(script, password.as_deref());
    match session.run_with_stdin(&remote, input.as_bytes())? {
        0 => Ok(()),
        code => bail!("bootstrap failed with status {code}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(ansible: AnsibleMode, passwordless_sudo: bool) -> Options {
        Options {
            host: "gw".into(),
            ansible,
            passwordless_sudo,
        }
    }

    #[test]
    fn creates_the_9999_service_account() {
        let s = script(&opts(AnsibleMode::Local, false), "admin").unwrap();
        assert!(s.contains("groupadd --system --gid 9999 lanlord"));
        assert!(s.contains("useradd --system --uid 9999 --gid 9999"));
        assert!(s.contains("/etc/sudoers.d/lanlord"));
    }

    #[test]
    fn refuses_when_9999_is_taken() {
        let s = script(&opts(AnsibleMode::Local, false), "admin").unwrap();
        assert!(s.contains("gid 9999 is already used by group"));
        assert!(s.contains("uid 9999 is already used by user"));
    }

    #[test]
    fn passwordless_sudo_is_opt_in() {
        let off = script(&opts(AnsibleMode::Local, false), "admin").unwrap();
        let on = script(&opts(AnsibleMode::Local, true), "admin").unwrap();
        assert!(!off.contains("lanlord-admin"));
        assert!(on.contains("admin ALL=(ALL) NOPASSWD: ALL"));
    }

    #[test]
    fn remote_mode_installs_ansible_in_a_venv() {
        let remote = script(&opts(AnsibleMode::Remote, false), "admin").unwrap();
        let local = script(&opts(AnsibleMode::Local, false), "admin").unwrap();
        assert!(remote.contains("pip install --quiet ansible-core"));
        assert!(!local.contains("ansible-core"));
    }

    #[test]
    fn plan_without_a_password_uses_noninteractive_sudo() {
        let (remote, input) = plan("echo hi\n".into(), None);
        assert_eq!(remote, ["sudo", "-n", "sh", "-s"]);
        assert_eq!(input, "echo hi\n");
    }

    #[test]
    fn plan_with_a_password_sends_it_first() {
        let (remote, input) = plan("echo hi\n".into(), Some("hunter2"));
        assert_eq!(remote, ["sudo", "-S", "-p", "", "sh", "-s"]);
        assert_eq!(input, "hunter2\necho hi\n");
    }

    #[test]
    fn detect_picks_a_mode() {
        assert!(matches!(
            AnsibleMode::detect(),
            AnsibleMode::Local | AnsibleMode::Remote
        ));
    }

    #[test]
    fn rejects_unsafe_user_names() {
        for bad in ["", "Root", "a b", "a;rm", "1abc", "x'y"] {
            assert!(
                script(&opts(AnsibleMode::Local, true), bad).is_err(),
                "{bad}"
            );
        }
        assert!(script(&opts(AnsibleMode::Local, true), "deploy-user_1").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn generated_script_is_valid_sh() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        for ansible in [AnsibleMode::Local, AnsibleMode::Remote] {
            for passwordless in [false, true] {
                let s = script(&opts(ansible, passwordless), "admin").unwrap();
                let mut child = Command::new("sh")
                    .arg("-n")
                    .stdin(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap();
                child.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
                let out = child.wait_with_output().unwrap();
                assert!(
                    out.status.success(),
                    "{}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
        }
    }
}
