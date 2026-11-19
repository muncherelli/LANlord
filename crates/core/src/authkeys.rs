//! a person's own ssh authorized keys on the gateway: list them, add one, remove one. the account
//! is always the caller's own (the operations layer passes it, never the request). the work runs
//! as root, because the operations service is not the person; the key travels base64 encoded
//! inside the script and is never an argument or part of a shell word.

use anyhow::{bail, Result};
use base64::Engine;

use crate::account::{check_key, login_user};
use crate::bootstrap::{capture_as_root, valid_user, AskPassword};
use crate::ssh::Session;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// the account must be a plain login that is neither root nor lanlord's own service account.
fn check_user(user: &str) -> Result<()> {
    if !valid_user(user) || matches!(user, "root" | "lanlord") {
        bail!("ssh keys are managed for personal accounts only, not {user:?}");
    }
    Ok(())
}

/// the shell that finds the account's home and the keys file, refusing links in the way so the
/// file root reads and writes is the account's own and nothing it points to.
fn locate(user: &str) -> String {
    format!(
        r#"set -eu
home=$(getent passwd {user} | cut -d: -f6)
if [ -z "$home" ] || [ ! -d "$home" ]; then
  echo "the account {user} has no home folder on this server" >&2; exit 2
fi
file="$home/.ssh/authorized_keys"
if [ -L "$home/.ssh" ] || [ -L "$file" ]; then
  echo "the ssh folder of {user} is a link; it was left alone" >&2; exit 3
fi
"#
    )
}

fn list_script(user: &str) -> String {
    format!(
        "{}if [ -f \"$file\" ]; then cat \"$file\"; fi\n",
        locate(user)
    )
}

/// what to do with the key.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Edit {
    Add,
    Remove,
}

fn edit_script(user: &str, key: &str, edit: Edit) -> String {
    let key = B64.encode(key);
    let change = match edit {
        Edit::Add => {
            "if ! grep -qxF -- \"$want\" \"$new\"; then printf '%s\\n' \"$want\" >> \"$new\"; fi"
        }
        Edit::Remove => {
            "grep -vxF -- \"$want\" \"$new\" > \"$new.2\" || true\nmv -f \"$new.2\" \"$new\""
        }
    };
    format!(
        r#"{locate}want=$(printf %s '{key}' | base64 -d)
new=$(mktemp)
trap 'rm -f "$new" "$new.2"' EXIT
if [ -f "$file" ]; then sed '$a\' "$file" > "$new"; fi
{change}
group=$(id -gn {user})
install -d -o {user} -g "$group" -m 0700 "$home/.ssh"
install -o {user} -g "$group" -m 0600 "$new" "$file"
"#,
        locate = locate(user)
    )
}

/// the key lines, without blank lines and comments.
fn key_lines(out: &str) -> Vec<String> {
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

fn run(session: &Session, script: String, ask: AskPassword) -> Result<String> {
    let login = login_user(session)?;
    capture_as_root(session, &login.name, script, ask).map_err(|e| {
        // the script's own message is the useful part
        let text = format!("{e:#}");
        match text.split_once(": ") {
            Some((head, message)) if head.starts_with("command exited with status") => {
                anyhow::anyhow!("{}", message.trim())
            }
            _ => e,
        }
    })
}

/// the authorized keys of `user`.
pub fn list(session: &Session, user: &str, ask: AskPassword) -> Result<Vec<String>> {
    check_user(user)?;
    Ok(key_lines(&run(session, list_script(user), ask)?))
}

/// add `key` to the authorized keys of `user`, creating `~/.ssh` (0700) and the file (0600).
pub fn add(session: &Session, user: &str, key: &str, ask: AskPassword) -> Result<()> {
    check_user(user)?;
    let key = key.trim();
    check_key(key)?;
    run(session, edit_script(user, key, Edit::Add), ask).map(drop)
}

/// remove the line `key` from the authorized keys of `user`.
pub fn remove(session: &Session, user: &str, key: &str, ask: AskPassword) -> Result<()> {
    check_user(user)?;
    let key = key.trim();
    check_key(key)?;
    run(session, edit_script(user, key, Edit::Remove), ask).map(drop)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGoZ4HMhG4uZ+G4cDf8iUKvmCZ8F8RAV90ZwPN1B0bJTT pat@laptop";

    fn valid_sh(script: &str) {
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
    fn the_scripts_parse_and_keep_the_key_out_of_the_text() {
        for edit in [Edit::Add, Edit::Remove] {
            let s = edit_script("olive", KEY, edit);
            assert!(!s.contains("pat@laptop"), "the key travels encoded");
            assert!(s.contains("-m 0700") && s.contains("-m 0600"));
            assert!(s.contains("-L \"$home/.ssh\""), "links are refused");
            valid_sh(&s);
        }
        valid_sh(&list_script("olive"));
    }

    #[test]
    fn only_personal_accounts_are_edited() {
        for bad in ["root", "lanlord", "", "a b", "x;y", "Root"] {
            assert!(check_user(bad).is_err(), "{bad}");
        }
        assert!(check_user("olive").is_ok());
    }

    #[test]
    fn a_key_that_is_not_one_public_key_line_is_refused() {
        let session = Session::fake("exit 1");
        for bad in [
            "",
            "hello",
            "ssh-ed25519",
            "ssh-ed25519 AA'A",
            "ssh-ed25519 AAAA\nssh-rsa B",
        ] {
            let err = add(&session, "olive", bad, &mut |_| bail!("no")).unwrap_err();
            assert!(format!("{err}").contains("public key"), "{bad}: {err}");
        }
    }

    #[test]
    fn listing_skips_comments_and_blank_lines() {
        assert_eq!(
            key_lines("# note\n\n  ssh-ed25519 AAAA a \n"),
            ["ssh-ed25519 AAAA a"]
        );
    }
}
