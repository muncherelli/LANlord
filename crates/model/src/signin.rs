//! signing in to a gateway with your own ssh credentials. nothing is stored: an agent or key
//! signs you in silently; when the server offers passwords the caller asks for one, and it
//! reaches ssh through its askpass hook for that one attempt only.

use std::fmt;

/// who the gateway says you are once signed in.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Identity {
    pub user: String,
    pub uid: u32,
}

impl Identity {
    pub fn is_root(&self) -> bool {
        self.uid == 0
    }
}

/// why signing in did not work, in terms the logon box can show.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SigninError {
    NotInstalled,
    AgentLocked,
    /// keys did not work, but the server accepts a password.
    NeedsPassword,
    /// the server takes none of the offered methods but these (for example only keys).
    KeysOnly(Vec<String>),
    WrongPassword,
    /// no trusted host key is known for the server yet.
    HostKeyUnknown,
    HostKeyChanged,
    Timeout,
    Refused,
    Unreachable(String),
    Other(String),
}

impl fmt::Display for SigninError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled => write!(f, "The ssh client was not found. Install OpenSSH."),
            Self::AgentLocked => write!(
                f,
                "The ssh agent is not responding. Unlock it (for example 1Password) and try again."
            ),
            Self::NeedsPassword => write!(f, "The server asks for a password."),
            Self::KeysOnly(methods) => write!(
                f,
                "This server only accepts keys ({}), and none of yours were accepted. \
                 Add your key to the agent or ask an administrator to authorise it.",
                methods.join(", ")
            ),
            Self::WrongPassword => write!(f, "The username or password is incorrect."),
            Self::HostKeyUnknown => write!(f, "The server's identity is not known yet."),
            Self::HostKeyChanged => write!(
                f,
                "The server's identity has changed since you last connected. \
                 Someone may be intercepting the connection, or the server was reinstalled. \
                 Remove its line from known_hosts if you expected this."
            ),
            Self::Timeout => write!(f, "The server did not answer in time."),
            Self::Refused => write!(
                f,
                "The server refused the connection. Is ssh running there?"
            ),
            Self::Unreachable(detail) => write!(f, "{detail}"),
            Self::Other(detail) => write!(f, "The logon failed: {detail}"),
        }
    }
}

impl std::error::Error for SigninError {}
