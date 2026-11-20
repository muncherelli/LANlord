//! how root may sign in to the gateway over ssh: an sshd drop-in that lanlord writes only when an
//! administrator chooses a level, checked with `sshd -t` and rolled back when sshd would not take
//! it. sshd is left alone until then.

use anyhow::Result;
use lanlord_model::rootssh::{RootSsh, RootSshInfo};

use crate::account::login_user;
use crate::bootstrap::{capture_as_root, AskPassword};
use crate::log::{emit, Kind};
use crate::ssh::Session;

/// where the pieces are, so the script can run against a scratch folder in a test.
pub(crate) struct Paths<'a> {
    /// sshd's configuration folder, `/etc/ssh` on a gateway.
    pub base: &'a str,
    /// shell that sets `sshd` to the daemon's path, or leaves it empty.
    pub find_sshd: &'a str,
    /// a shell function body that asks the running sshd to read its configuration again.
    pub reload: &'a str,
}

impl Paths<'static> {
    pub(crate) const SYSTEM: Paths<'static> = Paths {
        base: "/etc/ssh",
        find_sshd: "sshd=$(command -v sshd 2>/dev/null || true)\n\
                    if [ -z \"$sshd\" ]; then for c in /usr/sbin/sshd /usr/local/sbin/sshd; do\n\
                    \x20 if [ -x \"$c\" ]; then sshd=$c; break; fi; done; fi",
        reload: "systemctl reload sshd.service 2>/dev/null \
                 || systemctl reload ssh.service 2>/dev/null \
                 || { [ -r /run/sshd.pid ] && kill -HUP \"$(cat /run/sshd.pid)\"; }",
    };
}

/// the sh script that reads the state: whether sshd reads the drop-in folder and what the drop-in
/// `file` (a name in `sshd_config.d`) holds. nothing in it needs root.
pub(crate) fn read_dropin_script(base: &str, file: &str) -> String {
    format!(
        r#"if grep -Eqi '^[[:space:]]*Include[[:space:]].*sshd_config\.d' '{base}/sshd_config' 2>/dev/null; then
  echo "dropins=yes"
else
  echo "dropins=no"
fi
if [ -r '{base}/sshd_config.d/{file}' ]; then
  echo "file:"
  cat '{base}/sshd_config.d/{file}'
fi
"#
    )
}

fn read_script(base: &str) -> String {
    read_dropin_script(base, "10-lanlord-root.conf")
}

/// read what the server's sshd setting is, as `read_script` printed it.
fn parse_state(out: &str) -> RootSshInfo {
    let dropins = out.lines().any(|l| l.trim() == "dropins=yes");
    let chosen = out
        .split_once("file:\n")
        .and_then(|(_, file)| RootSsh::from_dropin(file));
    RootSshInfo { chosen, dropins }
}

/// how root's ssh access stands on the server.
pub fn state(session: &Session) -> Result<RootSshInfo> {
    let script = read_script("/etc/ssh");
    let out = session.capture(&["sh".into(), "-c".into(), script])?;
    Ok(parse_state(&out))
}

/// the sh script (run as root) that prints what sshd says about password sign-in (`yes`, `no`),
/// or `unknown` when sshd cannot be asked.
fn passwords_script(paths: &Paths) -> String {
    let find_sshd = paths.find_sshd;
    format!(
        r#"set -eu
{find_sshd}
if [ -z "${{sshd:-}}" ] || [ ! -x "$sshd" ]; then echo unknown; exit 0; fi
v=$("$sshd" -T 2>/dev/null | awk 'tolower($1)=="passwordauthentication" {{ print $2; exit }}' || true)
echo "${{v:-unknown}}"
"#
    )
}

/// whether sshd refuses ssh passwords now. an answer that cannot be read counts as no: only a
/// clear `no` blocks anything.
pub fn passwords_refused(session: &Session, ask: AskPassword) -> Result<bool> {
    let login = login_user(session)?;
    let out = capture_as_root(session, &login.name, passwords_script(&Paths::SYSTEM), ask)?;
    Ok(out.trim() == "no")
}

