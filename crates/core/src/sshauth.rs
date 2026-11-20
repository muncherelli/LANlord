//! whether sshd takes passwords, and the authorized keys of the administrator accounts: an sshd
//! drop-in (`11-lanlord-auth.conf`) that lanlord writes only when an administrator chooses,
//! checked with `sshd -t` and `sshd -T` and rolled back when sshd would not take it, the same way
//! as root's access (`rootssh`). turning passwords off is refused while an administrator, or root
//! when it may sign in, has no key, unless it is confirmed.

use anyhow::{bail, Result};
use lanlord_model::sshauth::{self, AccountKeys, SshAuthInfo};

use crate::account::login_user;
use crate::bootstrap::{capture_as_root, AskPassword};
use crate::log::{emit, Kind};
use crate::rootssh::{dropin_script, read_dropin_script, script_message, Dropin, Paths};
use crate::ssh::Session;
use crate::{admins, authkeys};

/// the drop-in's name in `sshd_config.d`.
const FILE: &str = "11-lanlord-auth.conf";

/// the sh script (run as root) that prints what sshd says about passwords and root, and how many
/// keys root has: `passwords=yes|no|unknown`, `root=<PermitRootLogin value>|unknown`, `rootkeys=N`.
fn facts_script(paths: &Paths) -> String {
    let find_sshd = paths.find_sshd;
    format!(
        r#"set -eu
{find_sshd}
pw=unknown
root=unknown
if [ -n "${{sshd:-}}" ] && [ -x "$sshd" ]; then
  conf=$("$sshd" -T 2>/dev/null || true)
  pw=$(printf '%s\n' "$conf" | awk 'tolower($1)=="passwordauthentication" {{ print $2; exit }}')
  root=$(printf '%s\n' "$conf" | awk 'tolower($1)=="permitrootlogin" {{ print $2; exit }}')
fi
echo "passwords=${{pw:-unknown}}"
echo "root=${{root:-unknown}}"
n=0
home=$(getent passwd root | cut -d: -f6)
if [ -n "$home" ] && [ -f "$home/.ssh/authorized_keys" ]; then
  n=$(grep -Evc '^[[:space:]]*(#|$)' "$home/.ssh/authorized_keys" || true)
fi
echo "rootkeys=${{n:-0}}"
"#
    )
}

/// what `facts_script` printed: whether sshd takes passwords (none when it could not be asked),
/// whether root may sign in (it does unless sshd said `no`), and how many keys root has.
fn parse_facts(out: &str) -> (Option<bool>, bool, usize) {
    let value = |key: &str| {
        out.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .map(str::trim)
    };
    let passwords = match value("passwords") {
        Some("yes") => Some(true),
        Some("no") => Some(false),
        _ => None,
    };
    let root_login = value("root") != Some("no");
    let root_keys = value("rootkeys").and_then(|n| n.parse().ok()).unwrap_or(0);
    (passwords, root_login, root_keys)
}

/// what the drop-in file says: whether sshd reads the folder, and the choice lanlord made.
fn parse_dropin(out: &str) -> (bool, Option<bool>) {
    let dropins = out.lines().any(|l| l.trim() == "dropins=yes");
    let chosen = out
        .split_once("file:\n")
        .and_then(|(_, file)| sshauth::from_dropin(file));
    (dropins, chosen)
}

/// how ssh sign-in stands on the server: the setting, what sshd does, and the administrators'
/// keys. an administrator whose keys cannot be read counts as having none.
pub fn state(session: &Session, ask: AskPassword) -> Result<SshAuthInfo> {
    let login = login_user(session)?;
    let read = session.capture(&[
        "sh".into(),
        "-c".into(),
        read_dropin_script("/etc/ssh", FILE),
    ])?;
    let (dropins, chosen) = parse_dropin(&read);
    let facts = capture_as_root(session, &login.name, facts_script(&Paths::SYSTEM), ask)?;
    let (passwords, root_login, root_keys) = parse_facts(&facts);
    let mut accounts = Vec::new();
    for user in admins::list_with(session, &mut *ask)? {
        // root and the service account are not managed here; they never are members anyway
        if let Ok(keys) = authkeys::list(session, &user, &mut *ask) {
            accounts.push(AccountKeys { user, keys });
        } else {
            accounts.push(AccountKeys {
                user,
                keys: Vec::new(),
            });
        }
    }
    Ok(SshAuthInfo {
        chosen,
        passwords,
        dropins,
        accounts,
        root_login,
        root_keys,
    })
}

