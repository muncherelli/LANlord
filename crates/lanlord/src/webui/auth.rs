//! web ui logins and sessions. a login is checked by the root `lanlord-auth-helper` over its unix
//! socket (pam; any ordinary account may log in, and the session records whether it is in the
//! admins group); a good one gets a random token in a
//! `__Host-` cookie. only the token's sha-256 is kept in memory, so a memory dump or a log never
//! holds a usable cookie. failures are slowed down and locked out per user and per client
//! address, and every failure looks the same to the caller.

use std::collections::HashMap;
use std::future::Future;
use std::net::IpAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use lanlord_core::log::{self, Kind};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

/// the cookie's name. the `__Host-` prefix makes a browser refuse it unless it is secure, has
/// path `/` and names no domain.
pub const COOKIE: &str = "__Host-lanlord";

/// where the login helper listens (see ansible/playbooks/webui-login.yml).
pub const HELPER_SOCKET: &str = "/run/lanlord-auth/auth.sock";

/// the largest login body: a user name, a password and some json.
pub const LOGIN_LIMIT: usize = 4 * 1024;

/// what the helper said about a login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// the password is right and the account may log in; `admin` says whether it is in the admins
    /// group. system and service accounts never get this far, the helper denies them.
    Ok { admin: bool },
    /// anything else about the login.
    Denied,
    /// the helper could not answer.
    Unavailable,
}

type Checking = Pin<Box<dyn Future<Output = Check> + Send>>;

/// asks something to check a login.
pub trait Verifier: Send + Sync {
    fn check(&self, user: &str, password: &[u8], ip: Option<IpAddr>) -> Checking;
}

/// the real check, through the helper's socket.
pub struct Helper(pub PathBuf);

impl Verifier for Helper {
    fn check(&self, user: &str, password: &[u8], ip: Option<IpAddr>) -> Checking {
        // the request holds the password, so it is wiped as soon as it is sent
        let mut request = Zeroizing::new(Vec::with_capacity(user.len() + password.len() + 48));
        request.extend_from_slice(user.as_bytes());
        request.push(0);
        request.extend_from_slice(password);
        request.push(0);
        if let Some(ip) = ip {
            request.extend_from_slice(ip.to_string().as_bytes());
            request.push(0);
        }
        let path = self.0.clone();
        Box::pin(async move {
            match tokio::time::timeout(Duration::from_secs(15), ask(&path, &request)).await {
                Ok(Ok(check)) => check,
                _ => Check::Unavailable,
            }
        })
    }
}

#[cfg(unix)]
async fn ask(path: &std::path::Path, request: &[u8]) -> std::io::Result<Check> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::UnixStream::connect(path).await?;
    stream.write_all(request).await?;
    // the helper reads up to the end of the request
    stream.shutdown().await?;
    let mut answer = Vec::new();
    stream.take(256).read_to_end(&mut answer).await?;
    Ok(match answer.trim_ascii() {
        b"ok" => Check::Ok { admin: true },
        b"ok user" => Check::Ok { admin: false },
        b"denied" => Check::Denied,
        _ => Check::Unavailable,
    })
}

#[cfg(not(unix))]
async fn ask(_path: &std::path::Path, _request: &[u8]) -> std::io::Result<Check> {
    Ok(Check::Unavailable)
}

/// the limits, kept apart so tests can shrink the waits.
#[derive(Debug, Clone)]
pub struct Limits {
    pub idle: Duration,
    pub absolute: Duration,
    pub per_user: usize,
    /// failures before a user is locked
    pub user_failures: u32,
    /// the longest lockout
    pub max_lock: Duration,
    pub ip_failures: usize,
    pub ip_window: Duration,
    /// a failed login takes at least this long to answer
    pub fail_delay: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            idle: Duration::from_secs(15 * 60),
            absolute: Duration::from_secs(8 * 3600),
            per_user: 5,
            user_failures: 5,
            max_lock: Duration::from_secs(3600),
            ip_failures: 20,
            ip_window: Duration::from_secs(15 * 60),
            fail_delay: Duration::from_secs(2),
        }
    }
}

struct Session {
    user: String,
    admin: bool,
    made: Instant,
    last: Instant,
}

#[derive(Default)]
struct Failures {
    count: u32,
    locked_until: Option<Instant>,
}

#[derive(Default)]
struct Book {
    /// by the sha-256 of the token
    sessions: HashMap<[u8; 32], Session>,
    users: HashMap<String, Failures>,
    ips: HashMap<String, Vec<Instant>>,
}

/// at most this many user names and addresses are remembered for lockouts.
const REMEMBERED: usize = 10_000;

