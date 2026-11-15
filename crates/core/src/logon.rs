//! the logon scripts of the nt4 domain: `logon.bat` runs for everyone and `<user>.bat` for one
//! user, both called by the generated `lanlord.bat`. they live in the netlogon share, which
//! domain admins may also edit from windows; this module reads and writes them from the gateway.

use anyhow::{bail, Result};

use crate::bootstrap::{capture_as_root, run_as_root, AskPassword};
use crate::config::{admin, wrap};
use crate::ssh::Session;
use crate::users::valid_name;

/// the netlogon share on the gateway, which only root can reach.
const DIR: &str = "/opt/lanlord/nt4/lib/netlogon";

/// the script that runs for everyone.
pub const COMMON: &str = "logon";

/// the generated script that calls the others; it is rewritten on every apply.
pub const GENERATED: &str = "lanlord";

/// the id of the domadmins group inside the container (see entrypoint.sh), so admins can edit.
const DOMADMINS_GID: u32 = 1002;

/// a script is a few lines of batch; anything bigger is a mistake.
pub const MAX_BYTES: usize = 64 * 1024;

/// `logon` or the name of a login; the generated script is not editable.
pub fn valid_script_name(name: &str) -> bool {
    name != GENERATED && valid_name(name)
}

/// dos line endings, as nt4 and windows 9x expect; a missing final newline is added.
pub fn to_dos(text: &str) -> String {
    let unix = text.replace("\r\n", "\n");
    let mut dos = unix.replace('\n', "\r\n");
    if !dos.is_empty() && !dos.ends_with("\r\n") {
        dos.push_str("\r\n");
    }
    dos
}

