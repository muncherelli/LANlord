//! ending a process from the web desktop. the live numbers themselves come from the event stream
//! (see `stream`); ending a process is for admins, the same rule as `/api/op`.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use lanlord_core::monitor::end_process;

use super::auth::{require, same_origin_json, Auth};

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
    let admin = match require(&auth, &headers) {
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