/// one sshd drop-in lanlord writes, and what sshd must say about it afterwards.
pub(crate) struct Dropin<'a> {
    /// the file's name in `sshd_config.d`.
    pub file: &'a str,
    /// what the person is told it is about, for the messages ("root's ssh access").
    pub what: &'a str,
    /// the file's contents, ending in a newline.
    pub content: &'a str,
    /// the keyword `sshd -T` is asked about (`PermitRootLogin`) and the value it must give.
    pub keyword: &'a str,
    pub expect: &'a str,
    /// shell run on the value sshd gave, to read an old name as the new one.
    pub normalize: &'a str,
    /// what is said when it worked.
    pub say: &'a str,
}

/// the sh script (run as root) that writes the drop-in: ask sshd to check its configuration and
/// say what it would use, reload it, and put things back the way they were when any step says no.
/// the exit status says which step: 3 sshd cannot take a drop-in, 4 `sshd -t` refused it, 5 sshd
/// would not use it, 6 the reload failed.
pub(crate) fn dropin_script(spec: &Dropin, paths: &Paths) -> String {
    let Paths {
        base,
        find_sshd,
        reload,
    } = paths;
    let Dropin {
        file,
        what,
        content,
        keyword,
        expect,
        normalize,
        say,
    } = spec;
    let lower = keyword.to_lowercase();
    format!(
        r#"set -eu
base='{base}'
dir="$base/sshd_config.d"
file="$dir/{file}"
{find_sshd}
if [ -z "${{sshd:-}}" ] || [ ! -x "$sshd" ]; then
  echo "sshd was not found on this server; nothing was changed" >&2; exit 3
fi
if ! grep -Eqi '^[[:space:]]*Include[[:space:]].*sshd_config\.d' "$base/sshd_config" 2>/dev/null; then
  echo "this server's sshd does not read $dir (there is no Include line in $base/sshd_config), so {what} cannot be set here; nothing was changed" >&2
  exit 3
fi
mkdir -p "$dir"
new=$(mktemp "$dir/.lanlord-dropin.XXXXXX")
old=
if [ -e "$file" ]; then
  old=$(mktemp "$dir/.lanlord-dropin-old.XXXXXX")
  cp -p "$file" "$old"
fi
back() {{
  if [ -n "$old" ]; then mv -f "$old" "$file"; else rm -f "$file"; fi
  rm -f "$new"
}}
reload() {{
  {reload}
}}
cat > "$new" <<'LANLORD_DROPIN'
{content}LANLORD_DROPIN
chmod 0644 "$new"
mv -f "$new" "$file"
if ! out=$("$sshd" -t 2>&1); then
  back
  echo "sshd refused the change, so it was rolled back: $out" >&2
  exit 4
fi
seen=$("$sshd" -T 2>/dev/null | awk 'tolower($1)=="{lower}" {{ print $2; exit }}' || true)
{normalize}
if [ "$seen" != '{expect}' ]; then
  back
  echo "sshd would not use the change (it reads {keyword} as '${{seen:-unknown}}', probably set earlier in $base/sshd_config), so it was rolled back" >&2
  exit 5
fi
if ! reload >/dev/null 2>&1; then
  back
  echo "sshd could not be reloaded, so the change was rolled back" >&2
  exit 6
fi
if [ -n "$old" ]; then rm -f "$old"; fi
echo '{say}'
"#
    )
}

/// the script that sets root's ssh access to `level`.
pub(crate) fn apply_script(level: RootSsh, paths: &Paths) -> String {
    let content = level.dropin();
    dropin_script(
        &Dropin {
            file: "10-lanlord-root.conf",
            what: "root's ssh access",
            content: &content,
            keyword: "PermitRootLogin",
            expect: level.directive(),
            // older openssh prints the keys-only level under its former name
            normalize: "if [ \"$seen\" = without-password ]; then seen=prohibit-password; fi",
            say: level.explain(),
        },
        paths,
    )
}

/// set root's ssh access to `level` on the server, as root (through sudo when the session is not).
pub fn apply(session: &Session, level: RootSsh, ask: AskPassword) -> Result<()> {
    let login = login_user(session)?;
    emit(
        Kind::Info,
        format!("setting root's ssh access to {}", level.directive()),
    );
    match capture_as_root(
        session,
        &login.name,
        apply_script(level, &Paths::SYSTEM),
        ask,
    ) {
        Ok(_) => {
            emit(
                Kind::Info,
                format!("root's ssh access is now {}", level.label()),
            );
            Ok(())
        }
        Err(e) => Err(script_message(e)),
    }
}

