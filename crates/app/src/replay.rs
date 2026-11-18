//! running a task whose operations cannot be waited for (the browser).
//!
//! a task is plain code that calls the gateway one operation after another and waits for each
//! answer. a page has no thread to wait on, so the task is run in passes instead: every operation
//! it has already had an answer to is answered from the record, and the first one it has not is
//! stopped at and handed back to be sent. once the answer is recorded the task runs again from the
//! start, so each operation is sent exactly once.

use std::sync::{Mutex, PoisonError};

use anyhow::{anyhow, Result};
use lanlord_model::operations::{Operation, Reply};

use crate::ask::AskPassword;
use crate::gateway::round_trip::SessionEnded;
use crate::gateway::Gateway;

/// what a task is told when an operation has no answer yet; the task must stop, and whatever it
/// returns after that is thrown away.
const WAITING: &str = "waiting for the server";

/// an operation's recorded answer; the error keeps what the ui needs to tell (a lost session).
#[derive(Clone)]
enum Recorded {
    Reply(Reply),
    Failed(String),
    SessionEnded,
}

#[derive(Default)]
struct State {
    /// the operations sent so far, with their answers.
    done: Vec<(Operation, Recorded)>,
    /// where the current pass is in `done`.
    cursor: usize,
    /// the operation the current pass stopped at.
    pending: Option<Operation>,
}

/// how one pass ended.
#[derive(Debug)]
pub enum Pass<T> {
    /// the task ran to its end with every answer it needed.
    Finished(Result<T>),
    /// the task needs this operation answered first; record it with [`Replay::record`].
    Needs(Operation),
}

/// the record of one task's operations; hand it to the task as its gateway.
#[derive(Default)]
pub struct Replay {
    state: Mutex<State>,
}

impl Replay {
    /// run `work` once more, from the start.
    pub fn pass<T>(&self, work: impl FnOnce(&dyn Gateway) -> Result<T>) -> Pass<T> {
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.cursor = 0;
            state.pending = None;
        }
        let outcome = work(self);
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        // a task that swallows the error it was given still has no final answer
        match state.pending.take() {
            Some(operation) => Pass::Needs(operation),
            None => Pass::Finished(outcome),
        }
    }

    /// record the answer to the operation the last pass stopped at.
    pub fn record(&self, operation: Operation, answer: Result<Reply>) {
        let recorded = match answer {
            Ok(reply) => Recorded::Reply(reply),
            Err(err) if err.is::<SessionEnded>() => Recorded::SessionEnded,
            Err(err) => Recorded::Failed(err.to_string()),
        };
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.done.push((operation, recorded));
    }
}

impl Gateway for Replay {
    fn call(&self, _host: &str, operation: Operation, _ask: AskPassword) -> Result<Reply> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.pending.is_some() {
            return Err(anyhow!(WAITING));
        }
        let at = state.cursor;
        let Some((sent, recorded)) = state.done.get(at).cloned() else {
            state.pending = Some(operation);
            return Err(anyhow!(WAITING));
        };
        state.cursor += 1;
        if sent != operation {
            return Err(anyhow!("the task changed its mind between two runs"));
        }
        match recorded {
            Recorded::Reply(reply) => Ok(reply),
            Recorded::Failed(message) => Err(anyhow!(message)),
            Recorded::SessionEnded => Err(anyhow!(SessionEnded)),
        }
    }

    fn is_local(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// drive `work` the way the browser does, answering each operation with `answer`; returns the
    /// operations sent, in order, and how many passes it took.
    fn drive<T>(
        work: impl Fn(&dyn Gateway) -> Result<T>,
        answer: impl Fn(&Operation) -> Result<Reply>,
    ) -> (Result<T>, Vec<Operation>, usize) {
        let replay = Replay::default();
        let (mut sent, mut passes) = (Vec::new(), 0);
        loop {
            passes += 1;
            match replay.pass(&work) {
                Pass::Finished(result) => return (result, sent, passes),
                Pass::Needs(operation) => {
                    replay.record(operation.clone(), answer(&operation));
                    sent.push(operation);
                }
            }
        }
    }

    fn call(gateway: &dyn Gateway, operation: Operation) -> Result<Reply> {
        gateway.call("gw", operation, &mut crate::ask::never)
    }

    #[test]
    fn a_chain_of_operations_sends_each_one_once() {
        let (result, sent, passes) = drive(
            |gw| {
                gw.load_document("gw", &mut crate::ask::never)?;
                gw.save_document("gw", "a: 1", &[], &mut crate::ask::never)?;
                gw.apply("gw", &["dns".into()], &mut crate::ask::never)
            },
            |operation| {
                Ok(match operation {
                    Operation::LoadDocument => Reply::Document(Default::default()),
                    _ => Reply::Done,
                })
            },
        );
        result.unwrap();
        assert_eq!(passes, 4);
        assert_eq!(sent.len(), 3);
        assert!(matches!(sent[0], Operation::LoadDocument));
        assert!(matches!(sent[1], Operation::SaveDocument { .. }));
        assert!(matches!(sent[2], Operation::Apply { .. }));
    }

    #[test]
    fn a_task_that_swallows_the_wait_still_waits() {
        let runs = AtomicUsize::new(0);
        let (result, sent, _) = drive(
            |gw| {
                runs.fetch_add(1, Ordering::SeqCst);
                // an error is turned into a value, as some windows do
                let names = call(gw, Operation::Leases).ok();
                Ok(names.is_some())
            },
            |_| Ok(Reply::Leases(Vec::new())),
        );
        assert!(result.unwrap(), "the final value comes from a real answer");
        assert_eq!(sent.len(), 1);
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn operations_after_the_first_wait_are_not_sent_ahead() {
        let replay = Replay::default();
        let pass = replay.pass(|gw| {
            let _ = call(gw, Operation::Leases);
            let _ = call(gw, Operation::ClockState);
            Ok(())
        });
        assert!(matches!(pass, Pass::Needs(Operation::Leases)));
    }

    #[test]
    fn a_server_error_ends_the_task_with_its_message() {
        let (result, sent, _) = drive(
            |gw| {
                call(gw, Operation::Leases)?;
                call(gw, Operation::ClockState)
            },
            |_| Err(anyhow!("no key")),
        );
        assert_eq!(result.unwrap_err().to_string(), "no key");
        assert_eq!(sent.len(), 1);
    }

    #[test]
    fn a_lost_session_stays_typed() {
        let (result, _, _) = drive(
            |gw| call(gw, Operation::Leases),
            |_| Err(anyhow!(SessionEnded)),
        );
        assert!(result.unwrap_err().is::<SessionEnded>());
    }

    #[test]
    fn a_task_that_changes_its_operations_is_stopped() {
        let replay = Replay::default();
        assert!(matches!(
            replay.pass(|gw| call(gw, Operation::Leases)),
            Pass::Needs(_)
        ));
        replay.record(Operation::Leases, Ok(Reply::Done));
        let Pass::Finished(result) = replay.pass(|gw| call(gw, Operation::ClockState)) else {
            panic!("finished")
        };
        assert!(result.is_err());
    }
}
