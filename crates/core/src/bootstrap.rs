use std::process::Command as Local;

use anyhow::{bail, Result};

use crate::admins::{ADMINS_GID, ADMINS_GROUP};
use crate::log::{emit, Kind};
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

/// how long sudo remembers the password of a `admins` member, in minutes.
pub(crate) const SUDO_MINUTES: u32 = 15;

/// the sudoers rules for `admins`: they may use sudo, and one password prompt is
/// remembered for `SUDO_MINUTES` across all the ssh sessions lanlord opens (they have no tty,
/// so the default per-tty record would never be reused).
pub(crate) fn admins_sudoers() -> String {
    format!(
        "# written by lanlord\nDefaults:%{ADMINS_GROUP} timestamp_type=global\nDefaults:%{ADMINS_GROUP} timestamp_timeout={SUDO_MINUTES}\n%{ADMINS_GROUP} ALL=(ALL:ALL) ALL\n"
    )
}

/// the shell that writes `rules` to /etc/sudoers.d/`file` after `visudo` has checked them.
/// `rules` must contain no single quote.
pub(crate) fn install_sudoers(file: &str, rules: &str) -> String {
    format!(
        r#"tmp=$(mktemp)
printf '%s' '{rules}' > "$tmp"
visudo -cf "$tmp" >/dev/null
install -m 0440 "$tmp" /etc/sudoers.d/{file}
rm -f "$tmp"
"#
    )
}

/// shell that refuses to continue when `id` belongs to an account other than `name`.
fn id_taken_check(kind: &str, id_name: &str, id: u32, name: &str) -> String {
    let db = if kind == "user" { "passwd" } else { kind };
    format!(
        r#"owner=$(getent {db} {id} | cut -d: -f1 || true)
if [ -n "$owner" ] && [ "$owner" != {name} ]; then
  echo "{id_name} {id} is already used by {kind} $owner" >&2; exit 1
fi
"#
    )
}

/// shell that creates the lanlord group and account, or brings an old one into line: the id
/// and the home directory are fixed, and files under /opt/lanlord that the old ids owned follow.
fn service_account(id: u32) -> String {
    format!(
        r#"old_uid=$(getent passwd lanlord | cut -d: -f3 || true)
old_gid=$(getent group lanlord | cut -d: -f3 || true)
if [ -n "$old_uid" ] && [ "$old_uid" != {id} ] && pids=$(pgrep -u "$old_uid" | tr '\n' ' ') && [ -n "$pids" ]; then
  echo "the lanlord account is in use by processes $pids; stop them and run this again" >&2
  exit 1
fi
# files the old ids owned move first, so an interrupted run still finds the old ids next time
if [ -d /opt/lanlord ]; then
  if [ -n "$old_uid" ] && [ "$old_uid" != {id} ]; then
    find /opt/lanlord -xdev -uid "$old_uid" -exec chown -h {id} {{}} +
  fi
  if [ -n "$old_gid" ] && [ "$old_gid" != {id} ]; then
    find /opt/lanlord -xdev -gid "$old_gid" -exec chgrp -h {id} {{}} +
  fi
fi
if [ -z "$old_gid" ]; then
  groupadd --gid {id} lanlord
elif [ "$old_gid" != {id} ]; then
  groupmod --gid {id} lanlord
fi
if [ -z "$old_uid" ]; then
  useradd --uid {id} --gid {id} --home-dir /opt/lanlord --no-create-home \
    --shell "$(command -v nologin || echo /usr/sbin/nologin)" lanlord
else
  # the home moves before the uid, because usermod --uid re-owns the files in the current home
  if [ "$(getent passwd lanlord | cut -d: -f6)" != /opt/lanlord ]; then
    usermod --home /opt/lanlord lanlord
  fi
  if [ "$(getent passwd lanlord | cut -d: -f4)" != {id} ]; then
    usermod --gid {id} lanlord
  fi
  if [ "$old_uid" != {id} ]; then
    usermod --uid {id} lanlord
  fi
fi
"#
    )
}

