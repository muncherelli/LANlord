//! a person's own ssh authorized keys on the gateway: list them, add one, remove one. the account
//! is always the caller's own (the operations layer passes it, never the request). root only
//! starts the work: the script that touches ~/.ssh runs as the person (`runuser`), so a link they
//! plant there reaches nothing they could not already change. the key travels base64 encoded
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

/// the type and the base64 field of a key line, ignoring options, comment and spacing: what makes
/// two lines the same key.
pub(crate) fn key_id(line: &str) -> Option<String> {
    let mut words = line.split_whitespace();
    let kind = words.by_ref().find(|w| {
        ["ssh-", "ecdsa-", "sk-ssh-", "sk-ecdsa-"]
            .iter()
            .any(|p| w.starts_with(p))
    })?;
    Some(format!("{kind} {}", words.next()?))
}

/// the shell function (for the script that runs as the person) that prints a line's key id, the
/// same way [`key_id`] reads it.
const KEY_ID: &str = r#"keyid() {
  awk '{ for (i = 1; i < NF; i++) if ($i ~ /^(ssh-|ecdsa-|sk-ssh-|sk-ecdsa-)/) { print $i " " $(i+1); exit } }'
}
"#;

/// what to do with the key.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Edit {
    List,
    Add,
    Remove,
}

/// the script that runs as the person: `home` is a shell expression for their home folder. a
/// failing step stops it before the keys file is replaced.
fn inner_script(home: &str, key: Option<&str>, edit: Edit) -> String {
    if edit == Edit::List {
        return format!(
            r#"set -eu
umask 077
home={home}
file="$home/.ssh/authorized_keys"
if [ -f "$file" ] && [ ! -L "$file" ]; then cat "$file"; fi
"#
        );
    }
    let want = key.map_or_else(String::new, |k| {
        format!("want=$(printf %s '{}' | base64 -d)\n", B64.encode(k))
    });
    let change = if edit == Edit::Add {
        r#"found=0
while IFS= read -r line || [ -n "$line" ]; do
  printf '%s\n' "$line" >> "$new"
  if [ "$(printf '%s\n' "$line" | keyid)" = "$wantid" ]; then found=1; fi
done < "$src"
if [ "$found" = 0 ]; then printf '%s\n' "$want" >> "$new"; fi
"#
    } else {
        r#"hit=0
while IFS= read -r line || [ -n "$line" ]; do
  if [ "$(printf '%s\n' "$line" | keyid)" = "$wantid" ]; then hit=1; else printf '%s\n' "$line" >> "$new"; fi
done < "$src"
if [ "$hit" = 0 ]; then echo "that key is not in your authorized keys" >&2; exit 4; fi
"#
    };
    format!(
        r#"set -eu
umask 077
{KEY_ID}home={home}
if [ -z "$home" ] || [ ! -d "$home" ]; then echo "no home folder" >&2; exit 2; fi
if [ -L "$home/.ssh" ] || [ -L "$home/.ssh/authorized_keys" ]; then
  echo "your ssh folder is a link; it was left alone" >&2; exit 3
fi
{want}wantid=$(printf '%s\n' "$want" | keyid)
mkdir -p "$home/.ssh"
chmod 0700 "$home/.ssh"
file="$home/.ssh/authorized_keys"
new=$(mktemp "$home/.ssh/.lanlord-keys.XXXXXX")
trap 'rm -f "$new"' EXIT
src=/dev/null
if [ -f "$file" ] && [ ! -L "$file" ]; then src="$file"; fi
{change}chmod 0600 "$new"
mv -f "$new" "$file"
"#
    )
}