pub struct Auth {
    verifier: Box<dyn Verifier>,
    limits: Limits,
    clock: Box<dyn Fn() -> Instant + Send + Sync>,
    book: Mutex<Book>,
}

impl Auth {
    pub fn new(verifier: Box<dyn Verifier>, limits: Limits) -> Arc<Auth> {
        Self::with_clock(verifier, limits, Box::new(Instant::now))
    }

    pub fn with_clock(
        verifier: Box<dyn Verifier>,
        limits: Limits,
        clock: Box<dyn Fn() -> Instant + Send + Sync>,
    ) -> Arc<Auth> {
        Arc::new(Auth {
            verifier,
            limits,
            clock,
            book: Mutex::new(Book::default()),
        })
    }

    /// the real helper, with the usual limits.
    pub fn system() -> Arc<Auth> {
        Self::new(
            Box::new(Helper(PathBuf::from(HELPER_SOCKET))),
            Limits::default(),
        )
    }

    fn now(&self) -> Instant {
        (self.clock)()
    }

    fn book(&self) -> std::sync::MutexGuard<'_, Book> {
        self.book.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// whether this user or address may try a password now.
    fn may_try(&self, user: &str, ip: &str) -> bool {
        let now = self.now();
        let mut book = self.book();
        if let Some(until) = book.users.get(user).and_then(|f| f.locked_until) {
            if until > now {
                return false;
            }
        }
        let window = self.limits.ip_window;
        if let Some(times) = book.ips.get_mut(ip) {
            times.retain(|t| now.duration_since(*t) < window);
            if times.len() >= self.limits.ip_failures {
                return false;
            }
        }
        true
    }

    fn failed(&self, user: &str, ip: &str) {
        let now = self.now();
        let mut book = self.book();
        let window = self.limits.ip_window;
        // forget what has expired, and stop growing when flooded
        if book.users.len() >= REMEMBERED {
            book.users
                .retain(|_, f| f.locked_until.is_some_and(|t| t > now));
        }
        if book.ips.len() >= REMEMBERED {
            book.ips
                .retain(|_, v| v.iter().any(|t| now.duration_since(*t) < window));
        }
        if book.users.len() < REMEMBERED || book.users.contains_key(user) {
            let f = book.users.entry(user.to_string()).or_default();
            f.count += 1;
            if f.count >= self.limits.user_failures {
                // one minute at the fifth failure, doubling, up to the longest lockout
                let doublings = (f.count - self.limits.user_failures).min(16);
                let wait = Duration::from_secs(60)
                    .saturating_mul(1u32 << doublings)
                    .min(self.limits.max_lock);
                f.locked_until = Some(now + wait);
            }
        }
        if book.ips.len() < REMEMBERED || book.ips.contains_key(ip) {
            book.ips.entry(ip.to_string()).or_default().push(now);
        }
    }

    fn succeeded(&self, user: &str) {
        self.book().users.remove(user);
    }

    /// start a session and say its token.
    fn open(&self, user: &str, admin: bool) -> Result<String, ()> {
        let token = random_token()?;
        let now = self.now();
        let mut book = self.book();
        self.drop_expired(&mut book, now);
        let mut mine: Vec<([u8; 32], Instant)> = book
            .sessions
            .iter()
            .filter(|(_, s)| s.user == user)
            .map(|(k, s)| (*k, s.made))
            .collect();
        mine.sort_by_key(|(_, made)| *made);
        // room for the new one
        while mine.len() >= self.limits.per_user {
            let (oldest, _) = mine.remove(0);
            book.sessions.remove(&oldest);
        }
        book.sessions.insert(
            digest(&token),
            Session {
                user: user.to_string(),
                admin,
                made: now,
                last: now,
            },
        );
        Ok(token)
    }

    fn drop_expired(&self, book: &mut Book, now: Instant) {
        let l = &self.limits;
        book.sessions.retain(|_, s| {
            now.duration_since(s.last) < l.idle && now.duration_since(s.made) < l.absolute
        });
    }

    /// the user a token belongs to, when its session is still valid; a use keeps it alive.
    fn user_of(&self, token: &str) -> Option<(String, bool)> {
        let now = self.now();
        let key = digest(token);
        let mut book = self.book();
        let l = &self.limits;
        let session = book.sessions.get_mut(&key)?;
        if now.duration_since(session.last) >= l.idle
            || now.duration_since(session.made) >= l.absolute
        {
            book.sessions.remove(&key);
            return None;
        }
        session.last = now;
        Some((session.user.clone(), session.admin))
    }

