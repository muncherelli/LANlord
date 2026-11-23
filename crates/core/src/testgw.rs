//! a fake gateway for tests: the plain and the encrypted settings live in a directory.

use anyhow::{bail, Result};

use crate::crypto;
use crate::ssh::Session;

pub fn identity(name: &str) -> age::x25519::Identity {
    let path = std::env::temp_dir()
        .join(format!("lanlord-settings-{}-{name}", std::process::id()))
        .join("identity");
    crypto::generate(&path).unwrap();
    let identity = crypto::load(&path).unwrap();
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
    identity
}

/// `text` without the two version lines a stored settings file starts with.
pub fn unstamped(text: &str) -> String {
    lanlord_model::format::without_stamp(text)
}

pub fn no_password(_: &str) -> Result<String> {
    bail!("no password expected")
}

/// a fake gateway that keeps the plain and the encrypted settings in a directory.
pub struct Gateway {
    pub dir: std::path::PathBuf,
    pub session: Session,
}

impl Gateway {
    pub fn new(name: &str) -> Self {
        Self::build(name, true)
    }

    /// a gateway where lanlord was never installed: there is no lanlord account or /opt/lanlord.
    pub fn not_installed(name: &str) -> Self {
        Self::build(name, false)
    }

    fn build(name: &str, installed: bool) -> Self {
        let dir = std::env::temp_dir().join(format!("lanlord-gw-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let payload = r#"printf '%s\n' "$input" | sed -n '/^base64 -d/,/^LANLORD_PAYLOAD$/p' | sed '1d;$d' | base64 -d"#;
        let script = r##"D='@DIR@'
for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "id -u svc") echo 998 ;;
  "id -u ghost") exit 1 ;;
  "id -u flaky") echo 1005 ;;
  "id -nG flaky") exit 1 ;;
  "id -u "*) echo 1001 ;;
  "id -nG boss") echo "boss admins" ;;
  "id -nG "*) echo "users" ;;
  "sudo -n true") exit 0 ;;
  "sh -c "*"getent passwd lanlord"*) echo @INSTALLED@ ;;
  *"lanlord-user exec")
    if [ -f "$D/fail-exec" ] && case "$last" in *"$(cat "$D/fail-exec")"*) true ;; *) false ;; esac; then
      cat > /dev/null; echo boom >&2; exit 1
    fi
    printf '%s\n' "$last" >> "$D/argv"; cat > "$D/stdin" ;;
  *"lanlord-user list")
    c=${last##*docker exec }; c=${c%% *}
    if [ -f "$D/accounts-$c" ]; then cat "$D/accounts-$c"; fi ;;
  *"lanlord-exec run"*) printf '%s\n' "$last" >> "$D/calls"; cat > /dev/null ;;
  "sh -c "*settings.yml*) if [ -f "$D/yml" ]; then echo present; cat "$D/yml"; fi ;;
  *)
input=$(cat)
case "$input" in
  *"tr ',' '\\n' | sed '/^$/d' | sort"*) if [ -f "$D/fail-admins" ]; then echo "no getent" >&2; exit 1; fi ;;
  "# stored restore"*) if [ -f "$D/fail-restore" ]; then echo "read-only" >&2; exit 1; fi
    printf '%s\n' "$input" | sed "s#/opt/lanlord/config/settings.yml#$D/yml#g; s#/opt/lanlord/config/settings.age#$D/age#g" | sh ;;
  "# stored backup"*) printf '%s\n' "$input" | sed "s#/opt/lanlord/config/settings.yml#$D/yml#g; s#/opt/lanlord/config/settings.age#$D/age#g" | sh ;;
  *"> /opt/lanlord/config/settings.yml.tmp"*) @PAYLOAD@ > "$D/yml" ;;
  *"> /opt/lanlord/config/settings.age.tmp"*)
    if [ -f "$D/fail-age" ]; then echo "disk full" >&2; exit 1; fi
    @PAYLOAD@ > "$D/age" ;;
  *"lanlord-user machine "*) printf '%s\n' "$input" >> "$D/calls" ;;
  *"base64 < /opt/lanlord/config/settings.age"*) if [ -f "$D/age" ]; then base64 < "$D/age"; fi ;;
esac ;;
esac"##
            .replace("@INSTALLED@", if installed { "yes" } else { "no" })
            .replace("@DIR@", &dir.display().to_string())
            .replace("@PAYLOAD@", payload);
        Self {
            dir,
            session: Session::fake(&script),
        }
    }

    /// the plain settings as stored, without the version stamp.
    pub fn plain(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.join("yml"))
            .ok()
            .map(|text| unstamped(&text))
    }

    pub fn sealed(&self, id: &age::x25519::Identity) -> Option<String> {
        let bytes = std::fs::read(self.dir.join("age")).ok()?;
        Some(unstamped(
            &String::from_utf8(crypto::decrypt(id, &bytes).unwrap()).unwrap(),
        ))
    }

    /// the sealed text as stored, version stamp included.
    pub fn sealed_raw(&self, id: &age::x25519::Identity) -> Option<String> {
        let bytes = std::fs::read(self.dir.join("age")).ok()?;
        Some(String::from_utf8(crypto::decrypt(id, &bytes).unwrap()).unwrap())
    }

    /// the plain settings, written as a user would.
    pub fn seed_plain(&self, text: &str) {
        std::fs::write(self.dir.join("yml"), text).unwrap();
    }

    /// the arguments of each `lanlord-user exec` call the gateway saw, and the stdin of the last.
    pub fn exec_calls(&self) -> (Vec<String>, Vec<u8>) {
        let argv = std::fs::read_to_string(self.dir.join("argv")).unwrap_or_default();
        let stdin = std::fs::read(self.dir.join("stdin")).unwrap_or_default();
        (argv.lines().map(String::from).collect(), stdin)
    }

    /// make the `lanlord-user exec` call whose command names `container` fail.
    pub fn fail_exec_on(&self, container: &str) {
        std::fs::write(self.dir.join("fail-exec"), container).unwrap();
    }

    /// the accounts `lanlord-user list` prints in `container`, one `name:uid:` line each.
    pub fn seed_accounts(&self, container: &str, lines: &str) {
        std::fs::write(self.dir.join(format!("accounts-{container}")), lines).unwrap();
    }

    /// the `lanlord-user machine` scripts and `lanlord-exec run` calls the gateway saw.
    pub fn calls(&self) -> Vec<String> {
        let text = std::fs::read_to_string(self.dir.join("calls")).unwrap_or_default();
        text.lines().map(String::from).collect()
    }

    pub fn seed_sealed(&self, id: &age::x25519::Identity, text: &str) {
        std::fs::write(
            self.dir.join("age"),
            crypto::encrypt(id, text.as_bytes()).unwrap(),
        )
        .unwrap();
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
