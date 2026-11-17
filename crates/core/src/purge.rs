//! take lanlord back off a server: the containers, the files, the service account and the
//! sudo rules. the preview and the real run are the same script (`DRY=1` only lists), so
//! what you confirm is exactly what happens.

use anyhow::{bail, Result};

use crate::account::login_user;
use crate::admins::ADMINS_GROUP;
use crate::bootstrap::{capture_as_root, AskPassword};
use crate::log::{emit, Kind};
use crate::ssh::Session;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PurgeOptions {
    /// also remove the accounts in the admins group (not root); only allowed when you are root,
    /// because an account cannot be removed while it is signed in.
    pub remove_admins: bool,
}

/// the purge as a shell script. `root` prefixes every path (empty on a real server; a temp
/// directory in tests); it is only ever a fixed path.
fn script(root: &str, dry: bool, options: PurgeOptions) -> String {
    TEMPLATE
        .replace("@ROOT@", root)
        .replace("@DRY@", if dry { "1" } else { "0" })
        .replace(
            "@REMOVE_ADMINS@",
            if options.remove_admins { "1" } else { "0" },
        )
        .replace("@ADMINS@", ADMINS_GROUP)
}

/// one pass: say what would go and then, unless `DRY` is 1, remove it. a failed step is
/// reported and the rest still runs. the sudo rules go last so sudo keeps working until the end.
const TEMPLATE: &str = r#"set -u
ROOT='@ROOT@'
DRY=@DRY@
REMOVE_ADMINS=@REMOVE_ADMINS@

