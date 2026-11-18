//! live numbers for the web desktop: the gateway reads its own `/proc` in-process (no ssh to
//! itself) and the page asks for a fresh sample about once a second. there is no other live
//! channel in the web ui to push on, and a plain request per sample keeps the session, origin and
//! timeout rules of every other route. anyone logged in may look; ending a process is for admins,
//! the same rule as `/api/operations`.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use lanlord_core::monitor::{end_process, Sampler};

use super::auth::{require_current, require_peek, same_origin_json, Auth};

fn reply(status: StatusCode, body: String) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

fn error(status: StatusCode, text: &str) -> Response {
    reply(status, serde_json::json!({ "error": text }).to_string())
}

/// GET /api/monitor, or /api/monitor?procs=1 with the process list: one sample as json.
pub async fn sample(State(auth): State<Arc<Auth>>, headers: HeaderMap, uri: Uri) -> Response {
    // a poll the page makes on its own must not count as activity
    if let Err(unauthorized) = require_peek(&auth, &headers) {
        return unauthorized;
    }
    let procs = uri
        .query()
        .is_some_and(|q| q.split('&').any(|p| p == "procs=1"));
    let read = tokio::task::spawn_blocking(move || Sampler::default().sample(procs)).await;
    match read {
        Ok(Ok(sample)) => match serde_json::to_string(&sample) {
            Ok(text) => reply(StatusCode::OK, text),
            Err(_) => error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not encode the sample",
            ),
        },
        _ => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "this host cannot be monitored",
        ),
    }
}

/// the pid in `{"pid":123}`, and nothing else in the body.
fn parse_end(body: &[u8]) -> Option<u32> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let object = value.as_object().filter(|o| o.len() == 1)?;
    let pid = u32::try_from(object.get("pid")?.as_u64()?).ok()?;
    // 0 and anything above i32::MAX would reach kill as a process group or as -1
    (1..=i32::MAX as u32).contains(&pid).then_some(pid)
}

/// POST /api/monitor/end {"pid":123}: end a process. administrators only, decided here.
pub async fn end(
    State(auth): State<Arc<Auth>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if let Err(refused) = same_origin_json(&headers) {
        return refused;
    }
    let admin = match require_current(&auth, &headers).await {
        Ok((_, admin)) => admin,
        Err(unauthorized) => return unauthorized,
    };
    if !admin {
        return error(
            StatusCode::FORBIDDEN,
            "only administrators may end a process",
        );
    }
    let Some(pid) = parse_end(&body) else {
        return error(StatusCode::BAD_REQUEST, "expected {\"pid\":number}");
    };
    match tokio::task::spawn_blocking(move || end_process(pid)).await {
        Ok(Ok(())) => reply(StatusCode::OK, r#"{"ok":true}"#.into()),
        Ok(Err(err)) => error(StatusCode::BAD_REQUEST, &err.to_string()),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not end the process",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_end;

    #[test]
    fn the_end_body_is_one_pid_and_nothing_else() {
        assert_eq!(parse_end(br#"{"pid":42}"#), Some(42));
        assert_eq!(parse_end(br#"{"pid":2147483647}"#), Some(i32::MAX as u32));
        for bad in [
            &br#"{"pid":-1}"#[..],
            br#"{"pid":"4"}"#,
            br#"{"pid":4,"x":1}"#,
            br#"{"pid":99999999999}"#,
            br#"{"pid":0}"#,
            br#"{"pid":2147483648}"#,
            br#"{"pid":4294967295}"#,
            b"[]",
            b"nope",
        ] {
            assert_eq!(parse_end(bad), None, "{:?}", String::from_utf8_lossy(bad));
        }
    }
}
