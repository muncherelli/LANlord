//! one place for everything lanlord prints. the cli prints lines itself; the
//! tui and gui subscribe and show them in their console.

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Kind {
    /// stdout of a remote command.
    Out,
    /// stderr of a remote command.
    Err,
    /// progress worth showing every time.
    Info,
    /// what lanlord is doing, shown with --verbose or in a console.
    Debug,
    /// not for reading: the text is one `apply::ApplyEvent` as json, for the progress dialog.
    Event,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Line {
    pub kind: Kind,
    pub text: String,
}
