//! asking the person for a sudo password while a task runs.

use anyhow::Result;

/// asks for the sudo password, showing the prompt; the same shape as the cli's, so the two
/// can be passed to each other.
pub type AskPassword<'a> = &'a mut dyn FnMut(&str) -> Result<String>;

/// remembers the first password it is given, so one task asks once.
pub fn cached<'a>(ask: AskPassword<'a>) -> impl FnMut(&str) -> Result<String> + 'a {
    let mut saved: Option<String> = None;
    move |what| {
        if saved.is_none() {
            saved = Some(ask(what)?);
        }
        Ok(saved.clone().unwrap_or_default())
    }
}

/// for reads that never need sudo: answering a prompt is a bug, not a question for the person.
pub fn never(prompt: &str) -> Result<String> {
    Err(anyhow::anyhow!(
        "did not expect a password prompt: {prompt}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cached_password_is_asked_for_once() {
        let mut asked = 0;
        let mut ask = |_: &str| {
            asked += 1;
            Ok("secret".to_string())
        };
        let mut cached = cached(&mut ask);
        assert_eq!(cached("sudo").unwrap(), "secret");
        assert_eq!(cached("sudo").unwrap(), "secret");
        drop(cached);
        assert_eq!(asked, 1);
    }

    #[test]
    fn a_cancelled_prompt_is_asked_again() {
        let mut tries = 0;
        let mut ask = |_: &str| {
            tries += 1;
            if tries == 1 {
                Err(anyhow::anyhow!("cancelled"))
            } else {
                Ok("pw".to_string())
            }
        };
        let mut cached = cached(&mut ask);
        assert!(cached("sudo").is_err());
        assert_eq!(cached("sudo").unwrap(), "pw");
    }

    #[test]
    fn a_read_never_answers_a_prompt() {
        let err = never("[sudo] password:").unwrap_err().to_string();
        assert!(err.contains("[sudo] password:"), "{err}");
    }
}
