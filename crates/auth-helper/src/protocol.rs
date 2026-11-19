//! the wire format between the web ui and the helper: one request in, one line out.
//!
//! the request is `user NUL password NUL`, optionally followed by the client's ip address and a
//! NUL (handed to pam as the remote host), nothing before or after it, and the caller then shuts
//! down its writing side so the helper sees the end. the answer is one line:
//! `ok` (the password is right and the user is in the admins group), `ok user` (the password is
//! right, the user is an ordinary account), `ok setup` (the password is right, the user is root and
//! no other account is an administrator yet: first-run setup only), `denied` or `error <code>`, and
//! the exit status says the same (0, 0, 0, 1, 2).

use std::fmt;
use std::io::Read;
use std::net::IpAddr;

use zeroize::{Zeroize, Zeroizing};

/// the longest request accepted, in bytes. a login name is at most 32 bytes, so this leaves
/// plenty of room for a password.
pub const MAX_REQUEST: usize = 1024;

/// a login to check. the password is wiped from memory when the request is dropped.
pub struct Request {
    pub user: String,
    pub password: Zeroizing<Vec<u8>>,
    /// the web browser's address, when the web ui passes it on.
    pub rhost: Option<String>,
}

// the password never reaches a log, even through a debug print
impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("user", &self.user)
            .field("password", &"<redacted>")
            .field("rhost", &self.rhost)
            .finish()
    }
}

/// what the helper answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// the password is right and the user is in the admins group.
    Ok,
    /// the password is right and the user may log in, but is not in the admins group.
    User,
    /// the password is right and the user is root while the admins group has no other member:
    /// the web ui may offer first-run setup and nothing else.
    Setup,
    /// anything else about the login: an unknown user, a wrong password, an expired or locked
    /// account, or a system or service account. all of them look the same to the caller.
    Denied,
    /// the request or the helper itself is broken; the code says how, never anything about
    /// the user.
    Error(&'static str),
}

impl Verdict {
    /// the line written back to the caller, without the newline.
    pub fn line(&self) -> String {
        match self {
            Verdict::Ok => "ok".to_string(),
            Verdict::User => "ok user".to_string(),
            Verdict::Setup => "ok setup".to_string(),
            Verdict::Denied => "denied".to_string(),
            Verdict::Error(code) => format!("error {code}"),
        }
    }

    pub fn exit_code(&self) -> u8 {
        match self {
            Verdict::Ok | Verdict::User | Verdict::Setup => 0,
            Verdict::Denied => 1,
            Verdict::Error(_) => 2,
        }
    }
}

/// a unix login name lanlord accepts: a lowercase letter or underscore, then lowercase letters,
/// digits, underscores or hyphens, at most 32 in all. machine accounts (`name$`) are refused.
pub fn valid_user(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 32
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// read one request from `input`, up to its end. the bytes read are wiped before returning.
pub fn read_request(input: &mut impl Read) -> Result<Request, &'static str> {
    let mut buf = Zeroizing::new(Vec::with_capacity(MAX_REQUEST + 1));
    input
        .take(MAX_REQUEST as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|_| "read")?;
    if buf.len() > MAX_REQUEST {
        return Err("too-long");
    }
    parse(&mut buf)
}

/// split `user NUL password NUL [address NUL]` out of `buf`, then wipe `buf`, whatever the
/// outcome.
pub fn parse(buf: &mut [u8]) -> Result<Request, &'static str> {
    let parsed = split(buf);
    buf.zeroize();
    parsed
}

