//! the web ui step of an install: get the server binary, the login helper and the browser build,
//! put them on the gateway beside the previous version, apply the web-proxy and webui-login
//! modules with defaults that need no lanlord network, and check that https answers.
//!
//! the files come from one of two places: a local build (`--server-binary`, `--web-dist` and
//! `--auth-helper`, for development) that is uploaded over ssh, or the release assets of the cli's
//! own version, which the gateway downloads and checks against their sha256 before it unpacks them.
//! that check is integrity only (the sums come from the same release as the files); there is no
//! signature check yet.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

use crate::account::login_user;
use crate::apply;
use crate::bootstrap::{capture_as_root, run_as_root, run_as_root_lines, AskPassword};
use crate::ssh::Session;

/// where the files wait on the gateway until they are all there and checked.
pub const STAGE: &str = "/opt/lanlord/web-stage";

/// the port caddy serves the web ui on.
pub const PORT: u16 = 8443;

/// the modules the step applies, in the order the playbook runs them.
pub const MODULES: &[&str] = &["web-proxy", "webui-login"];

/// the settings the step applies: no lanlord networks, no sites and no status page, so caddy
/// binds only the web ui port on the host address, with its own certificate authority.
pub const SETTINGS: &str = "\
# the install's defaults for the web ui
networks: []
web-proxy:
  tls: \"off\"
  status:
    enabled: false
  webui:
    tls: internal
";

const REPO: &str = "https://github.com/muncherelli/LANlord";

const FLAGS: &str =
    "pass --server-binary PATH --web-dist DIR --auth-helper PATH to use a local build";

/// a local build to upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Local {
    pub server_binary: PathBuf,
    pub web_dist: PathBuf,
    pub auth_helper: PathBuf,
}

impl Local {
    fn check(&self) -> Result<()> {
        for (what, path) in [
            ("--server-binary", &self.server_binary),
            ("--auth-helper", &self.auth_helper),
        ] {
            if !path.is_file() {
                bail!("{what}: {} is not a file", path.display());
            }
        }
        if !self.web_dist.join("index.html").is_file() {
            bail!(
                "--web-dist: {} has no index.html; build it with tools/dev.sh web",
                self.web_dist.display()
            );
        }
        Ok(())
    }
}

/// where the web ui files come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebSource {
    Local(Local),
    /// the release assets of this tag, like `v0.1.0`.
    Release {
        tag: String,
    },
}

impl WebSource {
    /// the flags decide: all three name a local build, none means this version's release, and
    /// anything in between is a mistake worth stopping on.
    pub fn choose(
        server_binary: Option<PathBuf>,
        web_dist: Option<PathBuf>,
        auth_helper: Option<PathBuf>,
    ) -> Result<Self> {
        Self::choose_for(
            lanlord_model::version::is_development(),
            server_binary,
            web_dist,
            auth_helper,
        )
    }

    /// `choose`, with whether this is a development build spelled out. a development build has
    /// no release of its own to download, so it must be told where the files are.
    fn choose_for(
        development: bool,
        server_binary: Option<PathBuf>,
        web_dist: Option<PathBuf>,
        auth_helper: Option<PathBuf>,
    ) -> Result<Self> {
        match (server_binary, web_dist, auth_helper) {
            (None, None, None) if development => bail!(
                "this is a development build ({}), and there is no release to download for it; \
                 pass --server-binary, --web-dist and --auth-helper, or use a release build",
                lanlord_model::version::display()
            ),
            (None, None, None) => Ok(Self::release()),
            (Some(server_binary), Some(web_dist), Some(auth_helper)) => {
                let local = Local {
                    server_binary,
                    web_dist,
                    auth_helper,
                };
                local.check()?;
                Ok(Self::Local(local))
            }
            _ => bail!("--server-binary, --web-dist and --auth-helper go together; {FLAGS}"),
        }
    }

