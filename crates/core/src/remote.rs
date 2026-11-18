//! the command line's way to the settings: every read and write goes through the operations the
//! other front ends use, so the key that opens the secrets stays on the gateway. what comes back
//! holds no secret, only which are set.

use anyhow::{bail, Result};

use crate::bootstrap::AskPassword;
use crate::ops::{execute, Op, Reply};
use crate::settings::Document;
use crate::users::{self, cached, User};

/// the settings document on `host`, without its secrets.
pub fn document(host: &str, ask: AskPassword) -> Result<Document> {
    match execute(host, &Op::LoadDocument, ask)? {
        Reply::Document(document) if document == Document::default() => {
            bail!("no settings on {host}; run lanlord config push first")
        }
        Reply::Document(document) => Ok(document),
        _ => bail!("unexpected answer to the settings request"),
    }
}

/// the users the settings define on `host`.
pub fn users_on(host: &str, ask: AskPassword) -> Result<Vec<User>> {
    users::list(document(host, ask)?.plain.as_bytes())
}

/// change logins on `host` and apply the module; returns the users now defined.
pub fn change_users(host: &str, changes: &[users::Change], ask: AskPassword) -> Result<Vec<User>> {
    let mut ask = cached(ask);
    let changes = changes.iter().map(Into::into).collect();
    done(execute(host, &Op::ChangeUsers { changes }, &mut ask)?)?;
    users_on(host, &mut ask)
}

/// edit the plain settings on `host` with `edit`, store them and apply `modules`.
pub fn edit(
    host: &str,
    modules: &[String],
    edit: impl FnOnce(&str) -> Result<String>,
    ask: AskPassword,
) -> Result<()> {
    let mut ask = cached(ask);
    let plain = edit(&document(host, &mut ask)?.plain)?;
    let save = Op::SaveDocument {
        plain,
        updates: Vec::new(),
    };
    done(execute(host, &save, &mut ask)?)?;
    let apply = Op::Apply {
        modules: modules.to_vec(),
    };
    done(execute(host, &apply, &mut ask)?)
}

fn done(reply: Reply) -> Result<()> {
    match reply {
        Reply::Done => Ok(()),
        _ => bail!("unexpected answer"),
    }
}

/// store a whole settings file on `host`: the plain part as the settings, each secret in it set
/// on the gateway, where it is sealed. nothing is read back.
pub fn push_file(host: &str, file: &[u8], ask: AskPassword) -> Result<()> {
    use crate::settings::{dump, parse_value, secret_values, split};
    use lanlord_model::ops::SecretUpdate;
    let (plain, secrets) = split(&parse_value(file)?);
    let updates = secret_values(&secrets)
        .into_iter()
        .map(|(path, value)| SecretUpdate {
            path,
            value: value.into(),
        })
        .collect();
    let save = Op::SaveDocument {
        plain: dump(&plain)?,
        updates,
    };
    done(execute(host, &save, ask)?)
}

/// the settings text on `host` and the paths of the secrets that are set; the secrets
/// themselves are never shown.
pub fn show(host: &str, ask: AskPassword) -> Result<(String, Vec<String>)> {
    let document = document(host, ask)?;
    Ok((document.plain, document.secrets_set))
}
