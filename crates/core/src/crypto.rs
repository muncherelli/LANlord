//! the gateway's age key, and encrypting settings with it. only the gateway's root operations
//! service (or `lanlord operate` under sudo) ever loads the key: it is created root-only on
//! first start and never leaves the gateway. clients hold no key and never decrypt; they send
//! secrets to the service over their authenticated channel and read back only "set" or "not set".

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
    let text = fs::read_to_string(path).with_context(|| format!("no key at {}", path.display()))?;
    parse(&text).with_context(|| format!("the key at {}", path.display()))
}

/// the identity in `text`, a key file as `generate` writes it.
pub fn parse(text: &str) -> Result<Identity> {
    let secret = text
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("AGE-SECRET-KEY-"))
        .context("no age key found")?;
    secret
        .parse::<Identity>()
        .map_err(|err| anyhow::anyhow!("not a valid age key: {err}"))
}

/// where the gateway's key is kept, once the service or `lanlord operate` has said so.
static DEFAULT_PATH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// where the gateway keeps its key unless told otherwise.
pub const GATEWAY_KEY: &str = "/var/lib/lanlord-operations/identity";

/// use the key at `path` for the rest of this process, and make sure it exists: only the
/// gateway's root service does this. the load is retried, so a service never runs without a
/// key because another starter was making it at the same moment. only the first call counts.
pub fn use_gateway_key(path: std::path::PathBuf) -> Result<()> {
    let mut tries = 0;
    loop {
        match ensure(&path) {
            Ok(_) => break,
            Err(error) if tries >= 5 => return Err(error),
            Err(_) => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
    let _ = DEFAULT_PATH.set(path);
    Ok(())
}

/// the gateway's key at `path`: created (root only, mode 0600 in a 0700 folder) when it is not
/// there yet. the key is written whole to a private temp file in the same folder, synced, and
/// linked to its name, which fails when another starter got there first: then the temp file goes
/// and the winner's key is the one loaded. so a key file is never seen half written.
pub fn ensure(path: &Path) -> Result<Identity> {
    if !path.exists() {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                // a folder this call made is private; one that was there keeps its mode
                if fs::read_dir(dir)?.next().is_none() {
                    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
                }
            }
        }
        create_atomically(path)?;
    }
    load(path)
}

/// write a new key to a temp file beside `path` and link it into place.
fn create_atomically(path: &Path) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let temp = dir.join(format!(".identity-{}.tmp", std::process::id()));
    let _ = fs::remove_file(&temp);
    generate(&temp)?;
    fs::File::open(&temp)?.sync_all()?;
    let linked = fs::hard_link(&temp, path);
    let _ = fs::remove_file(&temp);
    match linked {
        Ok(()) => {}
        // lost the race: the winner's key is the key
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    // a folder cannot be opened to sync on every platform; where it can, the link is made durable
    #[cfg(unix)]
    fs::File::open(dir)?.sync_all()?;
    Ok(())
}

/// the gateway's key. on a client there is none, by design: it fails there instead of finding a
/// key in a home folder.
pub fn load_default() -> Result<Option<Identity>> {
    match DEFAULT_PATH.get() {
        Some(path) => load(path).map(Some),
        None => bail!("{CLIENT_HAS_NO_KEY}"),
    }
}

/// what a client is told when something asks it to touch secrets.
pub const CLIENT_HAS_NO_KEY: &str =
    "secrets are kept and used on the gateway; this machine holds no key";

/// the settings were not sealed to this gateway's key (an older client's key sealed them):
/// they need moving to the gateway first.
#[derive(Debug)]
pub struct NotOurKey;

impl std::fmt::Display for NotOurKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the stored secrets are sealed to another key; move them to the gateway first")
    }
}

impl std::error::Error for NotOurKey {}

/// encrypt `plaintext` so only `identity` can read it.
pub fn encrypt(identity: &Identity, plaintext: &[u8]) -> Result<Vec<u8>> {
    Ok(age::encrypt(&identity.to_public(), plaintext)?)
}

/// decrypt what `encrypt` produced.
pub fn decrypt(identity: &Identity, ciphertext: &[u8]) -> Result<Vec<u8>> {
    age::decrypt(identity, ciphertext).map_err(|err| match err {
        age::DecryptError::NoMatchingKeys => NotOurKey.into(),
        err => anyhow::anyhow!("could not decrypt: {err}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("lanlord-crypto-{}-{name}", std::process::id()))
            .join("identity")
    }

    #[test]
    #[cfg(unix)]
    fn the_gateway_makes_its_key_once_root_only_and_keeps_it() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp("ensure");
        let first = ensure(&path).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        // a second start finds the same key
        let again = ensure(&path).unwrap();
        assert_eq!(first.to_public().to_string(), again.to_public().to_string());
        cleanup(&path);
    }

    #[test]
    fn two_starters_end_with_one_key_and_no_temp_file_is_left() {
        let path = temp("race");
        let keys: Vec<String> = std::thread::scope(|scope| {
            let starters: Vec<_> = (0..4)
                .map(|_| scope.spawn(|| ensure(&path).map(|k| k.to_public().to_string())))
                .collect();
            starters
                .into_iter()
                .filter_map(|s| s.join().unwrap().ok())
                .collect()
        });
        assert!(!keys.is_empty());
        assert!(keys.iter().all(|k| *k == keys[0]));
        let left: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["identity"]);
        cleanup(&path);
    }

    #[test]
    fn a_key_that_did_not_seal_the_text_is_told_apart() {
        let (ours, theirs) = (Identity::generate(), Identity::generate());
        let sealed = encrypt(&ours, b"x").unwrap();
        let err = decrypt(&theirs, &sealed).unwrap_err();
        assert!(err.downcast_ref::<NotOurKey>().is_some(), "{err:#}");
        assert_eq!(decrypt(&ours, &sealed).unwrap(), b"x");
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
        assert!(err.contains("no key at"), "{err}");
        let bad = temp("bad");
        fs::create_dir_all(bad.parent().unwrap()).unwrap();
        fs::write(&bad, "not a key\n").unwrap();
        assert!(format!("{:#}", load(&bad).err().unwrap()).contains("no age key"));
        fs::write(&bad, "AGE-SECRET-KEY-nonsense\n").unwrap();
        assert!(format!("{:#}", load(&bad).err().unwrap()).contains("not a valid"));
        cleanup(&bad);
    }
}