    /// the release that matches this build of lanlord.
    pub fn release() -> Self {
        Self::Release {
            tag: format!("v{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

/// the release architecture name for what `uname -m` says on the gateway.
pub fn asset_arch(uname: &str) -> Result<&'static str> {
    match uname.trim() {
        "x86_64" | "amd64" => Ok("x86_64"),
        "aarch64" | "arm64" => Ok("arm64"),
        other => bail!(
            "no release build for the architecture {other:?} (x86_64 and aarch64 have one); {FLAGS}"
        ),
    }
}

/// the release files for one tag and architecture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assets {
    pub base: String,
    pub server: String,
    pub web: String,
}

pub fn release_assets(tag: &str, arch: &str) -> Assets {
    Assets {
        base: format!("{REPO}/releases/download/{tag}"),
        server: format!("lanlord-{tag}-linux-{arch}-headless.tar.gz"),
        web: format!("lanlord-{tag}-web.tar.gz"),
    }
}

/// `root` is empty on a server; the tests point it at a folder.
fn with_root(script: &str, root: &str) -> String {
    script
        .replace("@ROOT@", root)
        .replace("@STAGE@", STAGE)
        .replace("@PORT@", &PORT.to_string())
}

/// make an empty stage, readable by root only.
pub fn prepare_script(root: &str) -> String {
    with_root(
        r#"set -eu
S='@ROOT@@STAGE@'
rm -rf "$S"
mkdir -p "$S/web"
chmod 0700 "$S"
"#,
        root,
    )
}

/// download the release files on the gateway, refuse them unless their sha256 matches, and
/// unpack them into the stage. exit 20: a download failed, 21: a checksum did not match, 22: a
/// file is missing from the release.
pub fn fetch_script(root: &str, assets: &Assets) -> String {
    let script = r#"set -eu
S='@ROOT@@STAGE@'
rm -rf "$S"
mkdir -p "$S/web" "$S/dl"
chmod 0700 "$S"
cd "$S/dl"
fetch() {
  if ! curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 --retry 2 -o "$1" "@BASE@/$1" || ! curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 --retry 2 -o "$1.sha256" "@BASE@/$1.sha256"; then
    echo "could not download @BASE@/$1" >&2
    rm -rf "$S"
    exit 20
  fi
  want=$(awk '{print $1; exit}' "$1.sha256")
  got=$(sha256sum "$1" | awk '{print $1}')
  if [ -z "$want" ] || [ "$want" != "$got" ]; then
    echo "the sha256 of $1 is $got but the release says ${want:-nothing}; refusing it" >&2
    rm -rf "$S"
    exit 21
  fi
  echo "checked $1"
}
fetch '@SERVER@'
fetch '@WEB@'
tar -xzf '@SERVER@' --strip-components=1
tar -xzf '@WEB@' -C "$S/web"
if [ ! -f lanlord ] || [ ! -f lanlord-auth-helper ] || [ ! -f "$S/web/index.html" ]; then
  echo "the release files are missing the server binary, the login helper or the browser build" >&2
  rm -rf "$S"
  exit 22
fi
mv lanlord lanlord-auth-helper "$S/"
cd /
rm -rf "$S/dl"
"#;
    with_root(script, root)
        .replace("@BASE@", &assets.base)
        .replace("@SERVER@", &assets.server)
        .replace("@WEB@", &assets.web)
}

/// put the staged files in place, keeping the previous binary, login helper and web folder
/// beside them as `lanlord.prev`, `lanlord-auth-helper.prev` and `web.prev` the way
/// tools/dev-deploy.sh does, and restart the service when it exists so it runs the new files.
/// the backups of an earlier swap are removed first, so every `.prev` left afterwards belongs to
/// this swap and a rollback cannot bring back something older.
pub fn install_script(root: &str) -> String {
    with_root(
        r#"set -eu
S='@ROOT@@STAGE@'
B='@ROOT@/opt/lanlord/bin'
W='@ROOT@/opt/lanlord/web'
rm -f "$B/.placing"
for f in "$S/lanlord" "$S/lanlord-auth-helper" "$S/web/index.html"; do
  [ -f "$f" ] || { echo "the staged files are incomplete: $f is missing" >&2; exit 1; }
done
mkdir -p "$B"
chown root:lanlord "$B"
chmod 0750 "$B"
chown root:root "$S/lanlord" "$S/lanlord-auth-helper"
chmod 0755 "$S/lanlord"
# only root runs the login helper
chmod 0700 "$S/lanlord-auth-helper"
chown -R root:root "$S/web"
find "$S/web" -type d -exec chmod 0755 {} +
find "$S/web" -type f -exec chmod 0644 {} +
# marks the swap as begun, so that an upgrade knows a failure from here on needs a rollback
: > "$B/.placing"
rm -f "$B/lanlord.prev" "$B/lanlord-auth-helper.prev"
rm -rf "$W.prev"
if [ -f "$B/lanlord" ]; then mv -f "$B/lanlord" "$B/lanlord.prev"; fi
if [ -f "$B/lanlord-auth-helper" ]; then mv -f "$B/lanlord-auth-helper" "$B/lanlord-auth-helper.prev"; fi
mv -f "$S/lanlord" "$B/lanlord"
mv -f "$S/lanlord-auth-helper" "$B/lanlord-auth-helper"
if [ -d "$W" ]; then mv "$W" "$W.prev"; fi
mv "$S/web" "$W"
rm -rf "$S"
if [ -z '@ROOT@' ] && systemctl cat lanlord-webui.service >/dev/null 2>&1; then
  systemctl restart lanlord-webui.service
fi
rm -f "$B/.placing"
echo "installed the server binary, the login helper and the browser build"
"#,
        root,
    )
}

/// wait for https on the web ui port, on the loopback and on the host's own addresses (caddy
/// binds the host address), and say what is wrong when nothing answers.
pub fn verify_script() -> String {
    with_root(
        r#"i=0
while [ "$i" -lt 20 ]; do
  # caddy binds the host's own addresses; arch's hostname has no -I, so ask ip first
  addrs=$(ip -o -4 addr show scope global 2>/dev/null | awk '{print $4}' | cut -d/ -f1)
  [ -n "$addrs" ] || addrs=$(hostname -I 2>/dev/null || true)
  for ip in 127.0.0.1 $addrs; do
    code=$(curl -sk -m 5 -o /dev/null -w '%{http_code}' "https://$ip:@PORT@/" 2>/dev/null || true)
    case "$code" in
      2??|3??|401) echo "the web ui answers on https://$ip:@PORT@/ ($code)"; exit 0 ;;
    esac
  done
  i=$((i + 1))
  sleep 1
done
echo "nothing answered on https port @PORT@" >&2
echo "lanlord-webui is $(systemctl is-active lanlord-webui.service 2>&1 | head -n 1)" >&2
echo "lanlord-caddy is running: $(docker inspect --format '{{.State.Running}}' lanlord-caddy 2>&1 | head -n 1)" >&2
exit 1
"#,
        "",
    )
}

/// the sha256 of a file, as lowercase hex.
pub fn sha256_file(path: &Path) -> Result<String> {
    let bytes =
        std::fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    Ok(sha256_bytes(&bytes))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// the browser build as a tar archive, owned by root.
pub fn tar_dir(dir: &Path) -> Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    builder.mode(tar::HeaderMode::Deterministic);
    builder
        .append_dir_all(".", dir)
        .with_context(|| format!("could not read {}", dir.display()))?;
    Ok(builder.into_inner()?)
}

/// the remote command that runs `command` as root, reading the upload from its stdin; sudo must
/// already be primed (the prepare step ran as root through `run_as_root`).
pub fn upload_command(is_root: bool, command: &str) -> Vec<String> {
    let mut cmd: Vec<String> = Vec::new();
    if !is_root {
        cmd.extend(["sudo".to_string(), "-n".to_string()]);
    }
    cmd.extend(["sh".to_string(), "-c".to_string(), command.to_string()]);
    cmd
}

/// unpack the uploaded browser build, once its hash has been checked.
pub fn extract_script() -> String {
    with_root(
        "set -eu\ntar -C @STAGE@/web -xf @STAGE@/web.tar\nrm -f @STAGE@/web.tar\n",
        "",
    )
}

/// what each upload writes on the gateway.
pub fn upload_commands() -> [(&'static str, String); 3] {
    [
        ("server binary", format!("cat > {STAGE}/lanlord")),
        ("login helper", format!("cat > {STAGE}/lanlord-auth-helper")),
        ("browser build", format!("cat > {STAGE}/web.tar")),
    ]
}

fn root(session: &Session, script: String, ask: AskPassword) -> Result<i32> {
    let admin = login_user(session)?;
    run_as_root(session, &admin.name, script, ask)
}

fn root_lines(
    session: &Session,
    script: String,
    ask: AskPassword,
    on_line: &mut dyn FnMut(&str),
) -> Result<i32> {
    let admin = login_user(session)?;
    run_as_root_lines(session, &admin.name, script, ask, on_line)
}

/// upload a local build and check that what arrived is what was sent.
fn upload(
    session: &Session,
    local: &Local,
    ask: AskPassword,
    on_line: &mut dyn FnMut(&str),
) -> Result<()> {
    local.check()?;
    if root(session, prepare_script(""), &mut *ask)? != 0 {
        bail!("could not make the staging folder {STAGE} on the server");
    }
    let is_root = login_user(session)?.is_root;
    let parts = [
        std::fs::read(&local.server_binary)
            .with_context(|| format!("could not read {}", local.server_binary.display()))?,
        std::fs::read(&local.auth_helper)
            .with_context(|| format!("could not read {}", local.auth_helper.display()))?,
        tar_dir(&local.web_dist)?,
    ];
    for ((what, command), bytes) in upload_commands().iter().zip(&parts) {
        on_line(&format!("uploading the {what} ({} bytes)", bytes.len()));
        let code = session.run_with_stdin(&upload_command(is_root, command), bytes)?;
        if code != 0 {
            bail!("the upload of the {what} failed (status {code})");
        }
    }
    let want: Vec<String> = parts.iter().map(|b| sha256_bytes(b)).collect();
    let admin = login_user(session)?;
    let got = capture_as_root(
        session,
        &admin.name,
        format!("sha256sum {STAGE}/lanlord {STAGE}/lanlord-auth-helper {STAGE}/web.tar\n"),
        &mut *ask,
    )?;
    let got: Vec<&str> = got
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    if got != want.iter().map(String::as_str).collect::<Vec<_>>() {
        let _ = root(session, format!("rm -rf '{STAGE}'\n"), &mut |_| {
            Ok(String::new())
        });
        bail!("the uploaded files do not match the local ones (sha256); nothing was installed");
    }
    // the browser build is unpacked only now that its hash matched
    if root(session, extract_script(), &mut *ask)? != 0 {
        bail!("could not unpack the browser build on the server");
    }
    Ok(())
}

/// get the files onto the gateway's stage: upload a local build, or download the release.
pub(crate) fn stage(
    session: &Session,
    source: &WebSource,
    ask: AskPassword,
    on_line: &mut dyn FnMut(&str),
) -> Result<()> {
    match source {
        WebSource::Local(local) => upload(session, local, &mut *ask, on_line),
        WebSource::Release { tag } => {
            let arch = asset_arch(&session.capture(&["uname".into(), "-m".into()])?)?;
            let assets = release_assets(tag, arch);
            on_line(&format!("getting the {tag} release for {arch}"));
            match root_lines(session, fetch_script("", &assets), &mut *ask, on_line)? {
                0 => Ok(()),
                21 => bail!(
                    "a release file does not match its sha256, so nothing was installed; \
                     {FLAGS}"
                ),
                code => bail!(
                    "could not get the {tag} release files on the server (status {code}); {FLAGS}"
                ),
            }
        }
    }
}

/// put the staged files in place, keeping the previous ones beside them.
pub(crate) fn place(
    session: &Session,
    ask: AskPassword,
    on_line: &mut dyn FnMut(&str),
) -> Result<()> {
    if root_lines(session, install_script(""), ask, on_line)? != 0 {
        bail!("could not put the web ui files in place");
    }
    Ok(())
}

/// apply the web modules with the install's settings and check that https answers.
pub(crate) fn converge(
    session: &Session,
    host: &str,
    ask: AskPassword,
    on_line: &mut dyn FnMut(&str),
) -> Result<()> {
    apply::apply_with_settings(session, host, MODULES, SETTINGS, &mut *ask, on_line)?;
    if root_lines(session, verify_script(), ask, on_line)? != 0 {
        bail!(
            "the web ui does not answer on https port {PORT} on the server; see above \
             (check `systemctl status lanlord-webui` and `docker logs lanlord-caddy`)"
        );
    }
    Ok(())
}

/// the whole step on an open session.
pub(crate) fn run_step(
    session: &Session,
    host: &str,
    source: &WebSource,
    ask: AskPassword,
    on_line: &mut dyn FnMut(&str),
) -> Result<()> {
    stage(session, source, &mut *ask, on_line)?;
    place(session, &mut *ask, on_line)?;
    converge(session, host, ask, on_line)
}

#[cfg(all(test, unix))]
mod tests {
    use crate::webinstall::{
        asset_arch, extract_script, fetch_script, install_script, prepare_script, release_assets,
        sha256_file, tar_dir, upload_command, upload_commands, verify_script, Assets, WebSource,
        MODULES, SETTINGS, STAGE,
    };
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// a scratch folder that stands in for a server and for the release site.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("lanlord-web-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("stubs")).unwrap();
            std::fs::create_dir_all(dir.join("site")).unwrap();
            // stubs: curl serves files from the site folder, chown and systemctl do nothing, and
            // sha256sum falls back to shasum where the system has no such tool
            let site = dir.join("site");
            let stub = |name: &str, body: String| {
                std::fs::write(dir.join("stubs").join(name), format!("#!/bin/sh\n{body}\n"))
                    .unwrap();
                Command::new("chmod")
                    .arg("+x")
                    .arg(dir.join("stubs").join(name))
                    .status()
                    .unwrap();
            };
            stub(
                "curl",
                format!(
                    "while [ $# -gt 1 ]; do [ \"$1\" = -o ] && out=$2; shift; done\n\
                     cp '{}'/\"$(basename \"$1\")\" \"$out\"",
                    site.display()
                ),
            );
            stub("chown", "exit 0".into());
            stub("systemctl", "exit 0".into());
            stub(
                "sha256sum",
                "for d in /usr/bin /bin; do [ -x \"$d/sha256sum\" ] && exec \"$d/sha256sum\" \"$@\"; done\n\
                 exec shasum -a 256 \"$@\""
                    .into(),
            );
            Scratch(dir)
        }

        fn root(&self) -> String {
            self.0.join("root").display().to_string()
        }

        fn site(&self, name: &str, text: &str) {
            std::fs::write(self.0.join("site").join(name), text).unwrap();
        }

        /// run a script with the stubs first in the path; returns the exit code and stderr.
        fn sh(&self, script: &str) -> (i32, String) {
            let out = Command::new("sh")
                .arg("-c")
                .arg(script)
                .env(
                    "PATH",
                    format!("{}:/usr/bin:/bin", self.0.join("stubs").display()),
                )
                .output()
                .unwrap();
            (
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stderr).into_owned(),
            )
        }

