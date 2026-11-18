//! your decrypted settings file, with typed access to the `dns` section.
//! everything outside the parts a front end edits is kept as it was.

use anyhow::{bail, Result};
use serde_yaml_ng::{Mapping, Value};

use crate::bootstrap::AskPassword;
use crate::config;
use crate::ssh::Session;
use crate::users::cached;

pub use lanlord_model::settings::*;

pub(crate) fn load_document_with(
    session: &Session,
    identity: &age::x25519::Identity,
    ask: AskPassword,
) -> Result<Document> {
    let mut ask = cached(ask);
    let plain = config::pull_plain_with(session, &mut ask)?;
    let sealed = match config::pull_with(session, identity, &mut ask) {
        Ok(sealed) => sealed,
        // sealed to an older client's key: the plain settings still show, and the document says
        // the secrets have to be moved before they can be used
        Err(error) if error.is::<crate::crypto::NotOurKey>() => {
            return Ok(Document {
                plain: plain.map(text_of_bytes).transpose()?.unwrap_or_default(),
                needs_move: true,
                ..Document::default()
            });
        }
        Err(error) => return Err(error),
    };
    match (plain, sealed) {
        (Some(plain), sealed) => Ok(Document {
            plain: text_of_bytes(plain)?,
            secrets_set: match sealed {
                Some(bytes) => secret_paths(&parse_value(&bytes)?),
                None => Vec::new(),
            },
            legacy: false,
            needs_move: false,
        }),
        (None, Some(bytes)) => {
            let (kept, secrets) = split(&parse_value(&bytes)?);
            let secrets_set = secret_paths(&secrets);
            let plain = if secrets_set.is_empty() {
                text_of_bytes(bytes)?
            } else {
                dump(&kept)?
            };
            Ok(Document {
                plain,
                secrets_set,
                legacy: true,
                needs_move: false,
            })
        }
        (None, None) => Ok(Document::default()),
    }
}

/// seal the stored secrets to the gateway's key instead of the key `old` (an older client's):
/// the one-time move. what is already sealed to the gateway's key is left as it is. it returns
/// whether anything was moved. the old key travels only over the caller's authenticated channel
/// and is dropped with the request.
pub(crate) fn move_secrets_with(
    session: &Session,
    host: &str,
    gateway: &age::x25519::Identity,
    old: &age::x25519::Identity,
    ask: AskPassword,
) -> Result<bool> {
    let mut ask = cached(ask);
    let Some(sealed) = config::pull_sealed_with(session, &mut ask)? else {
        return Ok(false);
    };
    if crate::crypto::decrypt(gateway, &sealed).is_ok() {
        return Ok(false);
    }
    let plaintext = crate::crypto::decrypt(old, &sealed)?;
    config::push_with(session, host, gateway, &plaintext, &mut ask)?;
    Ok(true)
}

/// the settings the playbooks get: the plain settings with the secrets laid
/// over them. an old all-in-one encrypted file counts as the whole thing.
pub(crate) fn merged_with(
    session: &Session,
    identity: Option<&age::x25519::Identity>,
    ask: AskPassword,
) -> Result<Option<Vec<u8>>> {
    let mut ask = cached(ask);
    let plain = config::pull_plain_with(session, &mut ask)?;
    let sealed = match identity {
        Some(identity) => config::pull_with(session, identity, &mut ask)?,
        None => None,
    };
    match (plain, sealed) {
        (Some(plain), Some(sealed)) => {
            let merged = merge(&parse_value(&plain)?, &parse_value(&sealed)?);
            Ok(Some(dump(&merged)?.into_bytes()))
        }
        (plain, sealed) => Ok(plain.or(sealed)),
    }
}

/// store `plain_text` as the plain settings and apply `updates` (secret
/// path, new value) to the encrypted ones; blank values leave a secret as it is.
pub(crate) fn save_document_with(
    session: &Session,
    host: &str,
    identity: &age::x25519::Identity,
    plain_text: &str,
    updates: &[(String, String)],
    ask: AskPassword,
) -> Result<()> {
    save_changes_with(session, host, identity, plain_text, updates, &[], ask)
}

