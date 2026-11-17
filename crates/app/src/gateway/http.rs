//! the browser's backend: every op goes to the web ui's `/api/op` as json.
//!
//! a browser cannot wait for an answer, so ops are awaited with [`Http::exchange`]; tasks run
//! on it in passes (see `replay`). the synchronous [`Gateway::call`] the desktop uses only says so.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use lanlord_model::ops::{Op, Reply, Request};
use lanlord_model::secret::Secret;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use zeroize::Zeroizing;

use super::round_trip::{self, SessionEnded};
use super::Gateway;
use crate::ask::AskPassword;

/// the gateway this page was served from.
#[derive(Clone)]
pub struct Http {
    /// where ops are posted, `/api/op` on the page's own server.
    pub endpoint: String,
    /// set once the server answered that the session is gone; shared by every clone.
    ended: Arc<AtomicBool>,
}

impl Default for Http {
    fn default() -> Self {
        Self {
            endpoint: "/api/op".into(),
            ended: Arc::default(),
        }
    }
}

impl Http {
    /// post one op, with the sudo password once the server has asked for it. an answer that
    /// says the session is over also raises the flag [`Gateway::session_ended`] reads.
    pub async fn exchange(&self, op: Op, password: Option<Secret>) -> Result<Reply> {
        let answer = post(&self.endpoint, Request { op, password }).await;
        if matches!(&answer, Err(err) if err.is::<SessionEnded>()) {
            self.ended.store(true, Ordering::SeqCst);
        }
        answer
    }
}

impl Gateway for Http {
    fn call(&self, _host: &str, _op: Op, _ask: AskPassword) -> Result<Reply> {
        Err(anyhow!(
            "the browser cannot wait for the server; tasks send ops with Http::exchange"
        ))
    }

    fn is_local(&self) -> bool {
        // the web ui only runs on the gateway itself
        true
    }

    fn http(&self) -> Option<&Http> {
        Some(self)
    }

    fn session_ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }

    fn session_restored(&self) {
        self.ended.store(false, Ordering::SeqCst);
    }
}

async fn post(endpoint: &str, request: Request) -> Result<Reply> {
    // the body holds the password in plain text: keep it only until the browser has its copy
    let op = request.op.clone();
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
    let timeout = round_trip::timeout_ms(&op);
    let abort = Closure::once(move || controller.abort());
    let timer = window
        .set_timeout_with_callback_and_timeout_and_arguments_0(
            abort.as_ref().unchecked_ref(),
            timeout,
        )
        .map_err(js)?;
    let timed_out = || anyhow!("no answer from the server within {} s", timeout / 1000);
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
    round_trip::answer(status, &text)
}

fn js(value: wasm_bindgen::JsValue) -> anyhow::Error {
    anyhow!("{value:?}")
}