/// the script's own message is the useful part of the error of a failed script.
pub(crate) fn script_message(e: anyhow::Error) -> anyhow::Error {
    let text = format!("{e:#}");
    match text.split_once(": ") {
        Some((head, message)) if head.starts_with("command exited with status") => {
            anyhow::anyhow!("{}", message.trim())
        }
        _ => e,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

    /// a scratch `/etc/ssh` with an sshd_config, and a fake sshd that checks the drop-in.
    struct Rig {
        dir: std::path::PathBuf,
    }

    impl Rig {
        fn new(name: &str, config: &str, sshd: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("lanlord-rootssh-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("sshd_config"), config).unwrap();
            std::fs::write(dir.join("fake-sshd"), sshd).unwrap();
            let mut perms = std::fs::metadata(dir.join("fake-sshd"))
                .unwrap()
                .permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
            std::fs::set_permissions(dir.join("fake-sshd"), perms).unwrap();
            Self { dir }
        }

        fn base(&self) -> String {
            self.dir.to_string_lossy().into_owned()
        }

        fn dropin(&self) -> Option<String> {
            std::fs::read_to_string(self.dir.join("sshd_config.d/10-lanlord-root.conf")).ok()
        }

        fn apply(&self, level: RootSsh, reload: &str) -> (i32, String) {
            let base = self.base();
            let sshd = format!("sshd='{base}/fake-sshd'");
            let paths = Paths {
                base: &base,
                find_sshd: &sshd,
                reload,
            };
            let script = apply_script(level, &paths);
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
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    const INCLUDE: &str = "Include /etc/ssh/sshd_config.d/*.conf\nPasswordAuthentication yes\n";

    /// accepts `-t`, and for `-T` prints what a real sshd would: the first PermitRootLogin of the
    /// drop-in, else the main file's, else the default.
    fn honest_sshd() -> String {
        String::from(
            r#"#!/bin/sh
d=$(dirname "$0")
case "$1" in
  -t) exit 0 ;;
  -T)
    v=$(awk 'tolower($1)=="permitrootlogin" { print $2; exit }' "$d/sshd_config.d/10-lanlord-root.conf" "$d/sshd_config" 2>/dev/null)
    echo "permitrootlogin ${v:-prohibit-password}" ;;
esac
"#,
        )
    }

    #[test]
    fn each_level_writes_its_drop_in_and_reloads() {
        for level in RootSsh::ALL {
            let rig = Rig::new(level.directive(), INCLUDE, &honest_sshd());
            let (code, text) = rig.apply(level, "true");
            assert_eq!(code, 0, "{text}");
            assert_eq!(rig.dropin().as_deref(), Some(level.dropin().as_str()));
            assert!(text.contains(level.explain()), "{text}");
            let leftovers: Vec<_> = std::fs::read_dir(rig.dir.join("sshd_config.d"))
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            assert_eq!(leftovers.len(), 1, "{leftovers:?}");
        }
    }

    #[test]
    fn a_choice_can_be_changed_and_read_back() {
        let rig = Rig::new("change", INCLUDE, &honest_sshd());
        assert_eq!(rig.apply(RootSsh::Allowed, "true").0, 0);
        assert_eq!(rig.apply(RootSsh::Off, "true").0, 0);
        let text = read_script(&rig.base());
        let out = Command::new("sh").arg("-c").arg(text).output().unwrap();
        let info = parse_state(&String::from_utf8_lossy(&out.stdout));
        assert_eq!(
            info,
            RootSshInfo {
                chosen: Some(RootSsh::Off),
                dropins: true
            }
        );
    }

    #[test]
    fn a_failed_sshd_check_rolls_the_drop_in_back() {
        let failing =
            "#!/bin/sh\ncase \"$1\" in -t) echo 'bad configuration' >&2; exit 1 ;; esac\n";
        let rig = Rig::new("t-fails", INCLUDE, failing);
        let (code, text) = rig.apply(RootSsh::Off, "true");
        assert_eq!(code, 4, "{text}");
        assert!(
            text.contains("rolled back") && text.contains("bad configuration"),
            "{text}"
        );
        assert_eq!(rig.dropin(), None, "no drop-in is left");
    }

    #[test]
    fn a_failed_check_restores_the_earlier_choice() {
        let rig = Rig::new("t-restores", INCLUDE, &honest_sshd());
        assert_eq!(rig.apply(RootSsh::Allowed, "true").0, 0);
        std::fs::write(
            rig.dir.join("fake-sshd"),
            "#!/bin/sh\ncase \"$1\" in -t) exit 1 ;; esac\n",
        )
        .unwrap();
        let (code, _) = rig.apply(RootSsh::Off, "true");
        assert_eq!(code, 4);
        assert_eq!(
            rig.dropin().as_deref(),
            Some(RootSsh::Allowed.dropin().as_str())
        );
    }

    #[test]
    fn a_failed_reload_rolls_back_too() {
        let rig = Rig::new("reload-fails", INCLUDE, &honest_sshd());
        let (code, text) = rig.apply(RootSsh::KeysOnly, "false");
        assert_eq!(code, 6, "{text}");
        assert_eq!(rig.dropin(), None);
    }

    #[test]
    fn an_sshd_that_does_not_read_the_folder_is_said_so_and_left_alone() {
        let rig = Rig::new("no-include", "PasswordAuthentication yes\n", &honest_sshd());
        let (code, text) = rig.apply(RootSsh::Off, "true");
        assert_eq!(code, 3, "{text}");
        assert!(
            text.contains("does not read") && text.contains("nothing was changed"),
            "{text}"
        );
        assert!(!rig.dir.join("sshd_config.d").exists(), "no folder is made");
        let out = Command::new("sh")
            .arg("-c")
            .arg(read_script(&rig.base()))
            .output()
            .unwrap();
        let info = parse_state(&String::from_utf8_lossy(&out.stdout));
        assert_eq!(
            info,
            RootSshInfo {
                chosen: None,
                dropins: false
            }
        );
    }

    #[test]
    fn a_value_set_earlier_in_the_main_file_wins_so_the_change_is_rolled_back() {
        // the first value sshd reads wins: the main file sets it before the include
        let config = "PermitRootLogin yes\nInclude /etc/ssh/sshd_config.d/*.conf\n";
        let rig = Rig::new("shadowed", config, &{
            // this fake reads the main file first, like sshd would
            r#"#!/bin/sh
d=$(dirname "$0")
case "$1" in
  -t) exit 0 ;;
  -T) v=$(awk 'tolower($1)=="permitrootlogin" { print $2; exit }' "$d/sshd_config" "$d/sshd_config.d/10-lanlord-root.conf")
      echo "permitrootlogin $v" ;;
esac
"#
            .to_string()
        });
        let (code, text) = rig.apply(RootSsh::Off, "true");
        assert_eq!(code, 5, "{text}");
        assert!(text.contains("rolled back"), "{text}");
        assert_eq!(rig.dropin(), None);
    }

    #[test]
    fn the_password_check_reads_what_sshd_would_use() {
        let run = |sshd: &str| {
            let rig = Rig::new("passwords", INCLUDE, sshd);
            let base = rig.base();
            let find = format!("sshd='{base}/fake-sshd'");
            let paths = Paths {
                base: &base,
                find_sshd: &find,
                reload: "true",
            };
            let out = Command::new("sh")
                .arg("-c")
                .arg(passwords_script(&paths))
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        assert_eq!(run("#!/bin/sh\necho 'passwordauthentication no'\n"), "no");
        assert_eq!(run("#!/bin/sh\necho 'passwordauthentication yes'\n"), "yes");
        assert_eq!(run("#!/bin/sh\nexit 1\n"), "unknown");
    }

    #[test]
    fn a_server_without_sshd_is_said_so() {
        let rig = Rig::new("no-sshd", INCLUDE, "");
        std::fs::remove_file(rig.dir.join("fake-sshd")).unwrap();
        let (code, text) = rig.apply(RootSsh::Off, "true");
        assert_eq!(code, 3, "{text}");
        assert!(text.contains("sshd was not found"), "{text}");
    }

    #[test]
    fn the_system_script_is_valid_sh_and_names_the_real_paths() {
        let script = apply_script(RootSsh::KeysOnly, &Paths::SYSTEM);
        assert!(script.contains("base='/etc/ssh'"));
        assert!(script.contains("PermitRootLogin prohibit-password"));
        assert!(script.contains("sshd -t") || script.contains("\"$sshd\" -t"));
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
        assert!(lanlord_model::rootssh::DROPIN.ends_with("/10-lanlord-root.conf"));
        assert!(script.contains("10-lanlord-root.conf"));
    }
}