/// the script root runs: look up the account and hand the work to the person.
fn script(user: &str, key: Option<&str>, edit: Edit) -> String {
    let inner = inner_script("$(getent passwd \"$USER\" | cut -d: -f6)", key, edit);
    format!(
        r#"set -eu
if ! command -v runuser >/dev/null 2>&1; then
  echo "runuser was not found on this server; nothing was changed" >&2; exit 2
fi
if ! id -u {user} >/dev/null 2>&1; then echo "the account {user} does not exist" >&2; exit 2; fi
exec runuser -u {user} -- sh -s <<'LANLORD_AS_USER'
USER={user}
{inner}LANLORD_AS_USER
"#
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
    Ok(key_lines(&run(
        session,
        script(user, None, Edit::List),
        ask,
    )?))
}

/// add `key` to the authorized keys of `user`, creating `~/.ssh` (0700) and the file (0600).
pub fn add(session: &Session, user: &str, key: &str, ask: AskPassword) -> Result<()> {
    check_user(user)?;
    let key = key.trim();
    check_key(key)?;
    run(session, script(user, Some(key), Edit::Add), ask).map(drop)
}

/// remove the key of the line `key` (as listed, options and all) from the authorized keys of
/// `user`; it is an error when no line has that key.
pub fn remove(session: &Session, user: &str, key: &str, ask: AskPassword) -> Result<()> {
    check_user(user)?;
    let Some(id) = key_id(key.trim()) else {
        bail!("that does not look like one ssh public key line");
    };
    check_key(&id)?;
    run(session, script(user, Some(&id), Edit::Remove), ask).map(drop)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};

    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGoZ4HMhG4uZ+G4cDf8iUKvmCZ8F8RAV90ZwPN1B0bJTT pat@laptop";
    const OTHER: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOther000000000000000000000000000000000 pat@desk";

    fn sh(args: &[&str], script: &str) -> (i32, String) {
        let mut child = Command::new("sh")
            .args(args)
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

    /// a scratch home folder, run through the script that normally runs as the person.
    struct Home(std::path::PathBuf);

    impl Home {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("lanlord-authkeys-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn run(&self, key: Option<&str>, edit: Edit) -> (i32, String) {
            let home = format!("'{}'", self.0.display());
            sh(&["-s"], &inner_script(&home, key, edit))
        }

        fn keys(&self) -> String {
            std::fs::read_to_string(self.0.join(".ssh/authorized_keys")).unwrap_or_default()
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_work_runs_as_the_caller_and_the_key_stays_out_of_the_text() {
        for edit in [Edit::List, Edit::Add, Edit::Remove] {
            let s = script("olive", Some(KEY), edit);
            assert!(s.contains("exec runuser -u olive -- sh -s"), "{s}");
            assert!(s.contains("USER=olive"));
            assert!(!s.contains("pat@laptop"), "the key travels encoded");
            // nothing outside the heredoc touches the account's folder
            let outside = s.split("LANLORD_AS_USER").next().unwrap();
            assert!(
                !outside.contains(".ssh") && !outside.contains("install"),
                "{s}"
            );
            assert_eq!(sh(&["-n"], &s).0, 0, "{s}");
        }
    }

    #[test]
    fn adding_makes_the_folder_and_file_private_and_does_not_repeat_a_key() {
        let home = Home::new("add");
        assert_eq!(home.run(Some(KEY), Edit::Add).0, 0);
        assert_eq!(home.run(Some(KEY), Edit::Add).0, 0);
        assert_eq!(home.run(Some(OTHER), Edit::Add).0, 0);
        assert_eq!(home.keys(), format!("{KEY}\n{OTHER}\n"));
        let mode = |p: &str| {
            std::fs::metadata(home.0.join(p))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode(".ssh"), 0o700);
        assert_eq!(mode(".ssh/authorized_keys"), 0o600);
        let (code, listed) = home.run(None, Edit::List);
        assert_eq!(code, 0);
        assert_eq!(key_lines(&listed), [KEY, OTHER]);
    }

    #[test]
    fn removing_matches_the_key_whatever_its_options_and_comment_and_says_when_nothing_matched() {
        let home = Home::new("remove");
        std::fs::create_dir_all(home.0.join(".ssh")).unwrap();
        let with_options = KEY.replace("ssh-ed25519", "from=\"192.0.2.1\",no-pty ssh-ed25519");
        std::fs::write(
            home.0.join(".ssh/authorized_keys"),
            format!("{with_options}\n{OTHER}"),
        )
        .unwrap();
        let id = key_id(KEY).unwrap();
        assert_eq!(key_id(&with_options), Some(id.clone()));
        assert_eq!(home.run(Some(&id), Edit::Remove).0, 0);
        assert_eq!(home.keys(), format!("{OTHER}\n"));
        // nothing matched: an error, and the file is left as it was
        let (code, text) = home.run(Some(&id), Edit::Remove);
        assert_eq!(code, 4, "{text}");
        assert!(text.contains("not in your authorized keys"), "{text}");
        assert_eq!(home.keys(), format!("{OTHER}\n"));
    }

    #[test]
    fn a_failing_step_never_replaces_the_keys_file() {
        let home = Home::new("fail");
        assert_eq!(home.run(Some(KEY), Edit::Add).0, 0);
        // a keyid that cannot run leaves the file alone: no awk on the path
        let script = inner_script(
            &format!("'{}'", home.0.display()),
            Some(&key_id(KEY).unwrap()),
            Edit::Remove,
        );
        let (code, _) = sh(&["-c", "PATH=/nonexistent; exec /bin/sh -s"], &script);
        assert_ne!(code, 0);
        assert_eq!(home.keys(), format!("{KEY}\n"));
    }

    #[test]
    fn a_link_in_the_way_is_refused() {
        let home = Home::new("link");
        std::os::unix::fs::symlink("/", home.0.join(".ssh")).unwrap();
        let (code, text) = home.run(Some(KEY), Edit::Add);
        assert_eq!(code, 3, "{text}");
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
        let err = remove(&session, "olive", "hello", &mut |_| bail!("no")).unwrap_err();
        assert!(format!("{err}").contains("public key"), "{err}");
    }

    #[test]
    fn listing_skips_comments_and_blank_lines() {
        assert_eq!(
            key_lines("# note\n\n  ssh-ed25519 AAAA a \n"),
            ["ssh-ed25519 AAAA a"]
        );
    }
}
