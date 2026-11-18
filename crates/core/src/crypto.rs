//! the client's age key, and encrypting settings with it. the key never
//! leaves this machine; the gateway only ever stores ciphertext.

use std::fs;
use std::io::Write;
use std::path::Path;

use age::secrecy::ExposeSecret;
pub use age::x25519::Identity;
use anyhow::{bail, Context, Result};

/// create a new identity at `path` and return its public recipient.
pub fn generate(path: &Path) -> Result<String> {
    if path.exists() {
        bail!("{} already exists; not overwriting it", path.display());
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let identity = Identity::generate();
    let recipient = identity.to_public().to_string();
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    writeln!(file, "# public key: {recipient}")?;
    writeln!(file, "{}", identity.to_string().expose_secret())?;
    Ok(recipient)
}

/// read the identity at `path`.
pub fn load(path: &Path) -> Result<Identity> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("no key at {}; run lanlord keygen first", path.display()))?;
    let secret = text
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("AGE-SECRET-KEY-"))
        .with_context(|| format!("no age key found in {}", path.display()))?;
    secret
        .parse::<Identity>()
        .map_err(|err| anyhow::anyhow!("{} is not a valid age key: {err}", path.display()))
}

/// where the default key is kept, when something other than the user's config folder says so.
static DEFAULT_PATH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// keep the default key at `path` for the rest of this process: the gateway's operations
/// service has no home folder to look in. only the first call counts.
pub fn set_default_path(path: std::path::PathBuf) {
    let _ = DEFAULT_PATH.set(path);
}

/// the default key, or `None` when there is none yet; a damaged key is an error.
pub fn load_default() -> Result<Option<Identity>> {
    match DEFAULT_PATH
        .get()
        .cloned()
        .or_else(crate::paths::identity_path)
    {
        Some(path) if path.exists() => load(&path).map(Some),
        _ => Ok(None),
    }
}

/// encrypt `plaintext` so only `identity` can read it.
pub fn encrypt(identity: &Identity, plaintext: &[u8]) -> Result<Vec<u8>> {
    Ok(age::encrypt(&identity.to_public(), plaintext)?)
}

/// decrypt what `encrypt` produced.
pub fn decrypt(identity: &Identity, ciphertext: &[u8]) -> Result<Vec<u8>> {
    age::decrypt(identity, ciphertext).map_err(|err| anyhow::anyhow!("could not decrypt: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("lanlord-crypto-{}-{name}", std::process::id()))
            .join("identity")
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn generated_key_loads_back_and_round_trips() {
        let path = temp("roundtrip");
        let recipient = generate(&path).unwrap();
        assert!(recipient.starts_with("age1"));
        let identity = load(&path).unwrap();
        assert_eq!(identity.to_public().to_string(), recipient);
        let sealed = encrypt(&identity, b"motd: hello\n").unwrap();
        assert!(!String::from_utf8_lossy(&sealed).contains("motd"));
        assert_eq!(decrypt(&identity, &sealed).unwrap(), b"motd: hello\n");
        cleanup(&path);
    }

    #[cfg(unix)]
    #[test]
    fn the_key_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp("mode");
        generate(&path).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        cleanup(&path);
    }

    #[test]
    fn generate_refuses_to_overwrite() {
        let path = temp("overwrite");
        generate(&path).unwrap();
        assert!(generate(&path).is_err());
        cleanup(&path);
    }

    #[test]
    fn another_key_cannot_decrypt() {
        let (a, b) = (temp("a"), temp("b"));
        generate(&a).unwrap();
        generate(&b).unwrap();
        let sealed = encrypt(&load(&a).unwrap(), b"secret").unwrap();
        assert!(decrypt(&load(&b).unwrap(), &sealed).is_err());
        cleanup(&a);
        cleanup(&b);
    }

    #[test]
    fn load_explains_a_missing_or_bad_key() {
        let missing = temp("missing");
        let err = load(&missing).err().unwrap().to_string();
        assert!(err.contains("lanlord keygen"), "{err}");
        let bad = temp("bad");
        fs::create_dir_all(bad.parent().unwrap()).unwrap();
        fs::write(&bad, "not a key\n").unwrap();
        assert!(load(&bad).err().unwrap().to_string().contains("no age key"));
        fs::write(&bad, "AGE-SECRET-KEY-nonsense\n").unwrap();
        assert!(load(&bad)
            .err()
            .unwrap()
            .to_string()
            .contains("not a valid"));
        cleanup(&bad);
    }
}