        /// make the two release files for `tag` and `arch`, with a checksum that is right or wrong.
        fn release(&self, tag: &str, arch: &str, sums_right: bool) -> Assets {
            let assets = release_assets(tag, arch);
            let build = self.0.join("build");
            let inner = build.join(assets.server.trim_end_matches(".tar.gz"));
            std::fs::create_dir_all(&inner).unwrap();
            std::fs::write(inner.join("lanlord"), "server").unwrap();
            std::fs::write(inner.join("lanlord-auth-helper"), "helper").unwrap();
            let web = build.join("web");
            std::fs::create_dir_all(web.join("sub")).unwrap();
            std::fs::write(web.join("index.html"), "<html>").unwrap();
            std::fs::write(web.join("sub/app.js"), "js").unwrap();
            let site = self.0.join("site");
            for (asset, dir, only) in [
                (
                    &assets.server,
                    &build,
                    Some(assets.server.trim_end_matches(".tar.gz")),
                ),
                (&assets.web, &web, None),
            ] {
                let mut tar = Command::new("tar");
                tar.arg("czf").arg(site.join(asset)).arg("-C").arg(dir);
                tar.arg(only.unwrap_or("."));
                assert!(tar.status().unwrap().success());
                let sum = if sums_right {
                    sha256_file(&site.join(asset)).unwrap()
                } else {
                    "0".repeat(64)
                };
                self.site(&format!("{asset}.sha256"), &format!("{sum}  {asset}\n"));
            }
            assets
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_flags_choose_the_source() {
        let dir = std::env::temp_dir().join(format!("lanlord-web-{}-choose", std::process::id()));
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        std::fs::write(dir.join("dist/index.html"), "").unwrap();
        std::fs::write(dir.join("lanlord"), "").unwrap();
        std::fs::write(dir.join("helper"), "").unwrap();
        let some = |name: &str| Some(dir.join(name));

        // no flags on a development build: refused, and the message says what to pass
        let err = WebSource::choose_for(true, None, None, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("development build"), "{err}");
        assert!(
            err.contains("--server-binary, --web-dist and --auth-helper"),
            "{err}"
        );
        // the flags still work on one
        assert!(WebSource::choose_for(true, some("lanlord"), some("dist"), some("helper")).is_ok());
        // no flags: this version's release
        assert_eq!(
            WebSource::choose_for(false, None, None, None).unwrap(),
            WebSource::Release {
                tag: format!("v{}", env!("CARGO_PKG_VERSION"))
            }
        );
        // all three: the local build
        let WebSource::Local(local) =
            WebSource::choose(some("lanlord"), some("dist"), some("helper")).unwrap()
        else {
            panic!("not a local build");
        };
        assert_eq!(local.server_binary, dir.join("lanlord"));
        // some of them: a mistake that names the flags
        for (a, b, c) in [
            (some("lanlord"), None, None),
            (None, some("dist"), some("helper")),
            (some("lanlord"), some("dist"), None),
        ] {
            let err = WebSource::choose(a, b, c).unwrap_err().to_string();
            assert!(
                err.contains("--server-binary") && err.contains("--web-dist"),
                "{err}"
            );
        }
        // a path that is not there
        let err = WebSource::choose(some("nope"), some("dist"), some("helper"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("--server-binary"), "{err}");
        let err = WebSource::choose(some("lanlord"), some("lanlord"), some("helper"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("index.html"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_release_asset_names_follow_the_architecture() {
        for (uname, arch) in [
            ("x86_64\n", "x86_64"),
            ("amd64", "x86_64"),
            ("aarch64", "arm64"),
            ("arm64", "arm64"),
        ] {
            assert_eq!(asset_arch(uname).unwrap(), arch);
        }
        let err = asset_arch("riscv64").unwrap_err().to_string();
        assert!(
            err.contains("riscv64") && err.contains("--server-binary"),
            "{err}"
        );

        let a = release_assets("v1.2.3", "x86_64");
        assert_eq!(a.server, "lanlord-v1.2.3-linux-x86_64-headless.tar.gz");
        assert_eq!(a.web, "lanlord-v1.2.3-web.tar.gz");
        assert_eq!(
            a.base,
            "https://github.com/muncherelli/LANlord/releases/download/v1.2.3"
        );
        let a = release_assets("v1.2.3", "arm64");
        assert_eq!(a.server, "lanlord-v1.2.3-linux-arm64-headless.tar.gz");
    }

    #[test]
    fn a_release_that_matches_its_checksums_is_staged() {
        let s = Scratch::new("fetch-ok");
        let assets = s.release("v9.9.9", "x86_64", true);
        let (code, err) = s.sh(&fetch_script(&s.root(), &assets));
        assert_eq!(code, 0, "{err}");
        let stage = PathBuf::from(format!("{}{STAGE}", s.root()));
        assert_eq!(
            std::fs::read_to_string(stage.join("lanlord")).unwrap(),
            "server"
        );
        assert!(stage.join("lanlord-auth-helper").is_file());
        assert!(stage.join("web/sub/app.js").is_file());
        assert!(!stage.join("dl").exists());
    }

    #[test]
    fn a_release_with_a_wrong_checksum_is_refused_and_nothing_is_left() {
        let s = Scratch::new("fetch-bad");
        let assets = s.release("v9.9.9", "x86_64", false);
        let (code, err) = s.sh(&fetch_script(&s.root(), &assets));
        assert_eq!(code, 21, "{err}");
        assert!(err.contains("refusing"), "{err}");
        assert!(!PathBuf::from(format!("{}{STAGE}", s.root())).exists());
    }

    #[test]
    fn a_missing_release_is_reported_as_a_failed_download() {
        let s = Scratch::new("fetch-missing");
        let (code, err) = s.sh(&fetch_script(&s.root(), &release_assets("v9.9.9", "arm64")));
        assert_eq!(code, 20, "{err}");
        assert!(err.contains("could not download"), "{err}");
    }

    #[test]
    fn the_install_keeps_the_previous_version_beside_the_new_one() {
        let s = Scratch::new("install");
        let assets = s.release("v9.9.9", "x86_64", true);
        let root = s.root();
        let opt = PathBuf::from(format!("{root}/opt/lanlord"));
        std::fs::create_dir_all(opt.join("bin")).unwrap();
        std::fs::create_dir_all(opt.join("web")).unwrap();
        std::fs::write(opt.join("bin/lanlord"), "old server").unwrap();
        std::fs::write(opt.join("bin/lanlord-auth-helper"), "old helper").unwrap();
        std::fs::write(opt.join("web/index.html"), "old page").unwrap();
        for script in [fetch_script(&root, &assets), install_script(&root)] {
            let (code, err) = s.sh(&script);
            assert_eq!(code, 0, "{err}");
        }
        let read = |p: &str| std::fs::read_to_string(opt.join(p)).unwrap();
        assert_eq!(read("bin/lanlord"), "server");
        assert_eq!(read("bin/lanlord.prev"), "old server");
        assert_eq!(read("bin/lanlord-auth-helper"), "helper");
        assert_eq!(read("bin/lanlord-auth-helper.prev"), "old helper");
        // the marker of a swap in progress is gone once the swap is done
        assert!(!opt.join("bin/.placing").exists());
        assert_eq!(read("web/index.html"), "<html>");
        assert_eq!(read("web.prev/index.html"), "old page");
        assert!(!opt.join("web-stage").exists());
        let mode = |p: &str| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(opt.join(p)).unwrap().permissions().mode() & 0o777
        };
        assert_eq!(mode("bin/lanlord"), 0o755);
        assert_eq!(mode("bin/lanlord-auth-helper"), 0o700);
        assert_eq!(mode("web/sub/app.js"), 0o644);
    }

    #[test]
    fn an_incomplete_stage_is_not_installed() {
        let s = Scratch::new("incomplete");
        let (code, err) = s.sh(&format!(
            "{}\n{}",
            prepare_script(&s.root()),
            install_script(&s.root())
        ));
        assert_eq!(code, 1);
        assert!(err.contains("incomplete"), "{err}");
    }

    #[test]
    fn the_upload_commands_write_into_the_stage_as_root() {
        let [(_, bin), (_, helper), (_, web)] = upload_commands();
        assert_eq!(bin, "cat > /opt/lanlord/web-stage/lanlord");
        assert_eq!(helper, "cat > /opt/lanlord/web-stage/lanlord-auth-helper");
        assert_eq!(web, "cat > /opt/lanlord/web-stage/web.tar");
        assert!(extract_script()
            .contains("tar -C /opt/lanlord/web-stage/web -xf /opt/lanlord/web-stage/web.tar"));
        assert!(fetch_script("", &release_assets("v1", "arm64"))
            .contains("--proto '=https' --proto-redir '=https' --tlsv1.2"));
        assert_eq!(upload_command(true, &bin), ["sh", "-c", bin.as_str()]);
        assert_eq!(
            upload_command(false, &bin),
            ["sudo", "-n", "sh", "-c", bin.as_str()]
        );
        // the stage is made before anything is uploaded, readable by root only
        assert!(prepare_script("").contains("chmod 0700 \"$S\""));
    }

    #[test]
    fn the_browser_build_is_uploaded_as_a_tar_of_its_files() {
        let s = Scratch::new("tar");
        let dist = s.0.join("dist");
        std::fs::create_dir_all(dist.join("sub")).unwrap();
        std::fs::write(dist.join("index.html"), "<html>").unwrap();
        std::fs::write(dist.join("sub/app.js"), "js").unwrap();
        let bytes = tar_dir(&dist).unwrap();
        let mut archive = tar::Archive::new(bytes.as_slice());
        let mut names: Vec<String> = archive
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().display().to_string())
            .filter(|n| n.ends_with(".html") || n.ends_with(".js"))
            .collect();
        names.sort();
        assert_eq!(names, ["index.html", "sub/app.js"]);
    }

    #[test]
    fn the_defaults_need_no_lanlord_network_and_serve_only_the_web_ui_port() {
        let settings: serde_yaml_ng::Value = serde_yaml_ng::from_str(SETTINGS).unwrap();
        assert_eq!(settings["networks"].as_sequence().unwrap().len(), 0);
        let proxy = &settings["web-proxy"];
        assert_eq!(proxy["webui"]["tls"], "internal");
        assert_eq!(proxy["status"]["enabled"], false);
        assert!(proxy.get("sites").is_none() && proxy.get("listen").is_none());
        assert_eq!(MODULES, ["web-proxy", "webui-login"]);
        assert!(verify_script().contains("https://$ip:8443/"));
    }

    #[test]
    fn the_playbooks_give_the_service_and_the_port_the_defaults_rely_on() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ansible/playbooks");
        let proxy = std::fs::read_to_string(dir.join("web-proxy.yml")).unwrap();
        for want in [
            "dest: /etc/systemd/system/lanlord-webui.service",
            "User=lanlord",
            "ExecStart=/opt/lanlord/bin/lanlord webui --socket",
            "webui_cfg.port | default(8443)",
            "name: lanlord-webui.service",
        ] {
            assert!(proxy.contains(want), "web-proxy.yml lacks {want}");
        }
        let login = std::fs::read_to_string(dir.join("webui-login.yml")).unwrap();
        assert!(login.contains("staged: /opt/lanlord/bin/lanlord-auth-helper"));
        // with only the web ui, nothing listens on port 80
        let caddyfile = std::fs::read_to_string(dir.join("web-proxy/Caddyfile.j2")).unwrap();
        assert!(caddyfile.contains("{% if proxy_tls == 'off' and proxy_sites | length > 0 %}"));
    }
}