/// a unix account name safe to put in a sudoers file and a shell script.
pub(crate) fn valid_user(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// what to tell someone who signs in as root.
pub const ROOT_ADVICE: &str = "you are logged in as root; that is fine, but we recommend creating a privileged user account (lanlord admin HOST create NAME)";

/// the posix sh script that sets up the gateway; run as root.
pub fn script(opts: &Options, admin: &str) -> Result<String> {
    if !valid_user(admin) {
        bail!("unexpected management user name: {admin:?}");
    }
    let id = SERVICE_ID;
    let admins = ADMINS_GROUP;
    let admins_id = ADMINS_GID;
    let id_checks = format!(
        r#"{}{}{}gid=$(getent group {admins} | cut -d: -f3 || true)
if [ -n "$gid" ] && [ "$gid" != {admins_id} ]; then
  echo "group {admins} already exists with gid $gid" >&2; exit 1
fi
"#,
        id_taken_check("group", "gid", id, "lanlord"),
        id_taken_check("user", "uid", id, "lanlord"),
        id_taken_check("group", "gid", admins_id, admins),
    );
    // root needs no group membership or sudo rules
    let is_root = admin == "root";
    let join_admins = if is_root {
        String::new()
    } else {
        format!("usermod -aG {admins} {admin}\n")
    };
    let mut s = format!(
        r#"set -eu
. /etc/os-release
case " ${{ID:-}} ${{ID_LIKE:-}} " in
  *" arch "*) pm=pacman ;;
  *" debian "*|*" ubuntu "*) pm=apt ;;
  *) echo "unsupported distro: ${{ID:-unknown}}" >&2; exit 1 ;;
esac

# refuse to continue if {id} or {admins_id} belong to someone else
{id_checks}
if [ "$pm" = pacman ]; then
  pacman -S --needed --noconfirm python python-pip sudo
else
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y -qq python3 python3-venv python3-pip sudo
fi

{service_account}getent group {admins} >/dev/null || groupadd --gid {admins_id} {admins}
{join_admins}# root owns /opt/lanlord so lanlord cannot swap out the venv; it owns only what it writes
install -d -o root -g lanlord -m 0750 /opt/lanlord
install -d -o lanlord -g lanlord -m 0750 /opt/lanlord/config /opt/lanlord/.ansible /opt/lanlord/state