fn split(buf: &[u8]) -> Result<Request, &'static str> {
    let body = buf.strip_suffix(&[0]).ok_or("bad-request")?;
    // pam takes c strings, so no field can hold a nul: splitting on them is exact
    let fields: Vec<&[u8]> = body.split(|&b| b == 0).collect();
    let (user, password, rhost) = match fields[..] {
        [user, password] => (user, password, None),
        [user, password, rhost] => (user, password, Some(rhost)),
        _ => return Err("bad-request"),
    };
    // an empty password is never checked
    if password.is_empty() {
        return Err("bad-request");
    }
    let user = std::str::from_utf8(user).map_err(|_| "bad-request")?;
    if !valid_user(user) {
        return Err("bad-request");
    }
    let rhost = match rhost {
        None => None,
        Some(bytes) => {
            let text = std::str::from_utf8(bytes).map_err(|_| "bad-request")?;
            let address: IpAddr = text.parse().map_err(|_| "bad-request")?;
            Some(address.to_string())
        }
    };
    Ok(Request {
        user: user.to_string(),
        password: Zeroizing::new(password.to_vec()),
        rhost,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(bytes: &[u8]) -> Result<Request, &'static str> {
        read_request(&mut &bytes[..])
    }

    #[test]
    fn reads_a_user_and_a_password() {
        let r = request(b"alice\0correct horse\0").unwrap();
        assert_eq!(r.user, "alice");
        assert_eq!(&r.password[..], b"correct horse");
    }

    #[test]
    fn refuses_anything_but_exactly_two_fields() {
        for bad in [
            &b""[..],
            b"alice",
            b"alice\0",
            b"alice\0secret",
            b"alice\0\0",
            b"alice\0sec\0ret\0",
            b"\0secret\0",
        ] {
            assert_eq!(request(bad).unwrap_err(), "bad-request", "{bad:?}");
        }
    }

    #[test]
    fn refuses_names_that_are_not_plain_logins() {
        for name in [
            "Alice",
            "1alice",
            "al ice",
            "alice$",
            "../etc",
            "a;b",
            "-alice",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ] {
            let bytes = format!("{name}\0secret\0");
            assert_eq!(
                request(bytes.as_bytes()).unwrap_err(),
                "bad-request",
                "{name}"
            );
        }
        for name in ["alice", "_svc", "bob-2", "a_b"] {
            let bytes = format!("{name}\0secret\0");
            assert!(request(bytes.as_bytes()).is_ok(), "{name}");
        }
    }

    #[test]
    fn refuses_a_request_that_is_too_long() {
        let mut bytes = b"alice\0".to_vec();
        bytes.extend(std::iter::repeat_n(b'x', MAX_REQUEST));
        bytes.push(0);
        assert_eq!(request(&bytes).unwrap_err(), "too-long");
    }

    #[test]
    fn takes_the_client_address_when_given() {
        let r = request(b"alice\0secret\x00192.0.2.7\0").unwrap();
        assert_eq!(r.rhost.as_deref(), Some("192.0.2.7"));
        let r = request(b"alice\0secret\x002001:db8::1\0").unwrap();
        assert_eq!(r.rhost.as_deref(), Some("2001:db8::1"));
        assert_eq!(request(b"alice\0secret\0").unwrap().rhost, None);
        for bad in [
            &b"alice\0secret\0not-an-ip\0"[..],
            b"alice\0secret\0\0",
            b"alice\0secret\x00192.0.2.7\0extra\0",
            b"alice\0secret\0\xff\0",
        ] {
            assert_eq!(request(bad).unwrap_err(), "bad-request", "{bad:?}");
        }
    }

    #[test]
    fn a_failed_read_is_an_error() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("gone"))
            }
        }
        assert_eq!(read_request(&mut Broken).unwrap_err(), "read");
    }

    #[test]
    fn a_name_must_be_utf8() {
        assert_eq!(request(b"al\xffice\0secret\0").unwrap_err(), "bad-request");
    }

    #[test]
    fn the_bytes_read_are_wiped_whatever_the_outcome() {
        let mut good = b"alice\0secret\0".to_vec();
        assert!(parse(&mut good).is_ok());
        assert!(good.iter().all(|&b| b == 0));
        let mut bad = b"alice\0sec\0ret\0".to_vec();
        assert!(parse(&mut bad).is_err());
        assert!(bad.iter().all(|&b| b == 0));
    }

    #[test]
    fn the_password_never_shows_in_a_debug_print() {
        let r = request(b"alice\0hunter2\0").unwrap();
        let printed = format!("{r:?}");
        assert!(printed.contains("alice"));
        assert!(!printed.contains("hunter2"));
    }

    #[test]
    fn verdicts_have_a_line_and_an_exit_status() {
        assert_eq!(
            (Verdict::Ok.line(), Verdict::Ok.exit_code()),
            ("ok".into(), 0)
        );
        assert_eq!(
            (Verdict::User.line(), Verdict::User.exit_code()),
            ("ok user".into(), 0)
        );
        assert_eq!(
            (Verdict::Setup.line(), Verdict::Setup.exit_code()),
            ("ok setup".into(), 0)
        );
        assert_eq!(
            (Verdict::Denied.line(), Verdict::Denied.exit_code()),
            ("denied".into(), 1)
        );
        let e = Verdict::Error("pam");
        assert_eq!((e.line(), e.exit_code()), ("error pam".into(), 2));
    }
}
