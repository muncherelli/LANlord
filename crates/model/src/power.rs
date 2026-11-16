//! reboot or power off the gateway.

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Power {
    Reboot,
    PowerOff,
}

impl Power {
    #[doc(hidden)]
    pub fn script(self) -> &'static str {
        match self {
            Self::Reboot => "systemctl reboot\n",
            Self::PowerOff => "systemctl poweroff\n",
        }
    }

    /// what is happening, for messages.
    pub fn doing(self) -> &'static str {
        match self {
            Self::Reboot => "rebooting",
            Self::PowerOff => "shutting down",
        }
    }
}

/// what happened to the command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Outcome {
    /// the command returned normally.
    Done,
    /// the connection dropped while it ran, which is what a machine going down looks like.
    Dropped,
}
