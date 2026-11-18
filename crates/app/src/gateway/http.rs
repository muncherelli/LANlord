//! the browser's backend: every operation goes to the web ui's `/api/operations` as json.
//!
//! a browser cannot wait for an answer, so operations are awaited with [`Http::exchange`]; tasks run
//! on it in passes (see `replay`). the synchronous [`Gateway::call`] the desktop uses only says so.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use lanlord_model::operations::{Operation, Reply, Request};
use lanlord_model::secret::Secret;
use lanlord_model::signin::{Identity, SigninError};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use zeroize::Zeroizing;

use super::round_trip::{self, SessionEnded};
use super::Gateway;
use crate::ask::AskPassword;
use crate::login::{login_status, LocalFuture, WebSession};

/// the gateway this page was served from.
#[derive(Clone)]
pub struct Http {
    /// where operations are posted, `/api/operations` on the page's own server.
    pub endpoint: String,
    /// set once the server answered that the session is gone; shared by every clone.
    ended: Arc<AtomicBool>,
}

impl Default for Http {
    fn default() -> Self {
        Self {
            endpoint: "/api/operations".into(),
            ended: Arc::default(),
        }
    }
}

impl Http {
    /// post one operation, with the sudo password once the server has asked for it. an answer that
    /// says the session is over also raises the flag [`Gateway::session_ended`] reads.
    pub async fn exchange(&self, operation: Operation, password: Option<Secret>) -> Result<Reply> {
        let answer = post(
            &self.endpoint,
            Request {
                operation,
                password,
            },
        )
        .await;
        if matches!(&answer, Err(err) if err.is::<SessionEnded>()) {
            self.ended.store(true, Ordering::SeqCst);
        }
        answer
    }
}

impl Gateway for Http {
    fn call(&self, _host: &str, _operation: Operation, _ask: AskPassword) -> Result<Reply> {
        Err(anyhow!(
            "the browser cannot wait for the server; tasks send operations with Http::exchange"
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
    let operation = request.operation.clone();
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
    let timeout = round_trip::timeout_ms(&operation);
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

/// the browser's logon: `/api/login`, `/api/session` and `/api/logout` on the page's own server.
/// the session itself is the `__Host-` cookie, which the browser keeps and JavaScript cannot see.
#[derive(Clone, Copy, Default)]
pub struct HttpSession;

impl WebSession for HttpSession {
    fn login(
        &self,
        user: String,
        password: Zeroizing<String>,
    ) -> LocalFuture<Result<Identity, SigninError>> {
        Box::pin(async move {
            // the body holds the password in plain text: keep it only until the browser has it
            let body = Zeroizing::new(
                serde_json::json!({ "user": user, "password": password.as_str() }).to_string(),
            );
            drop(password);
            let (status, _) = fetch("POST", "/api/login", Some(body))
                .await
                .map_err(|e| SigninError::Unreachable(e.to_string()))?;
            login_status(status)?;
            session_now()
                .await
                .ok()
                .flatten()
                .ok_or_else(|| SigninError::Other("The server did not start a session.".into()))
        })
    }

    fn current(&self) -> LocalFuture<anyhow::Result<Option<Identity>>> {
        Box::pin(session_now())
    }

    fn logout(&self) -> LocalFuture<()> {
        Box::pin(async {
            let _ = fetch("POST", "/api/logout", Some(Zeroizing::new("{}".into()))).await;
        })
    }
}

/// who `/api/session` says is logged on, and whether they are an admin. the api does not tell a
/// uid, so one is made up (above root's, so nothing treats the account as root).
/// `Ok(None)` is the server saying there is no session; an error is not being able to ask.
async fn session_now() -> Result<Option<Identity>> {
    let (status, text) = fetch("GET", "/api/session", None).await?;
    if status == 401 {
        return Ok(None);
    }
    if status != 200 {
        return Err(anyhow!("the server answered {status}"));
    }
    let value: serde_json::Value = serde_json::from_str(&text)?;
    let field = |name: &str| anyhow!("the session answer has no {name}");
    Ok(Some(Identity {
        user: value
            .get("user")
            .and_then(|v| v.as_str())
            .ok_or_else(|| field("user"))?
            .to_string(),
        uid: 1000,
        admin: value
            .get("admin")
            .and_then(|v| v.as_bool())
            .ok_or_else(|| field("admin"))?,
        // the server reads the account's roles now; an answer without them leaves the fallback
        grants: value
            .get("grants")
            .and_then(|v| serde_json::from_value(v.clone()).ok()),
    }))
}

/// end a process on the gateway; the server only lets administrators.
pub async fn monitor_end(pid: u32) -> Result<()> {
    let body = Zeroizing::new(serde_json::json!({ "pid": pid }).to_string());
    let (status, text) = fetch("POST", "/api/monitor/end", Some(body)).await?;
    monitor_status(status, &text)
}

/// a monitor route answers 401 only for want of a login, so that is the session ending, the
/// same as for an operation: the desktop goes back to the logon box.
fn monitor_status(status: u16, body: &str) -> Result<()> {
    match status {
        200 => Ok(()),
        401 => Err(SessionEnded.into()),
        _ => Err(anyhow!(error_text(status, body))),
    }
}

/// the server's `{"error":..}` text, or the status.
fn error_text(status: u16, body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error")?.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("the server answered {status}"))
}

/// one request to the page's own server, answered with its status and body text. a post sends
/// json; the browser adds the Origin and fetch metadata headers the server checks.
async fn fetch(method: &str, url: &str, body: Option<Zeroizing<String>>) -> Result<(u16, String)> {
    let window = web_sys::window().ok_or_else(|| anyhow!("no window"))?;
    let controller = web_sys::AbortController::new().map_err(js)?;
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    init.set_signal(Some(&controller.signal()));
    if let Some(body) = &body {
        init.set_body(&wasm_bindgen::JsValue::from_str(body));
    }
    drop(body);
    let request = web_sys::Request::new_with_str_and_init(url, &init).map_err(js)?;
    if method == "POST" {
        request
            .headers()
            .set("content-type", "application/json")
            .map_err(js)?;
    }
    let abort = Closure::once(move || controller.abort());
    let timer = window
        .set_timeout_with_callback_and_timeout_and_arguments_0(
            abort.as_ref().unchecked_ref(),
            30_000,
        )
        .map_err(js)?;
    let no_answer = || anyhow!("no answer from the server");
    let fetched = async {
        let answer: web_sys::Response =
            wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(&request))
                .await
                .map_err(|_| no_answer())?
                .dyn_into()
                .map_err(js)?;
        let text = wasm_bindgen_futures::JsFuture::from(answer.text().map_err(js)?)
            .await
            .map_err(|_| no_answer())?
            .as_string()
            .unwrap_or_default();
        Ok::<_, anyhow::Error>((answer.status(), text))
    }
    .await;
    window.clear_timeout_with_handle(timer);
    drop(abort);
    fetched
}
