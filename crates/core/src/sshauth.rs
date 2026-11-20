//! whether sshd takes passwords, and the authorized keys of the administrator accounts: an sshd
//! drop-in (`11-lanlord-auth.conf`) that lanlord writes only when an administrator chooses,
//! checked with `sshd -t` and `sshd -T` and rolled back when sshd would not take it, the same way
//! as root's access (`rootssh`). turning passwords off is refused while an administrator, the
//! signed-in account, or root when it may sign in with a password, has no key, unless it is
//! confirmed; so is removing the last key of an account while passwords are off.

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

/// the sh script (run as root) that prints what sshd says about passwords, root and where it
/// reads keys, and how many keys root has: `passwords=yes|no|unknown`,
/// `root=<PermitRootLogin value>|unknown`, `keyfiles=<AuthorizedKeysFile value>|unknown`,
/// `rootkeys=N`.
fn facts_script(paths: &Paths) -> String {
    let find_sshd = paths.find_sshd;
    format!(
        r#"set -eu
{find_sshd}
pw=unknown
root=unknown
kf=unknown
if [ -n "${{sshd:-}}" ] && [ -x "$sshd" ]; then
  conf=$("$sshd" -T 2>/dev/null || true)
  pw=$(printf '%s\n' "$conf" | awk 'tolower($1)=="passwordauthentication" {{ print $2; exit }}')
  root=$(printf '%s\n' "$conf" | awk 'tolower($1)=="permitrootlogin" {{ print $2; exit }}')
  kf=$(printf '%s\n' "$conf" | awk 'tolower($1)=="authorizedkeysfile" {{ $1=""; sub(/^ /, ""); print; exit }}')
fi
echo "passwords=${{pw:-unknown}}"
echo "root=${{root:-unknown}}"
echo "keyfiles=${{kf:-unknown}}"
n=0
home=$(getent passwd root | cut -d: -f6)
if [ -n "$home" ] && [ -f "$home/.ssh/authorized_keys" ]; then
  n=$(grep -Evc '^[[:space:]]*(#|$)' "$home/.ssh/authorized_keys" || true)
fi
echo "rootkeys=${{n:-0}}"
"#
    )
}

/// what `facts_script` printed.
#[derive(Debug, PartialEq, Eq)]
struct Facts {
    /// whether sshd takes passwords (none when it could not be asked).
    passwords: Option<bool>,
    /// whether root may sign in with a password or a key it is not held back from: not when sshd
    /// said `no`, or one of the levels that never take a password.
    root_login: bool,
    /// sshd reads keys somewhere other than the default files, or could not be asked.
    keys_unknown: bool,
    root_keys: usize,
}

/// the files sshd reads keys from by default (`sshd -T` prints both).
const DEFAULT_KEY_FILES: [&str; 2] = [".ssh/authorized_keys", ".ssh/authorized_keys2"];

/// does `AuthorizedKeysFile` list only the default files? a value naming anything else may put
/// keys where lanlord does not look.
fn default_key_files(value: &str) -> bool {
    let files: Vec<&str> = value.split_whitespace().collect();
    !files.is_empty()
        && files[0] == DEFAULT_KEY_FILES[0]
        && files[1..].iter().all(|f| *f == DEFAULT_KEY_FILES[1])
}

fn parse_facts(out: &str) -> Facts {
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
    // with these levels root signs in with a key or not at all, so passwords off cannot lock it
    // out; unknown counts as able to, to be safe
    let root_login = !matches!(
        value("root"),
        Some("no" | "prohibit-password" | "without-password" | "forced-commands-only")
    );
    Facts {
        passwords,
        root_login,
        keys_unknown: !value("keyfiles").is_some_and(default_key_files),
        root_keys: value("rootkeys").and_then(|n| n.parse().ok()).unwrap_or(0),
    }
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
    let facts = parse_facts(&facts);
    let mut users = admins::list_with(session, &mut *ask)?;
    // whoever is signed in is checked too, administrator or not: their own lock-out counts
    if !matches!(login.name.as_str(), "root" | "lanlord") && !users.contains(&login.name) {
        users.push(login.name.clone());
    }
    let mut accounts = Vec::new();
    for user in users {
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
        passwords: facts.passwords,
        dropins,
        accounts,
        root_login: facts.root_login,
        root_keys: facts.root_keys,
        keys_unknown: facts.keys_unknown,
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

/// the account must be an administrator or the signed-in account: this manages their keys,
/// nobody else's.
fn check_admin(session: &Session, user: &str, ask: AskPassword) -> Result<()> {
    if login_user(session)?.name == user
        || admins::list_with(session, ask)?.iter().any(|a| a == user)
    {
        return Ok(());
    }
    bail!("{user} is not an administrator; keys are managed for administrators only");
}

/// add `key` to the authorized keys of the administrator `user`.
pub fn add_key(session: &Session, user: &str, key: &str, ask: AskPassword) -> Result<()> {
    check_admin(session, user, &mut *ask)?;
    authkeys::add(session, user, key, ask)
}

/// remove the key `key` (its id, see `sshauth::key_id`) from the authorized keys of the
/// administrator `user`. while passwords are off the last key stays, unless `confirmed`.
pub fn remove_key(
    session: &Session,
    user: &str,
    key: &str,
    confirmed: bool,
    ask: AskPassword,
) -> Result<()> {
    check_admin(session, user, &mut *ask)?;
    if !confirmed {
        if let Some(why) = state(session, &mut *ask)?.removal_refusal(user, key) {
            bail!("{why}");
        }
    }
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
        let default = ".ssh/authorized_keys .ssh/authorized_keys2";
        let facts = |pw: &str, root: &str, files: &str, n: usize| {
            parse_facts(&format!(
                "passwords={pw}\nroot={root}\nkeyfiles={files}\nrootkeys={n}\n"
            ))
        };
        let want = |passwords, root_login, keys_unknown, root_keys| Facts {
            passwords,
            root_login,
            keys_unknown,
            root_keys,
        };
        // keys only for root: passwords off cannot lock it out
        assert_eq!(
            facts("no", "prohibit-password", default, 2),
            want(Some(false), false, false, 2)
        );
        assert!(!facts("yes", "forced-commands-only", default, 0).root_login);
        assert!(!facts("yes", "without-password", default, 0).root_login);
        assert_eq!(
            facts("yes", "no", ".ssh/authorized_keys", 0),
            want(Some(true), false, false, 0)
        );
        assert!(facts("yes", "yes", default, 0).root_login);
        // keys sshd reads from other files cannot be seen from here
        assert!(facts("yes", "yes", "/etc/ssh/keys/%u", 0).keys_unknown);
        assert!(facts("yes", "yes", ".ssh/authorized_keys /etc/ssh/extra", 0).keys_unknown);
        // sshd could not be asked: nothing is known, and root is counted to be safe
        assert_eq!(
            facts("unknown", "unknown", "unknown", 0),
            want(None, true, true, 0)
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
        assert_eq!(parse_facts(&out).passwords, Some(false), "{out}");
    }
}