# lanlord runs ansible, which needs root; it has no password and no login
{service_sudoers}
# the people allowed to administer lanlord may use sudo, with one remembered password prompt
{admins_sudoers}"#,
        service_account = service_account(id),
        service_sudoers = install_sudoers("lanlord", "lanlord ALL=(ALL) NOPASSWD: ALL\n"),
        admins_sudoers = install_sudoers(ADMINS_GROUP, &admins_sudoers()),
    );
    // the per-user rule is named so that it sorts after admins: the last match wins
    if opts.passwordless_sudo && !is_root {
        s.push('\n');
        s.push_str(&install_sudoers(
            &format!("lanlord-nopasswd-{admin}"),
            &format!("{admin} ALL=(ALL) NOPASSWD: ALL\n"),
        ));
    }
    if opts.ansible == AnsibleMode::Remote {
        s.push_str(
            r#"
if [ ! -x /opt/lanlord/venv/bin/ansible-playbook ]; then
  python3 -m venv /opt/lanlord/venv
  /opt/lanlord/venv/bin/pip install --quiet --disable-pip-version-check ansible-core
fi
# lanlord runs this python as root through sudo, so lanlord must not be able to write it
chown -R root:root /opt/lanlord/venv
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

/// asks the user for a password; the argument says what for.
pub type AskPassword<'a> = &'a mut dyn FnMut(&str) -> Result<String>;

/// set up the gateway on `opts.host`; the ssh user needs sudo.
pub fn bootstrap(opts: &Options, ask_password: AskPassword) -> Result<()> {
    let session = Session::new(&opts.host)?;
    bootstrap_with(&session, opts, &mut *ask_password)?;
    // after the bootstrap sudo remembers the password, so this asks for nothing more
    crate::env::note(&session, ask_password);
    Ok(())
}

fn bootstrap_with(session: &Session, opts: &Options, ask_password: AskPassword) -> Result<()> {
    bootstrap_lines(session, opts, ask_password, &mut |_| {})
}

/// like `bootstrap` on an open session; every line the server prints also goes to `on_line`.
pub(crate) fn bootstrap_lines(
    session: &Session,
    opts: &Options,
    ask_password: AskPassword,
    on_line: &mut dyn FnMut(&str),
) -> Result<()> {
    emit(Kind::Info, format!("checking {}", opts.host));
    let admin = session.capture(&["id".into(), "-un".into()])?;
    let admin = admin.trim();
    let script = script(opts, admin)?;
    if admin == "root" {
        emit(Kind::Info, ROOT_ADVICE);
    }
    emit(
        Kind::Info,
        format!(
            "setting up {} as {admin} (ansible: {:?})",
            opts.host, opts.ansible
        ),
    );
    match run_as_root_lines(session, admin, script, ask_password, on_line)? {
        0 => {
            emit(Kind::Info, "bootstrap finished");
            Ok(())
        }
        code => bail!("bootstrap failed with status {code}"),
    }
}

/// run `script` as root on the gateway; sudo's password is only asked for
/// when passwordless sudo is not set up for `admin`.
pub(crate) fn run_as_root(
    session: &Session,
    admin: &str,
    script: String,
    ask_password: AskPassword,
) -> Result<i32> {
    let (remote, input) = root_command(session, admin, script, ask_password)?;
    session.run_with_stdin(&remote, input.as_bytes())
}

/// like `run_as_root`, and every line the script prints also goes to `on_line` as it arrives.
pub(crate) fn run_as_root_lines(
    session: &Session,
    admin: &str,
    script: String,
    ask_password: AskPassword,
    on_line: &mut dyn FnMut(&str),
) -> Result<i32> {
    let (remote, input) = root_command(session, admin, script, ask_password)?;
    session.run_with_stdin_lines(&remote, input.as_bytes(), on_line)
}

/// like `run_as_root`, but returns what the script printed.
pub(crate) fn capture_as_root(
    session: &Session,
    admin: &str,
    script: String,
    ask_password: AskPassword,
) -> Result<String> {
    let (remote, input) = root_command(session, admin, script, ask_password)?;
    session.capture_with_stdin(&remote, input.as_bytes())
}

fn root_command(
    session: &Session,
    admin: &str,
    script: String,
    ask_password: AskPassword,
) -> Result<(Vec<String>, String)> {
    // root runs it directly
    if admin == "root" {
        return Ok((vec!["sh".into(), "-s".into()], script));
    }
    // sudo -n fails when a password is needed; then ask once and let sudo remember it
    if session.capture(&sudo(&["-n", "true"])).is_ok() {
        return Ok(plan(script, None));
    }
    let password = ask_password(&format!("sudo password for {admin}"))?;
    if session
        .capture_with_stdin(
            &sudo(&["-S", "-p", "", "-v"]),
            format!("{password}\n").as_bytes(),
        )
        .is_err()
    {
        bail!("sudo did not accept the password for {admin}");
    }
    // the record is shared between ssh sessions once the sudoers rules from bootstrap exist;
    // before that (bootstrap itself) the password goes along with the command
    if session.capture(&sudo(&["-n", "true"])).is_ok() {
        Ok(plan(script, None))
    } else {
        Ok(plan(script, Some(&password)))
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
    fn an_old_lanlord_account_is_brought_to_9999_and_its_files_follow() {
        let s = script(&opts(AnsibleMode::Local, false), "admin").unwrap();
        assert!(s.contains("groupmod --gid 9999 lanlord"));
        assert!(s.contains("usermod --uid 9999 lanlord"));
        assert!(s.contains("usermod --gid 9999 lanlord"));
        assert!(s.contains("usermod --home /opt/lanlord lanlord"));
        // files move before the ids change, and the home before the uid, so an interrupted run
        // converges and usermod never re-owns files outside /opt/lanlord
        let at = |needle: &str| s.find(needle).unwrap();
        assert!(at("-exec chown -h 9999") < at("groupmod --gid 9999"));
        assert!(at("-exec chgrp -h 9999") < at("usermod --uid 9999"));
        assert!(at("usermod --home /opt/lanlord") < at("usermod --uid 9999"));
        assert!(at("pgrep -u") < at("-exec chown -h 9999"));
        assert!(s.contains("find /opt/lanlord -xdev -uid \"$old_uid\" -exec chown -h 9999"));
        assert!(s.contains("find /opt/lanlord -xdev -gid \"$old_gid\" -exec chgrp -h 9999"));
        // the group and account are fixed up after the taken-id checks, before anything uses them
        assert!(s.find("is already used by").unwrap() < s.find("groupmod").unwrap());
        assert!(s.find("usermod --uid").unwrap() < s.find("install -d -o root").unwrap());
    }

    #[test]
    fn creates_the_9999_service_account() {
        let s = script(&opts(AnsibleMode::Local, false), "admin").unwrap();
        assert!(s.contains("groupadd --gid 9999 lanlord"));
        assert!(s.contains("useradd --uid 9999 --gid 9999"));
        assert!(s.contains("/etc/sudoers.d/lanlord"));
    }

    #[test]
    fn refuses_when_9999_is_taken() {
        let s = script(&opts(AnsibleMode::Local, false), "admin").unwrap();
        assert!(s.contains("gid 9999 is already used by group"));
        assert!(s.contains("uid 9999 is already used by user"));
    }

    #[test]
    fn creates_the_admins_group_and_joins_the_ssh_user() {
        let s = script(&opts(AnsibleMode::Local, false), "admin").unwrap();
        assert!(s.contains("groupadd --gid 9998 admins"));
        assert!(s.contains("usermod -aG admins admin"));
        assert!(s.contains("gid 9998 is already used by group"));
        assert!(s.contains("group admins already exists with gid"));
    }

    #[test]
    fn the_venv_is_owned_by_root() {
        let s = script(&opts(AnsibleMode::Remote, false), "admin").unwrap();
        assert!(s.contains("chown -R root:root /opt/lanlord/venv"));
        assert!(!s.contains("sudo -u lanlord"));
        assert!(s.contains("-m 0750 /opt/lanlord/config /opt/lanlord/.ansible /opt/lanlord/state"));
    }

    #[test]
    fn passwordless_sudo_is_opt_in() {
        let off = script(&opts(AnsibleMode::Local, false), "admin").unwrap();
        let on = script(&opts(AnsibleMode::Local, true), "admin").unwrap();
        assert!(!off.contains("lanlord-nopasswd-"));
        assert!(on.contains("/etc/sudoers.d/lanlord-nopasswd-admin"));
        assert!(on.contains("admin ALL=(ALL) NOPASSWD: ALL"));
    }

    #[test]
    fn remote_mode_installs_ansible_in_a_venv() {
        let remote = script(&opts(AnsibleMode::Remote, false), "admin").unwrap();
        let local = script(&opts(AnsibleMode::Local, false), "admin").unwrap();
        assert!(remote.contains("pip install --quiet --disable-pip-version-check ansible-core"));
        assert!(!local.contains("ansible-core"));
    }

    #[test]
    fn tolerates_os_release_without_id_like() {
        // arch has no ID_LIKE and the script runs under set -u
        let s = script(&opts(AnsibleMode::Local, false), "admin").unwrap();
        assert!(s.contains("${ID_LIKE:-}"));
        assert!(!s.contains("$ID_LIKE"));
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

    #[test]
    fn admins_remember_one_sudo_password_for_fifteen_minutes() {
        let rules = admins_sudoers();
        assert!(rules.contains("Defaults:%admins timestamp_type=global\n"));
        assert!(rules.contains("Defaults:%admins timestamp_timeout=15\n"));
        assert!(rules.contains("%admins ALL=(ALL:ALL) ALL\n"));
        let s = script(&opts(AnsibleMode::Local, false), "admin").unwrap();
        assert!(s.contains("/etc/sudoers.d/admins"));
        assert!(s.contains("visudo -cf"));
        assert!(s.contains("install -m 0440 \"$tmp\" /etc/sudoers.d/admins"));
    }

    #[test]
    fn the_per_user_rule_sorts_after_the_group_rule() {
        // sudoers reads /etc/sudoers.d in name order and the last match wins
        let group = "admins";
        let user = format!("lanlord-nopasswd-{}", "admin");
        assert!(user.as_str() > group);
    }

    #[test]
    fn root_needs_no_group_or_sudo_rules() {
        let s = script(&opts(AnsibleMode::Local, true), "root").unwrap();
        assert!(!s.contains("usermod -aG"));
        assert!(!s.contains("lanlord-nopasswd-"));
        assert!(s.contains("/etc/sudoers.d/admins"));
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

    #[cfg(unix)]
    mod session {
        use super::*;
        use std::cell::Cell;

        fn temp(name: &str) -> std::path::PathBuf {
            std::env::temp_dir().join(format!("lanlord-bootstrap-{}-{name}", std::process::id()))
        }

        fn options() -> Options {
            Options {
                host: "gw".into(),
                ansible: AnsibleMode::Local,
                passwordless_sudo: false,
            }
        }

        #[test]
        fn passwordless_sudo_never_asks() {
            let record = temp("nopass");
            let asked = Cell::new(false);
            let mut ask = |_: &str| {
                asked.set(true);
                Ok("x".to_string())
            };
            bootstrap_with(
                &Session::fake_gateway(true, &record, 0),
                &options(),
                &mut ask,
            )
            .unwrap();
            let sent = std::fs::read_to_string(&record).unwrap();
            assert!(sent.starts_with("set -eu"), "{sent}");
            assert!(!asked.get());
            let _ = std::fs::remove_file(record);
        }

        #[test]
        fn sudo_password_goes_first_on_stdin() {
            let record = temp("pass");
            let mut what = String::new();
            let mut ask = |w: &str| {
                what = w.to_string();
                Ok("hunter2".to_string())
            };
            bootstrap_with(
                &Session::fake_gateway(false, &record, 0),
                &options(),
                &mut ask,
            )
            .unwrap();
            let sent = std::fs::read_to_string(&record).unwrap();
            assert!(sent.starts_with("hunter2\nset -eu"), "{sent}");
            assert_eq!(what, "sudo password for tester");
            let _ = std::fs::remove_file(record);
        }

        #[test]
        fn a_cancelled_prompt_stops_bootstrap() {
            let record = temp("cancel");
            let mut ask = |_: &str| anyhow::bail!("cancelled");
            let err = bootstrap_with(
                &Session::fake_gateway(false, &record, 0),
                &options(),
                &mut ask,
            )
            .unwrap_err();
            assert_eq!(err.to_string(), "cancelled");
            assert!(!record.exists());
        }

        /// a fake gateway where sudo remembers a password once `sudo -v` has been run, like the
        /// shared timestamp from the admins sudoers rules.
        fn remembering(dir: &std::path::Path, user: &str, v_exit: i32) -> Session {
            Session::fake(&format!(
                r#"for a; do last=$a; done
case "$last" in
  "id -un") echo {user} ;;
  "sudo -n true") [ -f '{d}/remembered' ] ;;
  "sudo -S -p '' -v") cat > '{d}/password'; [ {v_exit} -eq 0 ] && touch '{d}/remembered'; exit {v_exit} ;;
  *) printf '%s' "$last" > '{d}/command'; cat > '{d}/script' ;;
esac"#,
                d = dir.display()
            ))
        }

        fn dir(name: &str) -> std::path::PathBuf {
            let d = temp(name);
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            d
        }

        #[test]
        fn the_sudo_password_is_asked_once_and_remembered_by_sudo() {
            let d = dir("once");
            let session = remembering(&d, "tester", 0);
            let asked = Cell::new(0);
            let mut ask = |_: &str| {
                asked.set(asked.get() + 1);
                Ok("hunter2".to_string())
            };
            bootstrap_with(&session, &options(), &mut ask).unwrap();
            let script = std::fs::read_to_string(d.join("script")).unwrap();
            assert!(
                script.starts_with("set -eu"),
                "the password is not in the script"
            );
            assert_eq!(
                std::fs::read_to_string(d.join("password")).unwrap(),
                "hunter2\n"
            );
            assert_eq!(
                std::fs::read_to_string(d.join("command")).unwrap(),
                "sudo -n sh -s"
            );
            // a second run within the window asks nothing
            bootstrap_with(&session, &options(), &mut ask).unwrap();
            assert_eq!(asked.get(), 1);
            let _ = std::fs::remove_dir_all(d);
        }

        #[test]
        fn a_wrong_sudo_password_is_reported() {
            let d = dir("wrong");
            let session = remembering(&d, "tester", 1);
            let mut ask = |_: &str| Ok("nope".to_string());
            let err = bootstrap_with(&session, &options(), &mut ask).unwrap_err();
            assert!(
                err.to_string().contains("did not accept the password"),
                "{err}"
            );
            assert!(!d.join("script").exists());
            let _ = std::fs::remove_dir_all(d);
        }

        #[test]
        fn root_runs_the_script_directly_without_asking() {
            let d = dir("root");
            let session = remembering(&d, "root", 0);
            let mut ask = |_: &str| anyhow::bail!("root must not be asked");
            bootstrap_with(&session, &options(), &mut ask).unwrap();
            assert_eq!(std::fs::read_to_string(d.join("command")).unwrap(), "sh -s");
            let script = std::fs::read_to_string(d.join("script")).unwrap();
            assert!(script.starts_with("set -eu") && !script.contains("usermod -aG"));
            let _ = std::fs::remove_dir_all(d);
        }

        #[test]
        fn a_failing_script_is_an_error() {
            let record = temp("fail");
            let mut ask = |_: &str| Ok(String::new());
            let err = bootstrap_with(
                &Session::fake_gateway(true, &record, 4),
                &options(),
                &mut ask,
            )
            .unwrap_err();
            assert!(err.to_string().contains("status 4"), "{err}");
            let _ = std::fs::remove_file(record);
        }
    }
}
