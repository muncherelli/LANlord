//! the browser's backend: every op goes to the web ui's `/api/op` as json.
//!
//! a browser cannot wait for an answer, so [`Http::send`] hands the reply to a callback.
//! the synchronous [`Gateway::call`] the desktop uses only says so.

use anyhow::{anyhow, Result};
use lanlord_model::ops::{Op, Reply, Request, Response};
use lanlord_model::secret::Secret;
use wasm_bindgen::JsCast;

use super::Gateway;
use crate::ask::AskPassword;

/// the gateway this page was served from.
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
    /// post `op` (with the sudo `password` when the server asked for one) and call `done` with
    /// the reply. a [`Reply::NeedsPassword`] means: ask the person, then send the op again with
    /// the password.
    pub fn send(
        &self,
        op: Op,
        password: Option<Secret>,
        done: impl FnOnce(Result<Reply>) + 'static,
    ) {
        let endpoint = self.endpoint.clone();
        let request = Request { op, password };
        wasm_bindgen_futures::spawn_local(async move {
            done(post(&endpoint, &request).await);
        });
    }
}

impl Gateway for Http {
    fn call(&self, _host: &str, _op: Op, _ask: AskPassword) -> Result<Reply> {
        Err(anyhow!(
            "the browser cannot wait for the server; the web ui sends ops with Http::send"
        ))
    }

    fn is_local(&self) -> bool {
        // the web ui only runs on the gateway itself
        true
    }
}

async fn post(endpoint: &str, request: &Request) -> Result<Reply> {
    let body = serde_json::to_string(request)?;
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_body(&wasm_bindgen::JsValue::from_str(&body));
    let web_request = web_sys::Request::new_with_str_and_init(endpoint, &init).map_err(js)?;
    web_request
        .headers()
        .set("content-type", "application/json")
        .map_err(js)?;
    let window = web_sys::window().ok_or_else(|| anyhow!("no window"))?;
    let answer = wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(&web_request))
        .await
        .map_err(js)?;
    let answer: web_sys::Response = answer.dyn_into().map_err(js)?;
    let status = answer.status();
    let text = wasm_bindgen_futures::JsFuture::from(answer.text().map_err(js)?)
        .await
        .map_err(js)?
        .as_string()
        .unwrap_or_default();
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
