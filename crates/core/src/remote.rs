//! the command line's way to the settings: every read and write goes through the operations the
//! other front ends use, so the key that opens the secrets stays on the gateway. what comes back
//! holds no secret, only which are set.

use anyhow::{bail, Result};

use crate::bootstrap::AskPassword;
use crate::operations::{execute, Operation, Reply};
use crate::settings::Document;
use crate::users::{self, cached, User};

/// one operation on the gateway and what it answers: `execute` towards a host, or a stand-in.
type Run<'a> = &'a mut dyn FnMut(&Operation) -> Result<Reply>;

/// the settings document on `host`, without its secrets.
pub fn document(host: &str, ask: AskPassword) -> Result<Document> {
    document_with(host, &mut |operation| execute(host, operation, ask))
}

fn document_with(host: &str, run: Run) -> Result<Document> {
    match run(&Operation::LoadDocument)? {
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
    change_users_with(host, changes, &mut |operation| {
        execute(host, operation, &mut ask)
    })
}

fn change_users_with(host: &str, changes: &[users::Change], run: Run) -> Result<Vec<User>> {
    let changes = changes.iter().map(Into::into).collect();
    done(run(&Operation::ChangeUsers { changes })?)?;
    users::list(document_with(host, run)?.plain.as_bytes())
}

/// edit the plain settings on `host` with `edit`, store them and apply `modules`.
pub fn edit(
    host: &str,
    modules: &[String],
    edit: impl FnOnce(&str) -> Result<String>,
    ask: AskPassword,
) -> Result<()> {
    let mut ask = cached(ask);
    edit_with(host, modules, edit, &mut |operation| {
        execute(host, operation, &mut ask)
    })
}

fn edit_with(
    host: &str,
    modules: &[String],
    edit: impl FnOnce(&str) -> Result<String>,
    run: Run,
) -> Result<()> {
    let plain = edit(&document_with(host, run)?.plain)?;
    let save = Operation::SaveDocument {
        plain,
        updates: Vec::new(),
    };
    done(run(&save)?)?;
    let apply = Operation::Apply {
        modules: modules.to_vec(),
    };
    done(run(&apply)?)
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
    push_file_with(file, &mut |operation| execute(host, operation, ask))
}

fn push_file_with(file: &[u8], run: Run) -> Result<()> {
    use crate::settings::{dump, parse_value, secret_values, split};
    use lanlord_model::operations::SecretUpdate;
    let (plain, secrets) = split(&parse_value(file)?);
    let updates = secret_values(&secrets)
        .into_iter()
        .map(|(path, value)| SecretUpdate {
            path,
            value: value.into(),
        })
        .collect();
    let save = Operation::SaveDocument {
        plain: dump(&plain)?,
        updates,
    };
    done(run(&save)?)
}

/// the settings text on `host` and the paths of the secrets that are set; the secrets
/// themselves are never shown.
pub fn show(host: &str, ask: AskPassword) -> Result<(String, Vec<String>)> {
    let document = document(host, ask)?;
    Ok((document.plain, document.secrets_set))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn document_of(plain: &str, secrets_set: &[&str]) -> Document {
        Document {
            plain: plain.into(),
            secrets_set: secrets_set.iter().map(|s| s.to_string()).collect(),
            ..Document::default()
        }
    }

    /// a gateway that keeps what it was sent, and answers with `document` to every read.
    fn gateway(
        document: Document,
        sent: &RefCell<Vec<Operation>>,
    ) -> impl FnMut(&Operation) -> Result<Reply> + '_ {
        move |operation| {
            sent.borrow_mut().push(operation.clone());
            Ok(match operation {
                Operation::LoadDocument => Reply::Document(document.clone()),
                _ => Reply::Done,
            })
        }
    }

    #[test]
    fn an_empty_document_says_to_push_the_settings_first() {
        let sent = RefCell::new(Vec::new());
        let mut run = gateway(Document::default(), &sent);
        let error = document_with("srv", &mut run).unwrap_err().to_string();
        assert!(error.contains("config push"), "{error}");
    }

    #[test]
    fn a_document_comes_back_and_any_other_answer_is_refused() {
        let sent = RefCell::new(Vec::new());
        let mut run = gateway(document_of("a: 1\n", &["x.y"]), &sent);
        assert_eq!(document_with("srv", &mut run).unwrap().plain, "a: 1\n");
        assert!(document_with("srv", &mut |_| Ok(Reply::Done)).is_err());
        assert!(done(Reply::Done).is_ok());
        assert!(done(Reply::Document(Document::default())).is_err());
    }

    #[test]
    fn an_edit_reads_stores_and_applies_in_that_order() {
        let sent = RefCell::new(Vec::new());
        let mut run = gateway(document_of("a: 1\n", &[]), &sent);
        edit_with(
            "srv",
            &["samba".into()],
            |plain| Ok(format!("{plain}b: 2\n")),
            &mut run,
        )
        .unwrap();
        assert!(matches!(sent.borrow()[0], Operation::LoadDocument));
        assert!(
            matches!(&sent.borrow()[1], Operation::SaveDocument { plain, updates }
            if plain == "a: 1\nb: 2\n" && updates.is_empty())
        );
        assert!(matches!(&sent.borrow()[2], Operation::Apply { modules } if modules == &["samba"]));
    }

    #[test]
    fn a_failed_edit_sends_nothing_after_the_read() {
        let sent = RefCell::new(Vec::new());
        let mut run = gateway(document_of("a: 1\n", &[]), &sent);
        let failed = edit_with("srv", &[], |_| bail!("no"), &mut run);
        assert!(failed.is_err());
        assert_eq!(sent.borrow().len(), 1);
    }

    #[test]
    fn a_pushed_file_sends_its_secrets_as_updates_and_reads_nothing_back() {
        let sent = RefCell::new(Vec::new());
        let mut run = gateway(Document::default(), &sent);
        push_file_with(b"name: lan\n", &mut run).unwrap();
        assert!(
            matches!(&sent.borrow()[..], [Operation::SaveDocument { plain, .. }] if plain.contains("lan"))
        );
        assert!(push_file_with(b": : not yaml [", &mut |_| Ok(Reply::Done)).is_err());
    }

    #[test]
    fn changing_users_applies_the_change_then_lists_the_users() {
        let sent = RefCell::new(Vec::new());
        let mut run = gateway(document_of("nt4-domain:\n  users: []\n", &[]), &sent);
        let users = change_users_with("srv", &[], &mut run).unwrap();
        assert!(users.is_empty());
        assert!(matches!(sent.borrow()[0], Operation::ChangeUsers { .. }));
        assert!(matches!(sent.borrow()[1], Operation::LoadDocument));
    }
}