/// the script that sets passwords on or off.
pub(crate) fn apply_script(passwords: bool, paths: &Paths) -> String {
    let content = sshauth::dropin(passwords);
    dropin_script(
        &Dropin {
            file: FILE,
            what: "ssh password sign-in",
            content: &content,
            keyword: "PasswordAuthentication",
            expect: if passwords { "yes" } else { "no" },
            normalize: "",
            say: if passwords {
                "ssh sign-in with a password is on."
            } else {
                "ssh sign-in with a password is off; keys only."
            },
        },
        paths,
    )
}

/// turn ssh password sign-in on or off, as root. turning it off is refused first, unless
/// `confirmed`, when an administrator (or root, while it may sign in) would have no key.
pub fn set_passwords(
    session: &Session,
    passwords: bool,
    confirmed: bool,
    ask: AskPassword,
) -> Result<()> {
    if !passwords && !confirmed {
        if let Some(why) = state(session, &mut *ask)?.refusal() {
            bail!("{why}");
        }
    }
    let login = login_user(session)?;
    emit(
        Kind::Info,
        format!(
            "turning ssh password sign-in {}",
            if passwords { "on" } else { "off" }
        ),
    );
    capture_as_root(
        session,
        &login.name,
        apply_script(passwords, &Paths::SYSTEM),
        ask,
    )
    .map(drop)
    .map_err(script_message)
}

/// the account must be an administrator: this manages their keys, nobody else's.
fn check_admin(session: &Session, user: &str, ask: AskPassword) -> Result<()> {
    if !admins::list_with(session, ask)?.iter().any(|a| a == user) {
        bail!("{user} is not an administrator; keys are managed for administrators only");
    }
    Ok(())
}

/// add `key` to the authorized keys of the administrator `user`.
pub fn add_key(session: &Session, user: &str, key: &str, ask: AskPassword) -> Result<()> {
    check_admin(session, user, &mut *ask)?;
    authkeys::add(session, user, key, ask)
}

