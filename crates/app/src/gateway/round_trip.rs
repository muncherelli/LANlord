//! the sudo password round trip of a front end that cannot answer a prompt while an op runs
//! (the web ui): send the op, and when the server answers [`Reply::NeedsPassword`], ask the
//! person once and send it again with the password. a second request for it means the password
//! was wrong; there is no third try.

use anyhow::{anyhow, Result};
use lanlord_model::ops::{Op, Reply, Response};

/// what a retry with a password that still did not work says.
pub const WRONG_PASSWORD: &str = "wrong password";

/// what a cancelled password dialog says.
pub const CANCELLED: &str = "cancelled";

/// what to do with the first answer to an op.
#[derive(Debug)]
pub enum Step {
    /// the op is finished, for better or worse.
    Done(Result<Reply>),
    /// ask the person for the sudo password with this prompt, then send the op again.
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

/// how long one request may take before it is given up, in milliseconds: the ops that run for
/// minutes get longer than the server's own 15 minute limit, so its answer is the one shown.
pub fn timeout_ms(op: &Op) -> i32 {
    match op {
        Op::Apply { .. } | Op::Power { .. } | Op::ClockApply { .. } => 16 * 60 * 1000,
        _ => 30_000,
    }
}

/// what the server's http answer means: a reply, a message, or the end of the session.
pub fn answer(status: u16, body: &str) -> Result<Reply> {
    if status == 401 && is_session_error(body) {
        return Err(SessionEnded.into());
    }
    if !(200..300).contains(&status) {
        return Err(anyhow!("the server answered {status}: {}", body.trim()));
    }
    match serde_json::from_str::<Response>(body)? {
        Response::Ok(reply) => Ok(reply),
        Response::Error(message) => Err(anyhow!(message)),
    }
}

fn is_session_error(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .is_some_and(|v| v.get("error").and_then(|e| e.as_str()) == Some("session"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_401_session_answer_is_typed_and_other_401s_are_not() {
        let err = answer(401, r#"{"error":"session"}"#).unwrap_err();
        assert!(err.is::<SessionEnded>());
        let other = answer(401, r#"{"error":"password"}"#).unwrap_err();
        assert!(!other.is::<SessionEnded>() && other.to_string().contains("401"));
        // the same words under another status are just an error
        assert!(!answer(500, r#"{"error":"session"}"#)
            .unwrap_err()
            .is::<SessionEnded>());
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
    fn long_ops_wait_longer() {
        let short = timeout_ms(&Op::Leases);
        assert_eq!(short, 30_000);
        for op in [
            Op::Apply { modules: vec![] },
            Op::Power {
                power: lanlord_model::power::Power::Reboot,
            },
        ] {
            assert!(timeout_ms(&op) > 15 * 60 * 1000);
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
}
