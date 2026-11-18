//! the protocol between a controller and `lanlord bridge` on a gateway: one json value per line
//! over the stdio of an ssh session. the bridge speaks first with a [`Hello`]; the controller then
//! sends [`Request`]s and reads [`Event`]s. nothing but protocol goes to the bridge's stdout.

use serde::{Deserialize, Serialize};

use crate::operations::{Operation, Reply};

/// bumped whenever a frame or an operation changes in a way an older peer cannot read.
pub const PROTO: u32 = 1;

/// the first frame the bridge sends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub proto: u32,
    pub version: String,
}

impl Hello {
    /// this build's greeting.
    pub fn current(version: &str) -> Self {
        Self {
            proto: PROTO,
            version: version.to_string(),
        }
    }

    /// the reason the controller cannot talk to this bridge, if it cannot.
    pub fn mismatch(&self) -> Option<String> {
        (self.proto != PROTO).then(|| {
            format!(
                "the gateway runs lanlord {} (protocol {}), this lanlord speaks protocol {PROTO}; \
                 update the gateway with `lanlord update`",
                self.version, self.proto
            )
        })
    }
}

/// one operation for the bridge to run; the id comes back on every event about it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(rename = "op")]
    pub operation: Operation,
}

/// what the bridge says about a request. a request ends with a `Reply` or an `Error`; `Log` lines
/// come before that, and a subscription (later) keeps sending `Sample`s until it is cancelled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// a line of progress, shown as it happens.
    Log { id: u64, line: String },
    /// the answer; the request is done.
    Reply { id: u64, reply: Reply },
    /// the request failed; the request is done.
    Error { id: u64, message: String },
    /// one reading of a subscribed topic. reserved: nothing sends it yet.
    Sample {
        id: u64,
        topic: String,
        value: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_one_line_of_json() {
        let hello = serde_json::to_string(&Hello::current("1.2.3")).unwrap();
        assert_eq!(hello, r#"{"proto":1,"version":"1.2.3"}"#);
        let request = Request {
            id: 7,
            operation: Operation::ClockZoneNow { zone: "UTC".into() },
        };
        let text = serde_json::to_string(&request).unwrap();
        assert!(!text.contains('\n'));
        assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), request);
        for event in [
            Event::Log {
                id: 7,
                line: "x".into(),
            },
            Event::Reply {
                id: 7,
                reply: Reply::Done,
            },
            Event::Error {
                id: 7,
                message: "no".into(),
            },
            Event::Sample {
                id: 7,
                topic: "clock".into(),
                value: "1".into(),
            },
        ] {
            let text = serde_json::to_string(&event).unwrap();
            assert_eq!(serde_json::from_str::<Event>(&text).unwrap(), event);
        }
    }

    #[test]
    fn another_protocol_suggests_an_update() {
        assert_eq!(Hello::current("1.2.3").mismatch(), None);
        let old = Hello {
            proto: PROTO + 1,
            version: "9.0.0".into(),
        };
        let why = old.mismatch().unwrap();
        assert!(
            why.contains("lanlord update") && why.contains("9.0.0"),
            "{why}"
        );
    }

    #[test]
    fn only_clock_operations_run_on_the_bridge_and_only_writes_need_root() {
        assert!(Operation::ClockState.runs_on_bridge() && !Operation::ClockState.needs_root());
        assert!(Operation::ClockUpdateNow.needs_root());
        assert!(!Operation::Doctor.runs_on_bridge());
    }
}
