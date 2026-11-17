//! the sudo password round trip of a front end that cannot answer a prompt while an op runs
//! (the web ui): send the op, and when the server answers [`Reply::NeedsPassword`], ask the
//! person once and send it again with the password. a second request for it means the password
//! was wrong; there is no third try.

use anyhow::{anyhow, Result};
use lanlord_model::ops::Reply;

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

#[cfg(test)]
mod tests {
    use super::*;

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
