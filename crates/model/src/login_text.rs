//! the title and the message of the sign-in screen, kept in the `gateway` section of the plain
//! settings as `login_title` and `login_text`. the web ui serves them before anyone has signed
//! in, so they are public: nothing else from the settings goes with them. they are plain text;
//! an empty one means the front end's own words.

use serde::{Deserialize, Serialize};
use serde_yaml_ng::{Mapping, Value};

use crate::settings::Settings;

/// the settings section and the two keys.
const SECTION: &str = "gateway";
const TITLE_KEY: &str = "login_title";
const TEXT_KEY: &str = "login_text";

/// the longest title and message, in characters.
pub const TITLE_LIMIT: usize = 64;
pub const TEXT_LIMIT: usize = 500;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginText {
    pub title: String,
    pub text: String,
}

impl LoginText {
    /// what the settings hold; a missing or wrong-typed value is empty, and a value past its
    /// limit is cut, so a hand-edited file cannot make the public answer large.
    pub fn from_settings(settings: &Settings) -> Self {
        let field = |key: &str, limit: usize| {
            settings
                .section(SECTION)
                .and_then(|gateway| gateway.get(key))
                .and_then(Value::as_str)
                .map(|value| value.chars().take(limit).collect())
                .unwrap_or_default()
        };
        Self {
            title: field(TITLE_KEY, TITLE_LIMIT),
            text: field(TEXT_KEY, TEXT_LIMIT),
        }
    }

    /// the same read from settings text; text that does not parse holds nothing.
    pub fn from_text(plain: &str) -> Self {
        Settings::parse(plain)
            .map(|settings| Self::from_settings(&settings))
            .unwrap_or_default()
    }

    /// write the two values into `settings`, leaving the rest of the section alone; an empty
    /// value removes its key.
    pub fn store(&self, settings: &mut Settings) {
        let mut gateway = match settings.section(SECTION) {
            Some(Value::Mapping(map)) => map.clone(),
            _ => Mapping::new(),
        };
        for (key, value) in [(TITLE_KEY, &self.title), (TEXT_KEY, &self.text)] {
            if value.is_empty() {
                gateway.remove(key);
            } else {
                gateway.insert(key.into(), Value::String(value.clone()));
            }
        }
        if gateway.is_empty() {
            settings.remove_section(SECTION);
        } else {
            settings.set_section(SECTION, Value::Mapping(gateway));
        }
    }

    /// `plain` with these values in it, or why not.
    pub fn put_in(&self, plain: &str) -> anyhow::Result<String> {
        let mut settings = Settings::parse(plain)?;
        if Self::from_settings(&settings) == *self {
            return Ok(plain.to_string());
        }
        self.store(&mut settings);
        Ok(settings.text())
    }

    /// the first wrong field, like the server's own check.
    pub fn check(&self) -> Result<(), crate::validate::Invalid> {
        use crate::validate::{login_text, login_title, Invalid};
        login_title(&self.title).map_err(|reason| Invalid::new("gateway.login_title", reason))?;
        login_text(&self.text).map_err(|reason| Invalid::new("gateway.login_text", reason))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate::settings_document;

    #[test]
    fn the_values_are_read_from_the_gateway_section() {
        let found = LoginText::from_text(
            "gateway:\n  timezone: UTC\n  login_title: Lab\n  login_text: Hello\n",
        );
        assert_eq!(
            (found.title.as_str(), found.text.as_str()),
            ("Lab", "Hello")
        );
        assert_eq!(LoginText::from_text("dns: {}\n"), LoginText::default());
        assert_eq!(LoginText::from_text(": not yaml ["), LoginText::default());
        assert_eq!(
            LoginText::from_text("gateway:\n  login_title: 5\n"),
            LoginText::default()
        );
    }

    #[test]
    fn a_stored_value_past_its_limit_is_cut() {
        let long = "x".repeat(TEXT_LIMIT + 20);
        let found = LoginText::from_text(&format!(
            "gateway:\n  login_title: {long}\n  login_text: {long}\n"
        ));
        assert_eq!(found.title.chars().count(), TITLE_LIMIT);
        assert_eq!(found.text.chars().count(), TEXT_LIMIT);
    }

    #[test]
    fn writing_keeps_the_rest_and_an_empty_value_removes_its_key() {
        let text = LoginText {
            title: "Lab".into(),
            text: "Line one\nLine two".into(),
        };
        let written = text.put_in("gateway:\n  timezone: UTC\ndns: {}\n").unwrap();
        assert_eq!(LoginText::from_text(&written), text);
        assert!(written.contains("timezone: UTC") && written.contains("dns:"));
        assert!(settings_document(&written).is_ok());
        let cleared = LoginText::default().put_in(&written).unwrap();
        assert!(!cleared.contains("login_") && cleared.contains("timezone"));
        let bare = LoginText::default()
            .put_in(&text.put_in("").unwrap())
            .unwrap();
        assert!(!bare.contains("gateway"));
    }

    #[test]
    fn writing_what_is_already_there_keeps_the_text_as_it_was() {
        let plain = "# note\ngateway:\n  login_title: Lab\n";
        let same = LoginText {
            title: "Lab".into(),
            text: String::new(),
        };
        assert_eq!(same.put_in(plain).unwrap(), plain);
    }

    #[test]
    fn the_limits_are_counted_in_characters() {
        let ok = LoginText {
            title: "é".repeat(TITLE_LIMIT),
            text: "é".repeat(TEXT_LIMIT),
        };
        assert!(ok.check().is_ok());
        let long_title = LoginText {
            title: "x".repeat(TITLE_LIMIT + 1),
            ..LoginText::default()
        };
        assert_eq!(
            long_title.check().unwrap_err().to_string(),
            "gateway.login_title: is too long"
        );
        let long_text = LoginText {
            text: "x".repeat(TEXT_LIMIT + 1),
            ..LoginText::default()
        };
        assert_eq!(
            long_text.check().unwrap_err().to_string(),
            "gateway.login_text: is too long"
        );
        let breaks = LoginText {
            title: "a\nb".into(),
            ..LoginText::default()
        };
        assert!(breaks.check().is_err(), "a title is one line");
        let through_the_document = |key: &str, value: &str| {
            settings_document(&format!("gateway:\n  {key}: \"{value}\"\n"))
        };
        assert!(through_the_document("login_title", &"x".repeat(65)).is_err());
        assert!(through_the_document("login_title", &"x".repeat(64)).is_ok());
        assert!(through_the_document("login_text", &"x".repeat(501)).is_err());
        assert!(through_the_document("login_text", "a\\nb").is_ok());
    }
}
