//! the sudo password round trip of a front end that cannot answer a prompt while an operation runs
//! (the web ui): send the operation, and when the server answers [`Reply::NeedsPassword`], ask the
//! person once and send it again with the password. a second request for it means the password
//! was wrong; there is no third try.

use anyhow::{anyhow, Result};
use lanlord_model::login_text::LoginText;
use lanlord_model::operations::{Operation, Reply, Response};
use lanlord_model::secret::Secret;

/// what a retry with a password that still did not work says.
pub const WRONG_PASSWORD: &str = "wrong password";

/// what a cancelled password dialog says.
pub const CANCELLED: &str = "cancelled";

/// what to do with the first answer to an operation.
#[derive(Debug)]
pub enum Step {
    /// the operation is finished, for better or worse.
    Done(Result<Reply>),
    /// ask the person for the sudo password with this prompt, then send the operation again.
    Ask(String),
}

/// the first answer: either the end, or a prompt to show.
pub fn first(answer: Result<Reply>) -> Step {
    match answer {
        Ok(Reply::NeedsPassword { prompt }) => Step::Ask(prompt),
        other => Step::Done(other),
    }
}

/// the answer to the retry that carried the password: asking again means it was wrong.
pub fn retried(answer: Result<Reply>) -> Result<Reply> {
    match answer {
        Ok(Reply::NeedsPassword { .. }) => Err(anyhow!(WRONG_PASSWORD)),
        other => other,
    }
}

/// the server no longer knows this login: sign in again. the page keeps a shared flag so the
/// desktop can show its logon box.
#[derive(Debug, PartialEq, Eq)]
pub struct SessionEnded;

impl std::fmt::Display for SessionEnded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("your session has ended; sign in again")
    }
}

impl std::error::Error for SessionEnded {}

/// how long one request may take before it is given up, in milliseconds: the operations that run
/// for minutes get longer than the server's own 15 minute limit, so its answer is the one shown.
pub fn timeout_ms(operation: &Operation) -> i32 {
    match operation {
        Operation::Apply { .. }
        | Operation::Power { .. }
        | Operation::ClockApply { .. }
        | Operation::ChangeNetworks { .. }
        | Operation::Purge { .. }
        | Operation::Run { .. }
        | Operation::Bootstrap => 16 * 60 * 1000,
        _ => 30_000,
    }
}

/// what the server's http answer means: a reply, a message, or the end of the session.
pub fn answer(status: u16, body: &str) -> Result<Reply> {
    if status == 401 && is_session_error(body) {
        return Err(SessionEnded.into());
    }
    if !(200..300).contains(&status) {
        return Err(match server_message(body) {
            Some(message) => anyhow!(message),
            None => anyhow!("the server answered {status}: {}", body.trim()),
        });
    }
    match serde_json::from_str::<Response>(body)? {
        Response::Ok(reply) => Ok(reply),
        Response::Error(message) | Response::Denied(message) => Err(anyhow!(message)),
    }
}

/// the `error` text of a json error body, so the person reads it instead of raw json.
fn server_message(body: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(body).ok()?;
    let message = value.get("error")?.as_str()?.trim();
    (!message.is_empty()).then(|| message.to_string())
}

fn is_session_error(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .is_some_and(|v| v.get("error").and_then(|e| e.as_str()) == Some("session"))
}

/// the answer to the retry that carried `given`, and the password to keep for the task's later
/// operations: only one the server did not ask for again, so a wrong one is never sent twice.
pub fn after_retry(given: Secret, again: Result<Reply>) -> (Option<Secret>, Result<Reply>) {
    let keep = !matches!(again, Ok(Reply::NeedsPassword { .. }));
    (keep.then_some(given), retried(again))
}