/// unix line endings, for editing.
pub fn to_unix(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// the scripts in the share, without `.bat` and without the generated one.
const LIST_SCRIPT: &str = r#"set -eu
dir=@DIR@
if [ -d "$dir" ]; then
  for file in "$dir"/*.bat; do
    if [ -f "$file" ]; then
      name=$(basename "$file" .bat)
      if [ "$name" != lanlord ]; then echo "$name"; fi
    fi
  done
fi
"#;

/// prints `present` and then the script, or nothing when there is none.
fn read_script(name: &str) -> String {
    format!(
        r#"set -eu
file=@DIR@/{name}.bat
if [ -f "$file" ]; then
  echo present
  cat "$file"
fi
"#
    )
    .replace("@DIR@", DIR)
}

/// the name is checked by the caller and the content travels as base64, so nothing but
/// plain letters and digits ever reaches the shell.
fn write_script(name: &str, dos: &str) -> String {
    format!(
        r#"set -eu
dir=@DIR@
if [ ! -d "$dir" ]; then
  echo "the nt4-domain module is not applied on this gateway" >&2
  exit 1
fi
tmp=$(mktemp "$dir/.lanlord-XXXXXX")
trap 'rm -f "$tmp"' EXIT
base64 -d <<'LANLORD_PAYLOAD' > "$tmp"
{}
LANLORD_PAYLOAD
if [ "$(id -u)" = 0 ]; then chown 0:{DOMADMINS_GID} "$tmp"; fi
chmod 0664 "$tmp"
mv "$tmp" "$dir/{name}.bat"
"#,
        wrap(dos.as_bytes())
    )
    .replace("@DIR@", DIR)
}

/// the names of the editable scripts on the gateway, sorted.
pub fn list(session: &Session, ask_password: AskPassword) -> Result<Vec<String>> {
    let admin = admin(session)?;
    let out = capture_as_root(
        session,
        &admin,
        LIST_SCRIPT.replace("@DIR@", DIR),
        ask_password,
    )?;
    let mut names: Vec<String> = out
        .lines()
        .map(str::trim)
        .filter(|n| valid_script_name(n))
        .map(String::from)
        .collect();
    names.sort();
    Ok(names)
}

fn read_file(session: &Session, name: &str, ask_password: AskPassword) -> Result<Option<String>> {
    let admin = admin(session)?;
    let out = capture_as_root(session, &admin, read_script(name), ask_password)?;
    Ok(out.strip_prefix("present\n").map(to_unix))
}

/// the script `name` with unix line endings; `None` when there is none yet.
pub fn read(session: &Session, name: &str, ask_password: AskPassword) -> Result<Option<String>> {
    if !valid_script_name(name) {
        bail!("{name} is not a valid script name");
    }
    read_file(session, name, ask_password)
}

/// the generated `lanlord.bat`, for reference; `None` before the module is applied.
pub fn read_generated(session: &Session, ask_password: AskPassword) -> Result<Option<String>> {
    read_file(session, GENERATED, ask_password)
}

/// store the script `name`; it is saved with dos line endings and is editable by domain admins.
pub fn write(session: &Session, name: &str, text: &str, ask_password: AskPassword) -> Result<()> {
    if !valid_script_name(name) {
        bail!("{name} is not a valid script name");
    }
    if text.len() > MAX_BYTES {
        bail!("the script is larger than {} KiB", MAX_BYTES / 1024);
    }
    let admin = admin(session)?;
    let script = write_script(name, &to_dos(text));
    match run_as_root(session, &admin, script, ask_password)? {
        0 => Ok(()),
        code => bail!("storing the script failed with status {code}"),
    }
}

/// what the base64 payload of a write script decodes to (for tests).
#[cfg(test)]
fn payload(script: &str) -> Vec<u8> {
    use base64::Engine;
    let body: String = script
        .lines()
        .skip_while(|l| !l.contains("<<'LANLORD_PAYLOAD'"))
        .skip(1)
        .take_while(|l| *l != "LANLORD_PAYLOAD")
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(body)
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_logon_and_login_names_are_scripts() {
        for good in ["logon", "muncherelli", "a-b_c.d1"] {
            assert!(valid_script_name(good), "{good}");
        }
        for bad in [
            "", "lanlord", "Root", "a b", "a;rm", "1abc", "x'y", "../x", "a/b", ".a", "a$(x)",
        ] {
            assert!(!valid_script_name(bad), "{bad}");
        }
    }

    #[test]
    fn line_endings_round_trip() {
        assert_eq!(to_dos("a\nb\n"), "a\r\nb\r\n");
        assert_eq!(to_dos("a\r\nb"), "a\r\nb\r\n");
        assert_eq!(to_dos(""), "");
        assert_eq!(to_unix("a\r\nb\r\n"), "a\nb\n");
    }

    #[test]
    fn the_write_script_carries_the_content_only_as_base64() {
        let script = write_script("muncherelli", &to_dos("net use X: \\\\A\\b; rm -rf /\n"));
        assert!(!script.contains("rm -rf"), "{script}");
        assert_eq!(payload(&script), b"net use X: \\\\A\\b; rm -rf /\r\n");
        assert!(script.contains("mv \"$tmp\" \"$dir/muncherelli.bat\""));
        assert!(script.contains("chown 0:1002"));
        assert!(script.contains("chmod 0664"));
    }

    #[cfg(unix)]
    mod gateway {
        use super::*;
        use crate::testgw::no_password;

        /// a gateway whose netlogon share is a temp directory; root scripts run in `sh` with the
        /// real path swapped for it.
        struct Share {
            dir: std::path::PathBuf,
            session: Session,
        }

        impl Share {
            fn new(name: &str, make_dir: bool) -> Self {
                let dir = std::env::temp_dir()
                    .join(format!("lanlord-logon-{}-{name}", std::process::id()));
                let _ = std::fs::remove_dir_all(&dir);
                if make_dir {
                    std::fs::create_dir_all(&dir).unwrap();
                }
                let session = Session::fake(&format!(
                    r#"for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sudo -n true") exit 0 ;;
  "sudo -n sh -s") sed 's#{DIR}#{dir}#g' | sh ;;
  *) exit 1 ;;
esac"#,
                    dir = dir.display()
                ));
                Self { dir, session }
            }
        }

        impl Drop for Share {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }

        #[test]
        fn a_script_is_written_with_dos_endings_and_read_back_with_unix_ones() {
            let share = Share::new("round", true);
            write(
                &share.session,
                "muncherelli",
                "@echo off\nnet use P: \\\\A\\b\n",
                &mut no_password,
            )
            .unwrap();
            let raw = std::fs::read(share.dir.join("muncherelli.bat")).unwrap();
            assert_eq!(raw, b"@echo off\r\nnet use P: \\\\A\\b\r\n");
            let text = read(&share.session, "muncherelli", &mut no_password)
                .unwrap()
                .unwrap();
            assert_eq!(text, "@echo off\nnet use P: \\\\A\\b\n");
            assert!(read(&share.session, "logon", &mut no_password)
                .unwrap()
                .is_none());
            // no temporary file is left behind
            let left: Vec<_> = std::fs::read_dir(&share.dir).unwrap().collect();
            assert_eq!(left.len(), 1);
        }

        #[test]
        fn the_list_skips_the_generated_script_and_other_files() {
            let share = Share::new("list", true);
            for name in ["lanlord.bat", "logon.bat", "alice.bat", "notes.txt"] {
                std::fs::write(share.dir.join(name), "x").unwrap();
            }
            let names = list(&share.session, &mut no_password).unwrap();
            assert_eq!(names, ["alice", "logon"]);
            let generated = read_generated(&share.session, &mut no_password).unwrap();
            assert_eq!(generated.as_deref(), Some("x"));
        }

        #[test]
        fn a_gateway_without_the_share_has_no_scripts_and_refuses_writes() {
            let share = Share::new("missing", false);
            assert!(list(&share.session, &mut no_password).unwrap().is_empty());
            assert!(read_generated(&share.session, &mut no_password)
                .unwrap()
                .is_none());
            let err = write(&share.session, "logon", "echo hi", &mut no_password).unwrap_err();
            assert!(err.to_string().contains("status 1"), "{err}");
        }

        #[test]
        fn bad_names_and_huge_scripts_are_refused_before_connecting() {
            let share = Share::new("refuse", true);
            assert!(read(&share.session, "lanlord", &mut no_password).is_err());
            assert!(write(&share.session, "a;b", "x", &mut no_password).is_err());
            let huge = "x".repeat(MAX_BYTES + 1);
            assert!(write(&share.session, "logon", &huge, &mut no_password).is_err());
            assert_eq!(std::fs::read_dir(&share.dir).unwrap().count(), 0);
        }

        #[test]
        fn the_scripts_are_valid_sh() {
            use std::io::Write;
            use std::process::{Command, Stdio};
            for script in [
                LIST_SCRIPT.replace("@DIR@", DIR),
                read_script("logon"),
                write_script("logon", "echo hi\r\n"),
            ] {
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
        }
    }
}
