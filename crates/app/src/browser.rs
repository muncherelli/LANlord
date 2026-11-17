//! running a task in the browser: its ops are sent over http one at a time, and the task is
//! run again after each answer (see `replay`), so the windows that spawned it need not know.

use std::sync::mpsc::{self, Sender, TryRecvError};

use anyhow::{anyhow, Result};
use lanlord_model::secret::Secret;

use crate::ask::AskPassword;
use crate::gateway::http::Http;
use crate::gateway::round_trip::{self, Step, CANCELLED};
use crate::gateway::Gateway;
use crate::replay::{Pass, Replay};
use crate::tasks::{PasswordRequest, PROMPT_IN_BROWSER};
use crate::wake::Wake;

/// how often the dialog's answer is looked for, in milliseconds.
const POLL_MS: i32 = 50;

/// run `work` to its end. a sudo prompt goes to the ui through `requests` once per task and the
/// password is sent with every later op; without `requests` (a quiet read) it is an error.
pub async fn run<T>(
    http: &Http,
    requests: Option<&Sender<PasswordRequest>>,
    wake: &Wake,
    work: impl Fn(&dyn Gateway, AskPassword) -> Result<T>,
) -> Result<T> {
    let replay = Replay::default();
    let mut password: Option<Secret> = None;
    loop {
        let pass = replay.pass(|gateway| {
            let mut ask = |what: &str| Err(anyhow!("{PROMPT_IN_BROWSER} ({what})"));
            work(gateway, &mut ask)
        });
        let op = match pass {
            Pass::Finished(result) => return result,
            Pass::Needs(op) => op,
        };
        let first = http.exchange(op.clone(), password.clone()).await;
        let answer = match round_trip::first(first) {
            Step::Done(result) => result,
            Step::Ask(prompt) => match (&password, requests) {
                (Some(_), _) => Err(anyhow!(round_trip::WRONG_PASSWORD)),
                (None, None) => Err(anyhow!("the server wants a password: {prompt}")),
                (None, Some(requests)) => match ask(requests, wake, prompt).await {
                    None => Err(anyhow!(CANCELLED)),
                    Some(given) => {
                        let again = http.exchange(op.clone(), Some(given.clone())).await;
                        password = Some(given);
                        round_trip::retried(again)
                    }
                },
            },
        };
        replay.record(op, answer);
    }
}

/// show the password dialog and wait for its answer without blocking the page.
async fn ask(requests: &Sender<PasswordRequest>, wake: &Wake, prompt: String) -> Option<Secret> {
    let (reply, answer) = mpsc::channel();
    requests.send(PasswordRequest { prompt, reply }).ok()?;
    wake.wake();
    loop {
        match answer.try_recv() {
            // the plain copy lives only until the secret owns it
            Ok(given) => return given.map(|password| Secret::from(password.to_string())),
            Err(TryRecvError::Empty) => sleep(POLL_MS).await,
            Err(TryRecvError::Disconnected) => return None,
        }
    }
}

async fn sleep(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}
