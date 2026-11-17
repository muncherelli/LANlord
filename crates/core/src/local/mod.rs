//! procedures that run on the gateway itself, in plain rust: facts are read in process, a pure
//! `plan` decides what to do, and the steps act through d-bus, atomic file writes and commands
//! given as an argument list (never through a shell). `lanlord bridge` serves them.

pub mod clock;