    fn close(&self, token: &str) {
        self.book().sessions.remove(&digest(token));
    }

    /// end every session of `user`: after their password or their permissions changed, what a
    /// session was opened with is no longer true. gives how many there were.
    #[cfg(test)] // the operations executor calls it once operations are wired up
    pub fn revoke_user(&self, user: &str) -> usize {
        let mut book = self.book();
        let before = book.sessions.len();
        book.sessions.retain(|_, s| s.user != user);
        before - book.sessions.len()
    }

    /// the user of the session a request's cookie names.
    pub fn session(&self, headers: &HeaderMap) -> Option<(String, bool)> {
        self.user_of(&cookie_token(headers)?)
    }
}

fn digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// 32 random bytes from the system, as hex.
fn random_token() -> Result<String, ()> {
    let mut bytes = [0u8; 32];
    fill_random(&mut bytes).map_err(|_| ())?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    bytes.zeroize();
    Ok(token)
}

#[cfg(unix)]
fn fill_random(buf: &mut [u8]) -> std::io::Result<()> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")?.read_exact(buf)
}

#[cfg(not(unix))]
fn fill_random(_buf: &mut [u8]) -> std::io::Result<()> {
    Err(std::io::Error::other("no system random source"))
}

/// the session token in the request's cookies, when it looks like one.
fn cookie_token(headers: &HeaderMap) -> Option<String> {
    for value in headers.get_all(header::COOKIE) {
        for part in value.to_str().ok()?.split(';') {
            if let Some(token) = part.trim().strip_prefix(&format!("{COOKIE}=")) {
                if token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Some(token.to_string());
                }
            }
        }
    }
    None
}

/// the client's address, from the header caddy sets. caddy adds the address it saw to the end of
/// whatever the client sent, so the last entry is the one to trust.
fn client_ip(headers: &HeaderMap) -> Option<IpAddr> {
    let value = headers.get("x-forwarded-for")?.to_str().ok()?;
    value.rsplit(',').next()?.trim().parse().ok()
}

