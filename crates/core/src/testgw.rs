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
        let script = r#"D='@DIR@'
for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sudo -n true") exit 0 ;;
  "sh -c "*"getent passwd lanlord"*) echo @INSTALLED@ ;;
  "sh -c "*settings.yml*) if [ -f "$D/yml" ]; then echo present; cat "$D/yml"; fi ;;
  *)
input=$(cat)
case "$input" in
  *"tee /opt/lanlord/config/settings.yml"*) @PAYLOAD@ > "$D/yml" ;;
  *"tee /opt/lanlord/config/settings.age"*) @PAYLOAD@ > "$D/age" ;;
  *"base64 < /opt/lanlord/config/settings.age"*) if [ -f "$D/age" ]; then base64 < "$D/age"; fi ;;
esac ;;
esac"#
            .replace("@INSTALLED@", if installed { "yes" } else { "no" })
            .replace("@DIR@", &dir.display().to_string())
            .replace("@PAYLOAD@", payload);
        Self {
            dir,
            session: Session::fake(&script),
        }
    }

    pub fn plain(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.join("yml")).ok()
    }

    pub fn sealed(&self, id: &age::x25519::Identity) -> Option<String> {
        let bytes = std::fs::read(self.dir.join("age")).ok()?;
        Some(String::from_utf8(crypto::decrypt(id, &bytes).unwrap()).unwrap())
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
