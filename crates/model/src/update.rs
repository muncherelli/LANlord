//! what an update is and where the updater stands; the work of finding, fetching and installing
//! one lives in `lanlord_core::update` and `lanlord_core::updater`.

/// where a newer version would come from, and so how to install it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// new commits on trunk in the checkout this build was made from.
    Git,
    /// a new release; the page to open.
    Release { url: String },
}

/// what is new, for the notice and its box.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    pub source: Source,
    pub headline: String,
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pause {
    /// the user paused it, and it stays paused until they resume.
    Manual,
    /// the network is gone; it tries again by itself, `attempt` times so far.
    Offline { attempt: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// nothing to install.
    Idle,
    /// an update is there and nothing was started.
    Available,
    Downloading,
    Installing,
    Paused(Pause),
    Failed(String),
    /// installed; the new build runs after a restart.
    Done,
}