/// the sign-in text a `/api/login-text` answer holds; anything but a good answer is none, and the
/// logon box keeps its own words.
pub fn login_text_body(status: u16, body: &str) -> Option<LoginText> {
    (status == 200).then(|| serde_json::from_str(body).ok())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wrong_password_is_not_kept_for_later_operations() {
        let asked = || Ok(Reply::NeedsPassword { prompt: "x".into() });
        let (kept, answer) = after_retry(Secret::from("bad"), asked());
        assert!(kept.is_none());
        assert_eq!(answer.unwrap_err().to_string(), WRONG_PASSWORD);
        // the next operation has no password, so it asks again
        assert!(matches!(first(asked()), Step::Ask(_)));
        let (kept, answer) = after_retry(Secret::from("good"), Ok(Reply::Done));
        assert_eq!(kept, Some(Secret::from("good")));
        assert!(answer.is_ok());
        let (kept, _) = after_retry(Secret::from("pw"), Err(anyhow!("down")));
        assert!(kept.is_some());
    }

    #[test]
    fn a_401_session_answer_is_typed_and_other_401s_are_not() {
        let err = answer(401, r#"{"error":"session"}"#).unwrap_err();
        assert!(err.is::<SessionEnded>());
        let other = answer(401, r#"{"error":"password"}"#).unwrap_err();
        assert!(!other.is::<SessionEnded>() && other.to_string() == "password");
        // the same words under another status are just an error
        assert!(!answer(500, r#"{"error":"session"}"#)
            .unwrap_err()
            .is::<SessionEnded>());
    }

    #[test]
    fn a_refusal_shows_the_servers_message() {
        let denied = r#"{"error":"permission denied: this needs Manage on Users"}"#;
        assert_eq!(
            answer(403, denied).unwrap_err().to_string(),
            "permission denied: this needs Manage on Users"
        );
        // a body that is not an error message still shows what came back
        let other = answer(502, "bad gateway").unwrap_err().to_string();
        assert!(other.contains("502") && other.contains("bad gateway"));
    }

    #[test]
    fn replies_and_errors_come_out_of_the_body() {
        assert!(matches!(
            answer(200, r#"{"ok":{"reply":"done"}}"#),
            Ok(Reply::Done)
        ));
        assert_eq!(
            answer(200, r#"{"error":"no key"}"#)
                .unwrap_err()
                .to_string(),
            "no key"
        );
        assert!(answer(200, "not json").is_err());
    }

    #[test]
    fn long_operations_wait_longer() {
        let short = timeout_ms(&Operation::Leases);
        assert_eq!(short, 30_000);
        for operation in [
            Operation::Apply { modules: vec![] },
            Operation::Power {
                power: lanlord_model::power::Power::Reboot,
            },
            Operation::Purge {
                remove_admins: false,
            },
            Operation::Run {
                command: vec!["true".into()],
            },
            Operation::Bootstrap,
            Operation::ChangeNetworks {
                edit: lanlord_model::lan_edit::Edit::Remove("lab".into()),
            },
        ] {
            assert!(timeout_ms(&operation) > 15 * 60 * 1000);
        }
    }

    #[test]
    fn a_prompt_is_shown_once_and_a_second_one_is_a_wrong_password() {
        let asked = Ok(Reply::NeedsPassword {
            prompt: "[sudo] password for admin:".into(),
        });
        assert!(matches!(first(asked), Step::Ask(p) if p.contains("sudo")));
        let again = Ok(Reply::NeedsPassword { prompt: "x".into() });
        assert_eq!(retried(again).unwrap_err().to_string(), WRONG_PASSWORD);
        assert!(matches!(retried(Ok(Reply::Done)), Ok(Reply::Done)));
    }

    #[test]
    fn an_answer_without_a_prompt_is_the_end() {
        assert!(matches!(
            first(Ok(Reply::Done)),
            Step::Done(Ok(Reply::Done))
        ));
        assert!(matches!(first(Err(anyhow!("down"))), Step::Done(Err(_))));
        assert!(retried(Err(anyhow!("down"))).is_err());
    }

    #[test]
    fn the_sign_in_text_is_read_from_a_good_answer_only() {
        let body = r#"{"title":"Lab","text":"Hello"}"#;
        assert_eq!(
            login_text_body(200, body),
            Some(LoginText {
                title: "Lab".into(),
                text: "Hello".into()
            })
        );
        assert_eq!(login_text_body(503, body), None);
        assert_eq!(login_text_body(200, "not json"), None);
        assert_eq!(login_text_body(200, r#"{"title":"Lab"}"#), None);
    }
}
