//! the browser's backend: every op goes to the web ui's `/api/op` as json.
//!
//! a browser cannot wait for an answer, so [`Http::run`] hands the reply to a callback. the
//! synchronous [`Gateway::call`] the desktop uses only says so.

use anyhow::{anyhow, Result};
use lanlord_model::ops::{Op, Reply, Request, Response};
use lanlord_model::secret::Secret;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use zeroize::Zeroizing;

use super::round_trip::{self, Step, CANCELLED};
use super::Gateway;
use crate::ask::AskPassword;

/// how long one request may take before it is given up, in milliseconds.
pub const TIMEOUT_MS: i32 = 30_000;

/// shows the sudo prompt and calls back with the password, or `None` when cancelled.
pub type AskOnce = Box<dyn FnOnce(String, Box<dyn FnOnce(Option<Secret>)>)>;

/// the gateway this page was served from.
#[derive(Clone)]
pub struct Http {
    /// where ops are posted, `/api/op` on the page's own server.
    pub endpoint: String,
}

impl Default for Http {
    fn default() -> Self {
        Self {
            endpoint: "/api/op".into(),
        }
    }
}

impl Http {
    /// send `op` and call `done` with the reply. when the server needs the sudo password, `ask`
    /// shows the prompt once and the op is sent again with it; a second request for it is
    /// reported as a wrong password.
    pub fn run(&self, op: Op, ask: AskOnce, done: impl FnOnce(Result<Reply>) + 'static) {
        let http = self.clone();
        let retry = op.clone();
        self.send(op, None, move |answer| match round_trip::first(answer) {
            Step::Done(result) => done(result),
            Step::Ask(prompt) => ask(
                prompt,
                Box::new(move |password| match password {
                    None => done(Err(anyhow!(CANCELLED))),
                    Some(password) => http.send(retry, Some(password), move |answer| {
                        done(round_trip::retried(answer))
                    }),
                }),
            ),
        });
    }

    /// post one request; the password is wiped once it has been handed to the browser.
    fn send(&self, op: Op, password: Option<Secret>, done: impl FnOnce(Result<Reply>) + 'static) {
        let endpoint = self.endpoint.clone();
        let request = Request { op, password };
        wasm_bindgen_futures::spawn_local(async move {
            let answer = post(&endpoint, request).await;
            done(answer);
        });
    }
}

impl Gateway for Http {
    fn call(&self, _host: &str, _op: Op, _ask: AskPassword) -> Result<Reply> {
        Err(anyhow!(
            "the browser cannot wait for the server; the web ui sends ops with Http::run"
        ))
    }

    fn is_local(&self) -> bool {
        // the web ui only runs on the gateway itself
        true
    }
}

async fn post(endpoint: &str, request: Request) -> Result<Reply> {
    // the body holds the password in plain text: keep it only until the browser has its copy
    let body = Zeroizing::new(serde_json::to_string(&request)?);
    drop(request);
    let window = web_sys::window().ok_or_else(|| anyhow!("no window"))?;
    let controller = web_sys::AbortController::new().map_err(js)?;
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_body(&wasm_bindgen::JsValue::from_str(&body));
    init.set_signal(Some(&controller.signal()));
    drop(body);
    let web_request = web_sys::Request::new_with_str_and_init(endpoint, &init).map_err(js)?;
    web_request
        .headers()
        .set("content-type", "application/json")
        .map_err(js)?;
    let abort = Closure::once(move || controller.abort());
    let timer = window
        .set_timeout_with_callback_and_timeout_and_arguments_0(
            abort.as_ref().unchecked_ref(),
            TIMEOUT_MS,
        )
        .map_err(js)?;
    let timed_out = || anyhow!("no answer from the server within {} s", TIMEOUT_MS / 1000);
    // the abort timer stays armed until the body is read, so a stalled body also times out
    let fetched = async {
        let answer: web_sys::Response =
            wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(&web_request))
                .await
                .map_err(|_| timed_out())?
                .dyn_into()
                .map_err(js)?;
        let status = answer.status();
        let text = wasm_bindgen_futures::JsFuture::from(answer.text().map_err(js)?)
            .await
            .map_err(|_| timed_out())?
            .as_string()
            .unwrap_or_default();
        Ok::<_, anyhow::Error>((status, text))
    }
    .await;
    window.clear_timeout_with_handle(timer);
    drop(abort);
    let (status, text) = fetched?;
    if !(200..300).contains(&status) {
        return Err(anyhow!("the server answered {status}: {}", text.trim()));
    }
    match serde_json::from_str::<Response>(&text)? {
        Response::Ok(reply) => Ok(reply),
        Response::Error(message) => Err(anyhow!(message)),
    }
}

fn js(value: wasm_bindgen::JsValue) -> anyhow::Error {
    anyhow!("{value:?}")
}
