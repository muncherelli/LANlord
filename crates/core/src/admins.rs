//! who may administer lanlord on a gateway: the members of the `admins` group.

use anyhow::{bail, Result};

use crate::bootstrap::{capture_as_root, valid_user, AskPassword};
use crate::log::{emit, Kind};
use crate::ssh::Session;

/// the unix group whose members may administer lanlord.
pub(crate) const ADMINS_GROUP: &str = "admins";

/// its gid, the same on the gateway and inside the samba container so file ownership matches.
pub(crate) const ADMINS_GID: u32 = 9998;

pub enum Action {
    Add(String),
    Remove(String),
}

/// shell that sets `members` to the comma separated members, or fails when the group is missing.
fn read_members() -> String {
    format!(
        r#"line=$(getent group {ADMINS_GROUP}) || {{
  echo "group {ADMINS_GROUP} does not exist; run lanlord bootstrap first" >&2; exit 1
}}
members=${{line##*:}}
"#
    )
}

fn list_script() -> String {
    let read_members = read_members();
    format!(
        "set -eu\n{read_members}printf '%s\\n' \"$members\" | tr ',' '\\n' | sed '/^$/d' | sort\n"
    )
}

fn change_script(action: &Action) -> Result<String> {
    let (Action::Add(user) | Action::Remove(user)) = action;
    if !valid_user(user) {
        bail!("not a valid user name: {user:?}");
    }
    let body = match action {
        Action::Add(_) => format!(
            r#"id -u {user} >/dev/null 2>&1 || {{ echo "user {user} does not exist on the gateway" >&2; exit 1; }}
case ",$members," in
  *",{user},"*) echo "{user} is already in {ADMINS_GROUP}" ;;
  *) usermod -aG {ADMINS_GROUP} {user}; echo "added {user} to {ADMINS_GROUP}" ;;
esac
"#
        ),
        Action::Remove(_) => format!(
            r#"case ",$members," in
  *",{user},"*) ;;
  *) echo "{user} is not in {ADMINS_GROUP}" >&2; exit 1 ;;
esac
if [ "$members" = {user} ]; then
  echo "{user} is the last member of {ADMINS_GROUP}; add someone else first" >&2; exit 1
fi
gpasswd -d {user} {ADMINS_GROUP} >/dev/null
echo "removed {user} from {ADMINS_GROUP}"
"#
        ),
    };
    let read_members = read_members();
    Ok(format!("set -eu\n{read_members}{body}"))
}

/// the ssh user, for the sudo password prompt.
fn admin_of(session: &Session) -> Result<String> {
    Ok(session
        .capture(&["id".into(), "-un".into()])?
        .trim()
        .to_string())
}

/// the members of `admins` on `host`.
pub fn list(host: &str, ask_password: AskPassword) -> Result<Vec<String>> {
    list_with(&Session::new(host)?, ask_password)
}

fn list_with(session: &Session, ask_password: AskPassword) -> Result<Vec<String>> {
    let admin = admin_of(session)?;
    let out = capture_as_root(session, &admin, list_script(), ask_password)?;
    Ok(out.lines().map(str::to_string).collect())
}

/// add or remove a member of `admins` on `host`.
pub fn change(host: &str, action: &Action, ask_password: AskPassword) -> Result<()> {
    change_with(&Session::new(host)?, action, ask_password)
}

fn change_with(session: &Session, action: &Action, ask_password: AskPassword) -> Result<()> {
    // validate before connecting anywhere
    let script = change_script(action)?;
    let admin = admin_of(session)?;
    let out = capture_as_root(session, &admin, script, ask_password)?;
    emit(Kind::Info, out.trim());
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// a fake gateway whose root scripts print `stdout` and exit with `exit`,
    /// saving the script it was sent.
    fn gateway(record: &std::path::Path, stdout: &str, exit: i32) -> Session {
        Session::fake(&format!(
            r#"for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sudo -n true") exit 0 ;;
  *) cat > '{}'; echo '{stdout}'; echo boom >&2; exit {exit} ;;
esac"#,
            record.display()
        ))
    }

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("admins-{}-{name}", std::process::id()))
    }

    fn no_password(_: &str) -> Result<String> {
        bail!("should not ask")
    }

    fn valid_sh(script: &str) {
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
            .write_all(script.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn scripts_are_valid_sh() {
        valid_sh(&list_script());
        valid_sh(&change_script(&Action::Add("bob".into())).unwrap());
        valid_sh(&change_script(&Action::Remove("bob".into())).unwrap());
    }

    #[test]
    fn add_checks_the_user_exists_and_is_idempotent() {
        let s = change_script(&Action::Add("bob".into())).unwrap();
        assert!(s.contains("id -u bob"));
        assert!(s.contains("usermod -aG admins bob"));
        assert!(s.contains("bob is already in admins"));
    }

    #[test]
    fn remove_keeps_the_last_member() {
        let s = change_script(&Action::Remove("bob".into())).unwrap();
        assert!(s.contains("last member of admins"));
        assert!(s.contains("gpasswd -d bob admins"));
        assert!(s.contains("bob is not in admins"));
    }

    #[test]
    fn unsafe_names_are_refused() {
        for bad in ["", "Bob", "1bob", "a b", "a;b", "x'y"] {
            assert!(change_script(&Action::Add(bad.into())).is_err(), "{bad}");
            assert!(change_script(&Action::Remove(bad.into())).is_err(), "{bad}");
        }
    }

    #[test]
    fn list_returns_the_printed_members() {
        let record = temp("list");
        let members = list_with(&gateway(&record, "alice\nbob", 0), &mut no_password).unwrap();
        assert_eq!(members, ["alice", "bob"]);
        let sent = std::fs::read_to_string(&record).unwrap();
        assert!(sent.contains("getent group admins"));
        let _ = std::fs::remove_file(record);
    }

    #[test]
    fn change_sends_the_script() {
        let record = temp("add");
        let action = Action::Add("bob".into());
        change_with(&gateway(&record, "added bob", 0), &action, &mut no_password).unwrap();
        let sent = std::fs::read_to_string(&record).unwrap();
        assert!(sent.contains("usermod -aG"));
        let _ = std::fs::remove_file(record);
    }

    #[test]
    fn a_refusal_on_the_gateway_is_an_error() {
        let record = temp("refuse");
        let action = Action::Remove("bob".into());
        assert!(change_with(&gateway(&record, "", 1), &action, &mut no_password).is_err());
        let _ = std::fs::remove_file(record);
    }

    #[test]
    fn an_invalid_name_never_connects() {
        let record = temp("invalid");
        let action = Action::Add("Bad Name".into());
        assert!(change_with(&gateway(&record, "", 0), &action, &mut no_password).is_err());
        assert!(!record.exists());
    }
}