/// like `save_document_with`, and the secrets at the `removed` paths are forgotten.
pub(crate) fn save_changes_with(
    session: &Session,
    host: &str,
    identity: &age::x25519::Identity,
    plain_text: &str,
    updates: &[(String, String)],
    removed: &[String],
    ask: AskPassword,
) -> Result<()> {
    let leaked = secret_paths(&parse_value(plain_text.as_bytes())?);
    if !leaked.is_empty() {
        bail!(
            "the plain settings must not hold secrets ({}); remove them and use the Set buttons",
            leaked.join(", ")
        );
    }
    let mut ask = cached(ask);
    let stored = config::pull_with(session, identity, &mut ask)?
        .map(|bytes| parse_value(&bytes))
        .transpose()?;
    let mut secrets = stored
        .as_ref()
        .map_or_else(|| Value::Mapping(Mapping::new()), |v| split(v).1);
    for path in removed {
        remove_secret(&mut secrets, path)?;
    }
    for (path, value) in updates.iter().filter(|(_, value)| !value.is_empty()) {
        set_secret(&mut secrets, path, value)?;
    }
    config::push_plain_with(session, host, plain_text.as_bytes(), &mut ask)?;
    let unchanged = match &stored {
        Some(stored) => *stored == secrets,
        None => secrets.as_mapping().is_some_and(Mapping::is_empty),
    };
    if !unchanged {
        config::push_with(
            session,
            host,
            identity,
            dump(&secrets)?.as_bytes(),
            &mut ask,
        )?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod gateway_tests {
    use super::*;
    use crate::testgw::{identity, no_password, Gateway};

    const LEGACY: &str = "dns:\n  zone: lab\nnt4-domain:\n  admin_password: hunter2\n  users:\n    - name: dana\n      password: pw1\n";

    #[test]
    fn an_empty_gateway_loads_as_nothing() {
        let (gw, id) = (Gateway::new("empty"), identity("empty"));
        let doc = load_document_with(&gw.session, &id, &mut no_password).unwrap();
        assert_eq!(doc, Document::default());
    }

    #[test]
    fn the_old_encrypted_file_loads_as_legacy_with_secrets_split_off() {
        let (gw, id) = (Gateway::new("legacy"), identity("legacy"));
        gw.seed_sealed(&id, LEGACY);
        let doc = load_document_with(&gw.session, &id, &mut no_password).unwrap();
        assert!(doc.legacy);
        assert!(!doc.plain.contains("hunter2") && !doc.plain.contains("pw1"));
        assert!(doc.plain.contains("zone: lab"));
        assert_eq!(
            doc.secrets_set,
            [
                "nt4-domain.admin_password",
                "nt4-domain.users[dana].password"
            ]
        );
        // with no secrets in it, the text comes back untouched, comments and all
        let (gw, id) = (Gateway::new("legacy2"), identity("legacy2"));
        gw.seed_sealed(&id, "# note\ndns: {}\n");
        let doc = load_document_with(&gw.session, &id, &mut no_password).unwrap();
        assert_eq!(
            (doc.plain.as_str(), doc.legacy),
            ("# note\ndns: {}\n", true)
        );
    }

    #[test]
    fn the_first_save_migrates_to_the_two_stores() {
        let (gw, id) = (Gateway::new("migrate"), identity("migrate"));
        gw.seed_sealed(&id, LEGACY);
        let text = "# plain now\ndns:\n  zone: lab2\nnt4-domain:\n  users:\n    - name: dana\n";
        save_document_with(&gw.session, "gw", &id, text, &[], &mut no_password).unwrap();
        assert_eq!(gw.plain().as_deref(), Some(text), "exactly what was typed");
        let sealed = gw.sealed(&id).unwrap();
        assert!(sealed.contains("hunter2") && sealed.contains("pw1"));
        assert!(
            !sealed.contains("zone"),
            "the encrypted file holds secrets only"
        );
        let doc = load_document_with(&gw.session, &id, &mut no_password).unwrap();
        assert!(!doc.legacy);
        assert_eq!(doc.plain, text);
        assert_eq!(doc.secrets_set.len(), 2);
    }

    #[test]
    fn apply_gets_the_plain_settings_with_the_secrets_laid_over() {
        let (gw, id) = (Gateway::new("merged"), identity("merged"));
        gw.seed_sealed(&id, LEGACY);
        let before = merged_with(&gw.session, Some(&id), &mut no_password)
            .unwrap()
            .unwrap();
        assert_eq!(before, LEGACY.as_bytes(), "legacy is used as it is");
        let text = "dns:\n  zone: lab2\nnt4-domain:\n  users:\n    - name: dana\n";
        save_document_with(&gw.session, "gw", &id, text, &[], &mut no_password).unwrap();
        let merged = merged_with(&gw.session, Some(&id), &mut no_password)
            .unwrap()
            .unwrap();
        let merged = parse_value(&merged).unwrap();
        assert_eq!(merged["dns"]["zone"], Value::String("lab2".into()));
        assert_eq!(
            merged["nt4-domain"]["admin_password"],
            Value::String("hunter2".into())
        );
        assert_eq!(
            merged["nt4-domain"]["users"][0]["password"],
            Value::String("pw1".into())
        );
        let plain_only = merged_with(&gw.session, None, &mut no_password)
            .unwrap()
            .unwrap();
        assert!(!String::from_utf8(plain_only).unwrap().contains("hunter2"));
        let nothing = Gateway::new("nothing");
        assert!(merged_with(&nothing.session, Some(&id), &mut no_password)
            .unwrap()
            .is_none());
    }

    #[test]
    fn typed_secrets_are_stored_encrypted_and_blank_ones_change_nothing() {
        let (gw, id) = (Gateway::new("secrets"), identity("secrets"));
        let text = "nt4-domain:\n  name: RETRO\n";
        let set = |path: &str, value: &str| vec![(path.to_string(), value.to_string())];
        save_document_with(
            &gw.session,
            "gw",
            &id,
            text,
            &set("nt4-domain.admin_password", "s3cret"),
            &mut no_password,
        )
        .unwrap();
        assert!(!gw.plain().unwrap().contains("s3cret"));
        assert!(gw.sealed(&id).unwrap().contains("s3cret"));
        let raw = std::fs::read(gw.dir.join("age")).unwrap();
        save_document_with(
            &gw.session,
            "gw",
            &id,
            text,
            &set("nt4-domain.admin_password", ""),
            &mut no_password,
        )
        .unwrap();
        assert!(
            gw.sealed(&id).unwrap().contains("s3cret"),
            "blank keeps the old value"
        );
        assert_eq!(
            std::fs::read(gw.dir.join("age")).unwrap(),
            raw,
            "and nothing is re-encrypted"
        );
        save_document_with(
            &gw.session,
            "gw",
            &id,
            text,
            &set("nt4-domain.admin_password", "new"),
            &mut no_password,
        )
        .unwrap();
        assert!(gw.sealed(&id).unwrap().contains("new"));
        let doc = load_document_with(&gw.session, &id, &mut no_password).unwrap();
        assert_eq!(
            doc.secret_fields()[0],
            SecretField {
                path: "nt4-domain.admin_password".into(),
                set: true
            }
        );
    }

    #[test]
    fn secrets_in_the_plain_text_and_bad_paths_are_refused_before_anything_is_sent() {
        let (gw, id) = (Gateway::new("refuse"), identity("refuse"));
        let err = save_document_with(
            &gw.session,
            "gw",
            &id,
            "a:\n  password: x\n",
            &[],
            &mut no_password,
        )
        .unwrap_err();
        assert!(err.to_string().contains("a.password"), "{err}");
        let bad = vec![("a.name".to_string(), "x".to_string())];
        assert!(
            save_document_with(&gw.session, "gw", &id, "a: 1\n", &bad, &mut no_password).is_err()
        );
        assert!(
            save_document_with(&gw.session, "gw", &id, "a: [oops", &[], &mut no_password).is_err()
        );
        assert!(gw.plain().is_none() && gw.sealed(&id).is_none());
    }
}