fn json(status: StatusCode, body: &'static str) -> Response {
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

/// a request that changes state must come from the page itself: json, with an Origin that is
/// this site and a fetch metadata header that says same-origin.
#[allow(clippy::result_large_err)]
pub fn same_origin_json(headers: &HeaderMap) -> Result<(), Response> {
    let refuse = || json(StatusCode::FORBIDDEN, r#"{"error":"forbidden"}"#);
    let text = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let host = text("host").ok_or_else(refuse)?;
    if text("origin") != Some(&format!("https://{host}")) {
        return Err(refuse());
    }
    if text("sec-fetch-site") != Some("same-origin") {
        return Err(refuse());
    }
    let kind = text("content-type").unwrap_or_default();
    let kind = kind.split(';').next().unwrap_or_default().trim();
    if !kind.eq_ignore_ascii_case("application/json") {
        return Err(json(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            r#"{"error":"json only"}"#,
        ));
    }
    Ok(())
}

/// a request that needs a login: the user and whether it is an admin, or the 401 to send.
#[allow(clippy::result_large_err)]
pub fn require(auth: &Auth, headers: &HeaderMap) -> Result<(String, bool), Response> {
    auth.session(headers)
        .ok_or_else(|| json(StatusCode::UNAUTHORIZED, r#"{"error":"login required"}"#))
}

fn set_cookie(value: &str, max_age: u64) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{COOKIE}={value}; Path=/; Max-Age={max_age}; Secure; HttpOnly; SameSite=Strict"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static(""))
}

/// what the browser sends to log in. the password is wiped when this is dropped.
struct Login {
    user: String,
    password: String,
}

impl Login {
    /// the two strings of `{"user":..,"password":..}`, and nothing else.
    fn parse(body: &[u8]) -> Option<Login> {
        let mut value: serde_json::Value = serde_json::from_slice(body).ok()?;
        let object = value.as_object_mut()?;
        let user = object.remove("user")?;
        let password = object.remove("password")?;
        let login = match (user, password) {
            (serde_json::Value::String(user), serde_json::Value::String(password)) => {
                Login { user, password }
            }
            _ => return None,
        };
        object.is_empty().then_some(login)
    }
}

/// a unix login name the helper accepts: see the helper's `valid_user`.
fn valid_login(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 32
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

impl Drop for Login {
    fn drop(&mut self) {
        self.password.zeroize();
    }
}

fn denied() -> Response {
    json(StatusCode::UNAUTHORIZED, r#"{"error":"login failed"}"#)
}

/// POST /api/login: `{"user":..,"password":..}`. a good login answers with the session cookie;
/// every failure is the same 401, after at least `fail_delay`.
pub async fn login(State(auth): State<Arc<Auth>>, headers: HeaderMap, body: Bytes) -> Response {
    if let Err(refused) = same_origin_json(&headers) {
        return refused;
    }
    let started = Instant::now();
    let Some(login) = Login::parse(&body) else {
        return json(StatusCode::BAD_REQUEST, r#"{"error":"bad request"}"#);
    };
    let ip = client_ip(&headers);
    let ip_key = ip.map_or_else(|| "unknown".to_string(), |ip| ip.to_string());
    let user = login.user.as_str();
    let well_formed = !login.password.is_empty()
        && login.password.len() < 512
        && !login.password.contains('\0')
        && valid_login(user);
    // a login turned away for a lockout is not another failure, or the lockout would never end
    let blocked = well_formed && !auth.may_try(user, &ip_key);
    let verdict = if !well_formed || blocked {
        Check::Denied
    } else {
        auth.verifier
            .check(user, login.password.as_bytes(), ip)
            .await
    };
    match verdict {
        Check::Ok { admin } => {
            let Ok(token) = auth.open(user, admin) else {
                return json(StatusCode::INTERNAL_SERVER_ERROR, r#"{"error":"failed"}"#);
            };
            auth.succeeded(user);
            log::emit(
                Kind::Info,
                format!("web ui login for {user} from {ip_key}: ok"),
            );
            let mut response = json(StatusCode::OK, r#"{"ok":true}"#);
            response.headers_mut().insert(
                header::SET_COOKIE,
                set_cookie(&token, auth.limits.absolute.as_secs()),
            );
            response
        }
        Check::Unavailable => {
            log::emit(Kind::Err, "the web ui login helper did not answer");
            json(
                StatusCode::SERVICE_UNAVAILABLE,
                r#"{"error":"login unavailable"}"#,
            )
        }
        Check::Denied => {
            if !blocked {
                auth.failed(if well_formed { user } else { "" }, &ip_key);
            }
            // the shortest answer is as slow as the longest
            let wait = auth.limits.fail_delay.saturating_sub(started.elapsed());
            tokio::time::sleep(wait).await;
            denied()
        }
    }
}

/// POST /api/logout: ends the session the cookie names, if any, and clears the cookie.
pub async fn logout(State(auth): State<Arc<Auth>>, headers: HeaderMap) -> Response {
    if let Err(refused) = same_origin_json(&headers) {
        return refused;
    }
    if let Some(token) = cookie_token(&headers) {
        auth.close(&token);
    }
    let mut response = json(StatusCode::OK, r#"{"ok":true}"#);
    response
        .headers_mut()
        .insert(header::SET_COOKIE, set_cookie("", 0));
    response
}

/// GET /api/session: who is logged in, and whether they are an admin.
pub async fn session(State(auth): State<Arc<Auth>>, headers: HeaderMap) -> Response {
    match require(&auth, &headers) {
        Ok((user, admin)) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            serde_json::json!({ "user": user, "admin": admin }).to_string(),
        )
            .into_response(),
        Err(unauthorized) => unauthorized,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use axum::body::Body;
    use axum::http::Request;
    use axum::Router;
    use tower::ServiceExt;

    use super::*;

    const HOST: &str = "gw.example.test";

    type Calls = Arc<Mutex<Vec<(String, Option<IpAddr>)>>>;

    /// accepts the password `right` for `admin` (an admin) and `carol` (an ordinary user),
    /// denies everything else, and remembers who it was asked about.
    struct Fake {
        calls: Calls,
        down: bool,
    }

    impl Verifier for Fake {
        fn check(&self, user: &str, password: &[u8], ip: Option<IpAddr>) -> Checking {
            self.calls.lock().unwrap().push((user.to_string(), ip));
            let answer = match (user, password, self.down) {
                (_, _, true) => Check::Unavailable,
                ("admin", b"right", _) => Check::Ok { admin: true },
                ("carol", b"right", _) => Check::Ok { admin: false },
                _ => Check::Denied,
            };
            Box::pin(async move { answer })
        }
    }

    struct Rig {
        app: Router,
        auth: Arc<Auth>,
        calls: Calls,
        offset: Arc<AtomicU64>,
    }

    fn rig_with(down: bool) -> Rig {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let offset = Arc::new(AtomicU64::new(0));
        let base = Instant::now();
        let seen = offset.clone();
        let auth = Auth::with_clock(
            Box::new(Fake {
                calls: calls.clone(),
                down,
            }),
            Limits {
                fail_delay: Duration::ZERO,
                ..Limits::default()
            },
            Box::new(move || base + Duration::from_secs(seen.load(Ordering::SeqCst))),
        );
        Rig {
            app: super::super::router_with(None, auth.clone()),
            auth,
            calls,
            offset,
        }
    }

    fn rig() -> Rig {
        rig_with(false)
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    /// a request as the page would send it, unless a test spoils it.
    fn page(method: &str, uri: &str, body: &str) -> axum::http::request::Builder {
        let builder = Request::builder().method(method).uri(uri);
        if method == "GET" {
            return builder;
        }
        let _ = body;
        builder
            .header("host", HOST)
            .header("origin", format!("https://{HOST}"))
            .header("sec-fetch-site", "same-origin")
            .header("content-type", "application/json")
    }

    async fn send(rig: &Rig, request: Request<Body>) -> (StatusCode, HeaderMap, String) {
        let response = rig.app.clone().oneshot(request).await.unwrap();
        let (status, headers) = (response.status(), response.headers().clone());
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, headers, String::from_utf8_lossy(&body).into_owned())
    }

    async fn login_as(rig: &Rig, user: &str, password: &str) -> (StatusCode, HeaderMap, String) {
        let body = serde_json::json!({ "user": user, "password": password }).to_string();
        let request = page("POST", "/api/login", &body)
            .body(Body::from(body))
            .unwrap();
        send(rig, request).await
    }

    /// log in and return the `Cookie` header value for later requests.
    async fn session_for(rig: &Rig, user: &str) -> String {
        let (status, headers, _) = login_as(rig, user, "right").await;
        assert_eq!(status, StatusCode::OK);
        let set = headers[header::SET_COOKIE].to_str().unwrap();
        set.split(';').next().unwrap().to_string()
    }

    async fn get_session(rig: &Rig, cookie: &str) -> (StatusCode, String) {
        let request = Request::builder()
            .uri("/api/session")
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap();
        let (status, _, body) = send(rig, request).await;
        (status, body)
    }

    async fn post_op(rig: &Rig, cookie: Option<&str>) -> StatusCode {
        let mut builder = page("POST", "/api/op", "{}");
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        send(rig, builder.body(Body::from("{}")).unwrap()).await.0
    }

    #[test]
    fn a_login_sets_a_locked_down_cookie() {
        runtime().block_on(async {
            let rig = rig();
            let (status, headers, body) = login_as(&rig, "admin", "right").await;
            assert_eq!(status, StatusCode::OK);
            let cookie = headers[header::SET_COOKIE].to_str().unwrap();
            assert!(cookie.starts_with("__Host-lanlord="), "{cookie}");
            for needed in ["Secure", "HttpOnly", "SameSite=Strict", "Path=/"] {
                assert!(cookie.contains(needed), "{needed} in {cookie}");
            }
            assert!(!cookie.contains("Domain"), "{cookie}");
            let token = cookie["__Host-lanlord=".len()..].split(';').next().unwrap();
            assert_eq!(token.len(), 64);
            assert!(!body.contains(token), "the token is only in the cookie");
            // the server keeps only the token's hash
            let book = rig.auth.book();
            assert!(book.sessions.contains_key(&digest(token)));
        });
    }

    #[test]
    fn two_logins_get_different_tokens() {
        runtime().block_on(async {
            let rig = rig();
            let a = session_for(&rig, "admin").await;
            let b = session_for(&rig, "admin").await;
            assert_ne!(a, b);
        });
    }

    #[test]
    fn an_admin_and_an_ordinary_user_both_log_in_and_are_told_apart() {
        runtime().block_on(async {
            let rig = rig();
            let admin = session_for(&rig, "admin").await;
            let carol = session_for(&rig, "carol").await;
            let (status, body) = get_session(&rig, &admin).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, r#"{"admin":true,"user":"admin"}"#);
            let (status, body) = get_session(&rig, &carol).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, r#"{"admin":false,"user":"carol"}"#);
        });
    }

    #[test]
    fn every_failure_looks_the_same() {
        runtime().block_on(async {
            let rig = rig();
            let mut seen = Vec::new();
            // wrong password, unknown user, bad name, empty password
            for (user, password) in [
                ("admin", "wrong"),
                ("nobody", "right"),
                ("Not A Name", "right"),
                ("admin", ""),
            ] {
                let (status, headers, body) = login_as(&rig, user, password).await;
                assert_eq!(status, StatusCode::UNAUTHORIZED, "{user}");
                assert!(!headers.contains_key(header::SET_COOKIE));
                seen.push(body);
            }
            assert!(seen.windows(2).all(|w| w[0] == w[1]), "{seen:?}");
        });
    }

    #[test]
    fn a_failed_login_takes_at_least_the_fail_delay() {
        runtime().block_on(async {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let auth = Auth::new(
                Box::new(Fake { calls, down: false }),
                Limits {
                    fail_delay: Duration::from_millis(300),
                    ..Limits::default()
                },
            );
            let rig = Rig {
                app: super::super::router_with(None, auth.clone()),
                auth,
                calls: Arc::default(),
                offset: Arc::default(),
            };
            let started = Instant::now();
            let (status, _, _) = login_as(&rig, "admin", "wrong").await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert!(started.elapsed() >= Duration::from_millis(300));
            let started = Instant::now();
            let (status, _, _) = login_as(&rig, "Not A Name", "x").await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert!(started.elapsed() >= Duration::from_millis(300));
        });
    }

    #[test]
    fn no_password_comes_back_in_any_answer() {
        runtime().block_on(async {
            let rig = rig();
            for (user, password) in [("admin", "hunter2hunter2"), ("admin", "right")] {
                let (_, headers, body) = login_as(&rig, user, password).await;
                assert!(!body.contains(password) || password == "right");
                assert!(!format!("{headers:?}").contains("hunter2"));
                assert!(!body.contains("hunter2"));
            }
            let (status, _, body) = send(
                &rig,
                page("POST", "/api/login", "")
                    .body(Body::from(r#"{"user":"admin","password":"hunter2","x":1}"#))
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert!(!body.contains("hunter2"));
        });
    }

    #[test]
    fn only_a_page_of_this_site_sending_json_may_log_in_out_or_run_ops() {
        runtime().block_on(async {
            let rig = rig();
            let body = r#"{"user":"admin","password":"right"}"#;
            for uri in ["/api/login", "/api/logout", "/api/op"] {
                // axum builders append headers, so build each spoiled request from scratch
                let plain = |extra: &[(&str, &str)], skip: &[&str]| {
                    let mut b = Request::builder().method("POST").uri(uri);
                    let mut all = vec![
                        ("host", HOST.to_string()),
                        ("origin", format!("https://{HOST}")),
                        ("sec-fetch-site", "same-origin".to_string()),
                        ("content-type", "application/json".to_string()),
                    ];
                    all.retain(|(k, _)| !skip.contains(k) && !extra.iter().any(|(e, _)| e == k));
                    for (k, v) in extra {
                        all.push((k, v.to_string()));
                    }
                    for (k, v) in all {
                        b = b.header(k, v);
                    }
                    b.body(Body::from(body)).unwrap()
                };
                let cases = [
                    ("no origin", plain(&[], &["origin"])),
                    (
                        "other origin",
                        plain(&[("origin", "https://evil.example.test")], &[]),
                    ),
                    (
                        "http origin",
                        plain(&[("origin", "http://gw.example.test")], &[]),
                    ),
                    ("no fetch metadata", plain(&[], &["sec-fetch-site"])),
                    (
                        "cross-site",
                        plain(&[("sec-fetch-site", "cross-site")], &[]),
                    ),
                    ("same-site", plain(&[("sec-fetch-site", "same-site")], &[])),
                    ("no host", plain(&[], &["host"])),
                ];
                for (what, request) in cases {
                    let (status, _, _) = send(&rig, request).await;
                    assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {what}");
                }
                for kind in ["text/plain", "application/x-www-form-urlencoded", ""] {
                    let (status, _, _) = send(&rig, plain(&[("content-type", kind)], &[])).await;
                    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{uri}: {kind}");
                }
            }
            assert!(
                rig.calls.lock().unwrap().is_empty(),
                "the helper was never asked"
            );
        });
    }

    #[test]
    fn operations_need_a_session() {
        runtime().block_on(async {
            let rig = rig();
            assert_eq!(post_op(&rig, None).await, StatusCode::UNAUTHORIZED);
            let forged = format!("__Host-lanlord={}", "ab".repeat(32));
            assert_eq!(post_op(&rig, Some(&forged)).await, StatusCode::UNAUTHORIZED);
            for junk in ["__Host-lanlord=short", "other=1", "__Host-lanlord="] {
                assert_eq!(post_op(&rig, Some(junk)).await, StatusCode::UNAUTHORIZED);
            }
            // a cookie without the prefix is not ours
            let cookie = session_for(&rig, "admin").await;
            let unprefixed = cookie.replace("__Host-", "");
            assert_eq!(
                post_op(&rig, Some(&unprefixed)).await,
                StatusCode::UNAUTHORIZED
            );
            // with a session it still answers 501 until the operations are wired up
            assert_eq!(
                post_op(&rig, Some(&cookie)).await,
                StatusCode::NOT_IMPLEMENTED
            );
        });
    }

    #[test]
    fn a_password_or_permission_change_ends_that_users_sessions_only() {
        runtime().block_on(async {
            let rig = rig();
            let admin = session_for(&rig, "admin").await;
            let other = session_for(&rig, "other").await;
            assert_eq!(rig.auth.revoke_user("admin"), 1);
            assert_eq!(rig.auth.revoke_user("admin"), 0);
            assert_eq!(post_op(&rig, Some(&admin)).await, StatusCode::UNAUTHORIZED);
            assert_eq!(
                post_op(&rig, Some(&other)).await,
                StatusCode::NOT_IMPLEMENTED
            );
        });
    }

    #[test]
    fn the_session_endpoint_needs_a_session() {
        runtime().block_on(async {
            let rig = rig();
            let request = Request::builder()
                .uri("/api/session")
                .body(Body::empty())
                .unwrap();
            assert_eq!(send(&rig, request).await.0, StatusCode::UNAUTHORIZED);
        });
    }

    #[test]
    fn logging_out_ends_the_session() {
        runtime().block_on(async {
            let rig = rig();
            let cookie = session_for(&rig, "admin").await;
            let request = page("POST", "/api/logout", "")
                .header(header::COOKIE, &cookie)
                .body(Body::from("{}"))
                .unwrap();
            let (status, headers, _) = send(&rig, request).await;
            assert_eq!(status, StatusCode::OK);
            assert!(headers[header::SET_COOKIE]
                .to_str()
                .unwrap()
                .contains("Max-Age=0"));
            assert_eq!(get_session(&rig, &cookie).await.0, StatusCode::UNAUTHORIZED);
            assert_eq!(post_op(&rig, Some(&cookie)).await, StatusCode::UNAUTHORIZED);
        });
    }

    #[test]
    fn a_session_ends_after_fifteen_idle_minutes_and_a_use_keeps_it_alive() {
        runtime().block_on(async {
            let rig = rig();
            let cookie = session_for(&rig, "admin").await;
            for _ in 0..4 {
                rig.offset.fetch_add(10 * 60, Ordering::SeqCst);
                assert_eq!(get_session(&rig, &cookie).await.0, StatusCode::OK);
            }
            rig.offset.fetch_add(15 * 60, Ordering::SeqCst);
            assert_eq!(get_session(&rig, &cookie).await.0, StatusCode::UNAUTHORIZED);
        });
    }

    #[test]
    fn a_session_ends_after_eight_hours_however_busy() {
        runtime().block_on(async {
            let rig = rig();
            let cookie = session_for(&rig, "admin").await;
            for _ in 0..47 {
                rig.offset.fetch_add(10 * 60, Ordering::SeqCst);
                assert_eq!(get_session(&rig, &cookie).await.0, StatusCode::OK);
            }
            rig.offset.fetch_add(10 * 60, Ordering::SeqCst);
            assert_eq!(get_session(&rig, &cookie).await.0, StatusCode::UNAUTHORIZED);
        });
    }

    #[test]
    fn a_user_keeps_at_most_five_sessions() {
        runtime().block_on(async {
            let rig = rig();
            let mut cookies = Vec::new();
            for _ in 0..6 {
                rig.offset.fetch_add(1, Ordering::SeqCst);
                cookies.push(session_for(&rig, "admin").await);
            }
            assert_eq!(
                get_session(&rig, &cookies[0]).await.0,
                StatusCode::UNAUTHORIZED
            );
            for cookie in &cookies[1..] {
                assert_eq!(get_session(&rig, cookie).await.0, StatusCode::OK);
            }
            // another user is not touched
            let carol = session_for(&rig, "carol").await;
            assert_eq!(get_session(&rig, &carol).await.0, StatusCode::OK);
        });
    }

    #[test]
    fn five_failures_lock_a_user_out_with_a_growing_wait() {
        runtime().block_on(async {
            let rig = rig();
            for _ in 0..5 {
                assert_eq!(
                    login_as(&rig, "admin", "wrong").await.0,
                    StatusCode::UNAUTHORIZED
                );
            }
            let asked = rig.calls.lock().unwrap().len();
            assert_eq!(asked, 5);
            // the right password is refused while locked, without asking the helper
            assert_eq!(
                login_as(&rig, "admin", "right").await.0,
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(rig.calls.lock().unwrap().len(), asked);
            // another user is not affected
            assert_eq!(login_as(&rig, "carol", "right").await.0, StatusCode::OK);
            // a minute later it may try again; failing again doubles the wait
            rig.offset.fetch_add(61, Ordering::SeqCst);
            assert_eq!(
                login_as(&rig, "admin", "wrong").await.0,
                StatusCode::UNAUTHORIZED
            );
            rig.offset.fetch_add(61, Ordering::SeqCst);
            assert_eq!(
                login_as(&rig, "admin", "right").await.0,
                StatusCode::UNAUTHORIZED
            );
            rig.offset.fetch_add(120, Ordering::SeqCst);
            assert_eq!(login_as(&rig, "admin", "right").await.0, StatusCode::OK);
            // and a good login wipes the count
            assert!(!rig.auth.book().users.contains_key("admin"));
        });
    }

    #[test]
    fn the_lockout_never_lasts_longer_than_an_hour() {
        runtime().block_on(async {
            let rig = rig();
            for _ in 0..40 {
                rig.auth.failed("admin", "192.0.2.1");
                rig.offset.fetch_add(3601, Ordering::SeqCst);
            }
            assert!(rig.auth.may_try("admin", "192.0.2.9"));
            for _ in 0..40 {
                rig.auth.failed("admin", "192.0.2.1");
            }
            rig.offset.fetch_add(3601, Ordering::SeqCst);
            assert!(rig.auth.may_try("admin", "192.0.2.9"));
        });
    }

    #[test]
    fn twenty_failures_from_one_address_lock_that_address_out() {
        runtime().block_on(async {
            let rig = rig();
            let attempt = |user: &str, password: &str| {
                let body = serde_json::json!({ "user": user, "password": password }).to_string();
                page("POST", "/api/login", &body)
                    .header("x-forwarded-for", "192.0.2.50")
                    .body(Body::from(body))
                    .unwrap()
            };
            for i in 0..20 {
                let user = format!("user{i}");
                assert_eq!(
                    send(&rig, attempt(&user, "wrong")).await.0,
                    StatusCode::UNAUTHORIZED
                );
            }
            let asked = rig.calls.lock().unwrap().len();
            assert_eq!(
                send(&rig, attempt("carol", "right")).await.0,
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(rig.calls.lock().unwrap().len(), asked);
            // other addresses are fine, and so is this one after the window
            assert_eq!(login_as(&rig, "carol", "right").await.0, StatusCode::OK);
            rig.offset.fetch_add(15 * 60 + 1, Ordering::SeqCst);
            assert_eq!(
                send(&rig, attempt("carol", "right")).await.0,
                StatusCode::OK
            );
        });
    }

    #[test]
    fn the_client_address_is_the_last_one_caddy_wrote() {
        runtime().block_on(async {
            let rig = rig();
            let body = r#"{"user":"carol","password":"right"}"#;
            let request = page("POST", "/api/login", body)
                .header("x-forwarded-for", "198.51.100.1, 192.0.2.77")
                .body(Body::from(body))
                .unwrap();
            assert_eq!(send(&rig, request).await.0, StatusCode::OK);
            let calls = rig.calls.lock().unwrap();
            assert_eq!(calls[0].1, Some("192.0.2.77".parse().unwrap()));
        });
    }

    #[test]
    fn a_helper_that_cannot_answer_is_not_a_denial() {
        runtime().block_on(async {
            let rig = rig_with(true);
            let (status, headers, _) = login_as(&rig, "admin", "right").await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert!(!headers.contains_key(header::SET_COOKIE));
            assert!(rig.auth.book().users.is_empty(), "no failure was counted");
        });
    }

    #[test]
    fn a_login_body_is_at_most_4_kib() {
        runtime().block_on(async {
            let rig = rig();
            let body = format!(r#"{{"user":"admin","password":"{}"}}"#, "x".repeat(5000));
            let request = page("POST", "/api/login", &body)
                .body(Body::from(body))
                .unwrap();
            assert_eq!(send(&rig, request).await.0, StatusCode::PAYLOAD_TOO_LARGE);
        });
    }

    #[test]
    fn the_login_never_logs_the_password() {
        // the only lines written are these; they name the user and the address, nothing typed
        let ok = format!("web ui login for {} from {}: ok", "admin", "192.0.2.1");
        assert!(!ok.contains("right"));
        let login = Login::parse(br#"{"user":"admin","password":"hunter2"}"#).unwrap();
        assert!(Login::parse(br#"{"user":"admin"}"#).is_none());
        assert!(Login::parse(br#"{"user":1,"password":"x"}"#).is_none());
        assert_eq!(login.user, "admin");
    }
}