say() { printf '%s\n' "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }
exists() { [ -e "$1" ] || [ -L "$1" ]; }
act() {
  desc=$1
  shift
  say "$desc"
  if [ "$DRY" = 0 ]; then
    "$@" || say "  could not: $desc"
  fi
}

# an apply that is still running would be cut off halfway; the web ui also runs as lanlord, so
# only ansible counts. the line (not the exit status) tells the caller, so it reaches the user
if [ "$DRY" = 0 ] && have pgrep && pgrep -u lanlord -f ansible-playbook >/dev/null 2>&1; then
  say "lanlord is busy (an apply is running); try again in a moment"
  exit 0
fi

# what you would lose
for d in nt4/lib/profiles nt4/lib/homes nt4/lib/software config; do
  p="$ROOT/opt/lanlord/$d"
  if [ -d "$p" ]; then say "data: $(du -sk "$p" | cut -f1) KB in /opt/lanlord/$d"; fi
done
if exists "$ROOT/opt/lanlord/config/settings.age"; then
  say "note: settings.age is the only copy of the stored secrets"
fi

if have docker; then
  for c in lanlord-nt4 lanlord-dns lanlord-caddy; do
    if docker inspect "$c" >/dev/null 2>&1; then act "remove container $c" docker rm -f "$c"; fi
  done
  # one dnsmasq per lanlord network, and any previous copy left by an interrupted replacement
  for c in $(docker ps -a --format '{{.Names}}' 2>/dev/null | grep '^lanlord-dns-'); do
    act "remove container $c" docker rm -f "$c"
  done
  for img in $(docker image ls --format '{{.Repository}}:{{.Tag}}' 2>/dev/null | grep lanlord); do
    act "remove image $img" docker image rm "$img"
  done
fi

for u in lanlord-webui.service lanlord-auth.socket lanlord-auth@.service lanlord-status.timer lanlord-status.service lanlord-firewall.service; do
  if exists "$ROOT/etc/systemd/system/$u"; then
    # the per-connection login service is a template that is never enabled itself
    if have systemctl && [ "$u" != lanlord-auth@.service ]; then act "stop and disable $u" systemctl disable --now "$u"; fi
    act "remove unit $u" rm -f "$ROOT/etc/systemd/system/$u"
  fi
done
if [ "$DRY" = 0 ] && have systemctl; then systemctl daemon-reload >/dev/null 2>&1; fi

# the web ui login: the helper the playbook copies out of /opt/lanlord, and its pam rules
for f in /usr/local/libexec/lanlord-auth-helper /etc/pam.d/lanlord; do
  if exists "$ROOT$f"; then act "remove $f" rm -f "$ROOT$f"; fi
done

if have nft && nft list table inet lanlord >/dev/null 2>&1; then
  act "remove the firewall table inet lanlord" nft delete table inet lanlord
fi

netchanged=0
for f in "$ROOT"/etc/systemd/network/*.network.d/lanlord.conf; do
  if exists "$f"; then
    act "remove $f" rm -f "$f"
    if [ "$DRY" = 0 ]; then rmdir "$(dirname "$f")" 2>/dev/null; fi
    netchanged=1
  fi
done
if [ "$netchanged" = 1 ] && [ "$DRY" = 0 ] && have networkctl; then networkctl reload >/dev/null 2>&1; fi

for d in /etc/lanlord /opt/lanlord /run/lanlord /run/lanlord-web /run/lanlord-auth; do
  if exists "$ROOT$d"; then act "remove $d" rm -rf "$ROOT$d"; fi
done

if [ "$REMOVE_ADMINS" = 1 ]; then
  for m in $(getent group @ADMINS@ | cut -d: -f4 | tr ',' ' '); do
    if [ "$m" = root ]; then continue; fi
    act "remove administrator account $m and its home folder" userdel -r "$m"
  done
fi
if getent passwd lanlord >/dev/null 2>&1; then act "remove the service account lanlord" userdel lanlord; fi
if getent group lanlord-web >/dev/null 2>&1; then act "remove the group lanlord-web" groupdel lanlord-web; fi
if getent group lanlord >/dev/null 2>&1; then act "remove the group lanlord" groupdel lanlord; fi
if getent group @ADMINS@ >/dev/null 2>&1; then act "remove the group @ADMINS@" groupdel @ADMINS@; fi

for f in "$ROOT"/etc/sudoers.d/lanlord "$ROOT"/etc/sudoers.d/@ADMINS@ "$ROOT"/etc/sudoers.d/lanlord-nopasswd-*; do
  if exists "$f"; then act "remove the sudo rules $(basename "$f")" rm -f "$f"; fi
done

say "kept: the docker engine, /etc/motd and the accounts of people"
if [ "$DRY" = 1 ]; then say "dry run: nothing was changed"; else say "done"; fi
"#;

fn check(session: &Session, options: PurgeOptions) -> Result<String> {
    let me = login_user(session)?;
    if options.remove_admins && !me.is_root {
        bail!("removing the administrator accounts needs a root login, because you cannot remove the account you are signed in with");
    }
    Ok(me.name)
}

fn run(
    session: &Session,
    dry: bool,
    options: PurgeOptions,
    ask_password: AskPassword,
) -> Result<Vec<String>> {
    let admin = check(session, options)?;
    let out = capture_as_root(session, &admin, script("", dry, options), ask_password)?;
    Ok(out.lines().map(str::to_string).collect())
}

/// what a purge would remove; nothing is changed.
pub fn preview(
    session: &Session,
    options: PurgeOptions,
    ask_password: AskPassword,
) -> Result<Vec<String>> {
    run(session, true, options, ask_password)
}

/// remove lanlord from `host`. callers confirm the preview first.
pub fn purge(
    session: &Session,
    host: &str,
    options: PurgeOptions,
    ask_password: AskPassword,
) -> Result<Vec<String>> {
    emit(Kind::Info, format!("purging lanlord from {host}"));
    let lines = run(session, false, options, ask_password)?;
    if lines.iter().any(|l| l.contains("lanlord is busy")) {
        bail!("lanlord is busy on {host}; try again in a moment");
    }
    Ok(lines)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const OPTIONS: PurgeOptions = PurgeOptions {
        remove_admins: false,
    };

    /// a fake server in a temp directory: the files, and shims for the commands that would act.
    struct Fake {
        dir: PathBuf,
    }

    impl Fake {
        fn new(name: &str, pgrep_busy: bool) -> Self {
            let dir =
                std::env::temp_dir().join(format!("lanlord-purge-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            let root = dir.join("root");
            for file in [
                "opt/lanlord/config/settings.age",
                "opt/lanlord/install-state",
                "opt/lanlord/nt4/lib/profiles/dana/ntuser.dat",
                "etc/lanlord/firewall.nft",
                "etc/systemd/system/lanlord-firewall.service",
                "etc/systemd/system/lanlord-status.timer",
                "etc/systemd/network/eth1.network.d/lanlord.conf",
                "etc/sudoers.d/lanlord",
                "etc/sudoers.d/admins",
                "etc/sudoers.d/lanlord-nopasswd-alice",
                "etc/sudoers.d/other",
                "run/lanlord/settings.yml",
            ] {
                let path = root.join(file);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, "data").unwrap();
            }
            let bin = dir.join("bin");
            fs::create_dir_all(&bin).unwrap();
            let log = dir.join("log");
            let shims = [
                (
                    "docker",
                    r#"case "$1 $2" in
  "inspect lanlord-nt4") exit 0 ;;
  "inspect "*) exit 1 ;;
  "image ls") printf 'ghcr.io/x/lanlord-samba-nt4:abc\ncaddy:2-alpine\n'; exit 0 ;;
  "ps -a") printf 'lanlord-dns-retronet\nlanlord-dns-lab-prev\nunrelated\n'; exit 0 ;;
esac
echo "docker $*" >> LOG"#,
                ),
                ("userdel", r#"echo "userdel $*" >> LOG"#),
                ("groupdel", r#"echo "groupdel $*" >> LOG"#),
                ("systemctl", r#"echo "systemctl $*" >> LOG"#),
                (
                    "nft",
                    r#"case "$1" in list) exit 0 ;; esac
echo "nft $*" >> LOG"#,
                ),
                ("networkctl", r#"echo "networkctl $*" >> LOG"#),
                // the web ui always runs as lanlord; only an ansible-playbook makes the server busy
                (
                    "pgrep",
                    if pgrep_busy {
                        r#"case "$*" in *ansible-playbook*) exit 0 ;; *) exit 1 ;; esac"#
                    } else {
                        r#"case "$*" in *ansible-playbook*) exit 1 ;; *) exit 0 ;; esac"#
                    },
                ),
                (
                    "getent",
                    r#"case "$1 $2" in
  "group admins") echo "admins:x:9998:root,alice" ;;
  "passwd lanlord") echo "lanlord:x:9999:9999::/opt/lanlord:/bin/false" ;;
  "group lanlord") echo "lanlord:x:9999:" ;;
  "group lanlord-web") echo "lanlord-web:x:9990:" ;;
  *) exit 2 ;;
esac"#,
                ),
            ];
            for (name, body) in shims {
                let path = bin.join(name);
                let text = format!(
                    "#!/bin/sh\n{}\n",
                    body.replace("LOG", log.to_str().unwrap())
                );
                fs::write(&path, text).unwrap();
                Command::new("chmod").arg("+x").arg(&path).status().unwrap();
            }
            Fake { dir }
        }

        fn root(&self) -> PathBuf {
            self.dir.join("root")
        }

        fn run(&self, dry: bool, options: PurgeOptions) -> (i32, String) {
            let script = script(self.root().to_str().unwrap(), dry, options);
            let path = format!(
                "{}:{}",
                self.dir.join("bin").display(),
                std::env::var("PATH").unwrap()
            );
            let out = Command::new("sh")
                .arg("-c")
                .arg(&script)
                .env("PATH", path)
                .output()
                .unwrap();
            (
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stdout).into_owned(),
            )
        }

        fn log(&self) -> String {
            fs::read_to_string(self.dir.join("log")).unwrap_or_default()
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn exists(root: &Path, file: &str) -> bool {
        root.join(file).exists()
    }

    #[test]
    fn the_script_is_valid_sh() {
        for dry in [true, false] {
            let mut child = Command::new("sh")
                .arg("-n")
                .stdin(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(script("", dry, OPTIONS).as_bytes())
                .unwrap();
            assert!(child.wait().unwrap().success());
        }
    }

    #[test]
    fn a_dry_run_lists_everything_and_changes_nothing() {
        let fake = Fake::new("dry", false);
        let (code, out) = fake.run(true, OPTIONS);
        assert_eq!(code, 0, "{out}");
        for expected in [
            "remove container lanlord-nt4",
            "remove image ghcr.io/x/lanlord-samba-nt4:abc",
            "remove unit lanlord-firewall.service",
            "remove the firewall table inet lanlord",
            "remove /opt/lanlord",
            "remove the service account lanlord",
            "remove the group admins",
            "remove the sudo rules admins",
            "remove the sudo rules lanlord-nopasswd-alice",
            "note: settings.age is the only copy of the stored secrets",
            "data: ",
            "dry run: nothing was changed",
        ] {
            assert!(out.contains(expected), "missing {expected:?} in\n{out}");
        }
        assert!(
            !out.contains("caddy:2-alpine"),
            "only lanlord's own images go"
        );
        for dns in [
            "remove container lanlord-dns-retronet",
            "remove container lanlord-dns-lab-prev",
        ] {
            assert!(out.contains(dns), "{dns} in\n{out}");
        }
        assert!(
            !out.contains("unrelated") && !out.contains("container lanlord-dns\n"),
            "only lanlord's containers that exist are listed"
        );
        assert!(fake.log().is_empty(), "no command may act: {}", fake.log());
        for file in [
            "opt/lanlord/config/settings.age",
            "etc/sudoers.d/admins",
            "etc/lanlord/firewall.nft",
        ] {
            assert!(exists(&fake.root(), file), "{file} must still exist");
        }
    }

    #[test]
    fn a_real_run_removes_it_and_leaves_what_is_not_ours() {
        let fake = Fake::new("real", false);
        let (code, out) = fake.run(false, OPTIONS);
        assert_eq!(code, 0, "{out}");
        assert!(out.trim_end().ends_with("done"));
        let root = fake.root();
        for gone in [
            "opt/lanlord",
            "opt/lanlord/install-state",
            "etc/lanlord",
            "run/lanlord",
            "etc/systemd/system/lanlord-firewall.service",
            "etc/systemd/system/lanlord-status.timer",
            "etc/systemd/network/eth1.network.d",
            "etc/sudoers.d/lanlord",
            "etc/sudoers.d/admins",
            "etc/sudoers.d/lanlord-nopasswd-alice",
        ] {
            assert!(!exists(&root, gone), "{gone} should be gone");
        }
        assert!(
            exists(&root, "etc/sudoers.d/other"),
            "someone else's rule stays"
        );
        let log = fake.log();
        for expected in [
            "docker rm -f lanlord-nt4",
            "docker rm -f lanlord-dns-retronet",
            "docker rm -f lanlord-dns-lab-prev",
            "docker image rm ghcr.io/x/lanlord-samba-nt4:abc",
            "nft delete table inet lanlord",
            "userdel lanlord",
            "groupdel lanlord-web",
            "groupdel lanlord",
            "groupdel admins",
            "systemctl daemon-reload",
            "networkctl reload",
        ] {
            assert!(log.contains(expected), "missing {expected:?} in\n{log}");
        }
        assert!(!log.contains("caddy"), "{log}");
        assert!(!log.contains("userdel -r"), "people stay by default: {log}");
    }

    #[test]
    fn removing_admins_spares_root() {
        let fake = Fake::new("admins", false);
        let (_, out) = fake.run(
            false,
            PurgeOptions {
                remove_admins: true,
            },
        );
        assert!(out.contains("remove administrator account alice"), "{out}");
        let log = fake.log();
        assert!(log.contains("userdel -r alice"), "{log}");
        assert!(!log.contains("userdel -r root"), "{log}");
    }

    #[test]
    fn a_running_apply_stops_the_purge_before_it_removes_anything() {
        let fake = Fake::new("busy", true);
        let (code, out) = fake.run(false, OPTIONS);
        // the line, not the exit status, tells purge() so the message reaches the user
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("lanlord is busy"));
        assert!(fake.log().is_empty());
        assert!(exists(&fake.root(), "opt/lanlord/config/settings.age"));
    }

    #[test]
    fn everything_bootstrap_and_the_playbooks_create_is_in_the_purge() {
        let playbooks = [
            "dns",
            "docker",
            "nt4-domain",
            "web-proxy",
            "webui-login",
            "site",
        ]
        .map(|name| {
            fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join(format!("../../ansible/playbooks/{name}.yml")),
            )
            .unwrap()
        });
        let s = script("", false, OPTIONS);
        let mut wanted: Vec<String> = Vec::new();
        for text in &playbooks {
            for line in text.lines().map(str::trim) {
                if let Some(c) = line.strip_prefix("container: ") {
                    wanted.push(c.to_string());
                }
                if let Some(rest) = line.strip_prefix("dest: /etc/systemd/system/") {
                    wanted.push(rest.to_string());
                }
                if let Some(rest) = line.strip_prefix("dest: /etc/pam.d/") {
                    wanted.push(format!("/etc/pam.d/{rest}"));
                }
                if let Some(rest) = line.strip_prefix("helper: ") {
                    wanted.push(rest.to_string());
                }
                if let Some(rest) = line.strip_prefix("- name: lanlord-status") {
                    wanted.push(format!("lanlord-status{}", rest.trim()));
                }
            }
        }
        wanted.extend(
            [
                "/etc/lanlord",
                "/opt/lanlord",
                "/run/lanlord",
                "/run/lanlord-web",
                "/run/lanlord-auth",
                "lanlord",
                "lanlord-web",
                ADMINS_GROUP,
            ]
            .map(str::to_string),
        );
        for item in wanted {
            assert!(s.contains(&item), "the purge does not know about {item}");
        }
        // the install marker sits under a folder the purge removes whole
        assert!(crate::install::MARKER.starts_with("/opt/lanlord/"));
        // bootstrap's sudoers files
        let bootstrap =
            fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bootstrap.rs"))
                .unwrap();
        for file in ["lanlord", "lanlord-nopasswd-"] {
            assert!(
                bootstrap.contains(&format!("/etc/sudoers.d/{file}")),
                "bootstrap no longer writes {file}?"
            );
            assert!(
                s.contains(&format!("sudoers.d/{file}")),
                "the purge does not know {file}"
            );
        }
    }

    fn server(user: &str, uid: &str, output: &str) -> Session {
        Session::fake(&format!(
            r#"for a; do last=$a; done
case "$last" in
  "sh -c 'id -un; id -u'") printf '%s\n%s\n' {user} {uid} ;;
  "sudo -n true") exit 0 ;;
  *) cat >/dev/null; printf '%s\n' '{output}' ;;
esac"#
        ))
    }

    fn no_password(_: &str) -> Result<String> {
        bail!("no password expected")
    }

    #[test]
    fn preview_and_purge_return_what_the_server_said() {
        let session = server("root", "0", "remove /opt/lanlord");
        let lines = preview(&session, OPTIONS, &mut no_password).unwrap();
        assert_eq!(lines, ["remove /opt/lanlord"]);
        let lines = purge(&session, "gw", OPTIONS, &mut no_password).unwrap();
        assert_eq!(lines, ["remove /opt/lanlord"]);
    }

    #[test]
    fn removing_admins_needs_a_root_login() {
        let session = server("olive", "1000", "x");
        let options = PurgeOptions {
            remove_admins: true,
        };
        let err = preview(&session, options, &mut no_password).unwrap_err();
        assert!(err.to_string().contains("root login"), "{err}");
        assert!(preview(&session, OPTIONS, &mut no_password).is_ok());
    }

    #[test]
    fn a_running_web_ui_does_not_stop_the_purge() {
        // the fake's other lanlord processes (the web ui) match pgrep -u lanlord, ansible does not
        let fake = Fake::new("webui", false);
        let (code, out) = fake.run(false, OPTIONS);
        assert_eq!(code, 0, "{out}");
        assert!(!out.contains("lanlord is busy"), "{out}");
        assert!(!exists(&fake.root(), "opt/lanlord"));
    }

    #[test]
    fn a_busy_server_is_an_error() {
        let session = server(
            "root",
            "0",
            "lanlord is busy (an apply is running); try again in a moment",
        );
        let err = purge(&session, "gw", OPTIONS, &mut no_password).unwrap_err();
        assert!(err.to_string().contains("busy"), "{err}");
    }
}