/// remove `key` from the authorized keys of the administrator `user`.
pub fn remove_key(session: &Session, user: &str, key: &str, ask: AskPassword) -> Result<()> {
    check_admin(session, user, &mut *ask)?;
    authkeys::remove(session, user, key, ask)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

    fn run(script: &str) -> (i32, String) {
        let mut child = Command::new("sh")
            .arg("-s")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
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
        let text = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        (out.status.code().unwrap(), text)
    }

    /// a scratch `/etc/ssh` with a fake sshd that reads the first PasswordAuthentication of the
    /// drop-in folder (sorted), then the main file, as the real one does.
    struct Rig {
        dir: std::path::PathBuf,
    }

    impl Rig {
        fn new(name: &str, config: &str, extra: &[(&str, &str)]) -> Self {
            let dir =
                std::env::temp_dir().join(format!("lanlord-sshauth-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("sshd_config.d")).unwrap();
            std::fs::write(dir.join("sshd_config"), config).unwrap();
            for (file, text) in extra {
                std::fs::write(dir.join("sshd_config.d").join(file), text).unwrap();
            }
            let sshd = r#"#!/bin/sh
d=$(dirname "$0")
case "$1" in
  -t) exit 0 ;;
  -T)
    v=$(awk -v d="$d" 'tolower($1)=="include" { while ((("cat " d "/sshd_config.d/*.conf") | getline l) > 0) { split(l, w, " "); if (tolower(w[1])=="passwordauthentication") { print w[2]; exit } } next } tolower($1)=="passwordauthentication" { print $2; exit }' "$d/sshd_config")
    echo "passwordauthentication ${v:-yes}" ;;
esac
"#;
            std::fs::write(dir.join("fake-sshd"), sshd).unwrap();
            std::os::unix::fs::PermissionsExt::set_mode(
                &mut std::fs::metadata(dir.join("fake-sshd"))
                    .unwrap()
                    .permissions(),
                0o755,
            );
            std::fs::set_permissions(
                dir.join("fake-sshd"),
                std::os::unix::fs::PermissionsExt::from_mode(0o755),
            )
            .unwrap();
            Self { dir }
        }

        fn apply(&self, passwords: bool) -> (i32, String) {
            let base = self.dir.to_string_lossy().into_owned();
            let sshd = format!("sshd='{base}/fake-sshd'");
            run(&apply_script(
                passwords,
                &Paths {
                    base: &base,
                    find_sshd: &sshd,
                    reload: "true",
                },
            ))
        }

        fn dropin(&self) -> Option<String> {
            std::fs::read_to_string(self.dir.join("sshd_config.d").join(FILE)).ok()
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    const INCLUDE: &str = "Include /etc/ssh/sshd_config.d/*.conf\n";

    #[test]
    fn passwords_are_turned_off_and_on_again_through_the_drop_in() {
        // a distribution's own file, which sorts later, says yes
        let rig = Rig::new(
            "toggle",
            INCLUDE,
            &[("50-cloud-init.conf", "PasswordAuthentication yes\n")],
        );
        let (code, text) = rig.apply(false);
        assert_eq!(code, 0, "{text}");
        assert!(text.contains("keys only"), "{text}");
        assert_eq!(
            rig.dropin().as_deref(),
            Some(sshauth::dropin(false).as_str())
        );
        let (code, text) = rig.apply(true);
        assert_eq!(code, 0, "{text}");
        assert_eq!(
            rig.dropin().as_deref(),
            Some(sshauth::dropin(true).as_str())
        );
    }

    #[test]
    fn a_setting_sshd_would_not_use_is_rolled_back() {
        // the main file sets it before the Include, so it wins over the drop-in
        let rig = Rig::new(
            "earlier",
            "PasswordAuthentication yes\nInclude /etc/ssh/sshd_config.d/*.conf\n",
            &[],
        );
        let (code, text) = rig.apply(false);
        assert_eq!(code, 5, "{text}");
        assert!(text.contains("rolled back"), "{text}");
        assert_eq!(rig.dropin(), None);
    }

    #[test]
    fn without_an_include_line_nothing_is_changed() {
        let rig = Rig::new("no-include", "Port 22\n", &[]);
        let (code, text) = rig.apply(false);
        assert_eq!(code, 3, "{text}");
        assert_eq!(rig.dropin(), None);
    }

    #[test]
    fn the_facts_and_the_drop_in_are_read() {
        assert_eq!(
            parse_facts("passwords=no\nroot=prohibit-password\nrootkeys=2\n"),
            (Some(false), true, 2)
        );
        assert_eq!(
            parse_facts("passwords=yes\nroot=no\nrootkeys=0\n"),
            (Some(true), false, 0)
        );
        // sshd could not be asked: nothing is known, and root is counted to be safe
        assert_eq!(
            parse_facts("passwords=unknown\nroot=unknown\nrootkeys=0\n"),
            (None, true, 0)
        );
        assert_eq!(
            parse_dropin(&format!("dropins=yes\nfile:\n{}", sshauth::dropin(false))),
            (true, Some(false))
        );
        assert_eq!(parse_dropin("dropins=no\n"), (false, None));
    }

    #[test]
    fn the_facts_script_reads_sshd_and_counts_root_keys() {
        let rig = Rig::new(
            "facts",
            INCLUDE,
            &[("11-lanlord-auth.conf", "PasswordAuthentication no\n")],
        );
        let base = rig.dir.to_string_lossy().into_owned();
        let sshd = format!("sshd='{base}/fake-sshd'");
        let (code, out) = run(&facts_script(&Paths {
            base: &base,
            find_sshd: &sshd,
            reload: "true",
        }));
        assert_eq!(code, 0, "{out}");
        let (passwords, _, _) = parse_facts(&out);
        assert_eq!(passwords, Some(false), "{out}");
    }
}
