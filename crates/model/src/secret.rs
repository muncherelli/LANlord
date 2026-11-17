//! a password or other secret on its way to a server.
//!
//! it is wiped from memory when dropped and never shows in debug output. it (de)serializes as a
//! plain string, because it only travels where the connection itself is protected: in the https
//! request body the web ui posts to `/api/op`, and from there over the gateway's local socket or
//! the stdin of `lanlord op`. it is never logged or stored.
//!
//! `lanlord op` runs what it is given as is: purge, power and run ask for no confirmation there,
//! by design, because the front end that sent the op already asked.

use std::fmt;

use zeroize::Zeroizing;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    /// the secret itself, for the one place that hands it to the server.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl serde::Serialize for Secret {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.expose())
    }
}

impl<'de> serde::Deserialize<'de> for Secret {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_shows_the_value() {
        let secret = Secret::new("hunter2");
        let shown = format!("{secret:?} {:?}", vec![secret.clone()]);
        assert!(!shown.contains("hunter2"), "{shown}");
        assert_eq!(secret.expose(), "hunter2");
        assert_eq!(Secret::from(String::from("hunter2")), secret);
    }

    #[test]
    fn it_travels_as_a_plain_string() {
        let secret = Secret::from("pa ss");
        let json = serde_json::to_string(&secret).unwrap();
        assert_eq!(json, "\"pa ss\"");
        assert_eq!(serde_json::from_str::<Secret>(&json).unwrap(), secret);
    }
}
