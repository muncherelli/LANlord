//! the decision itself and everything around it, with pam, the group database and the system
//! calls behind traits, so all of it can be tested without root, pam or a socket.

use std::io::Read;
use std::time::Duration;

use crate::protocol::{read_request, valid_user, Request, Verdict};

/// the gid of the `admins` group, whose members may administer lanlord. it is fixed by
/// lanlord's id scheme (built-in groups live in 9000-9998), the same on every gateway.
pub const ADMINS_GID: u32 = 9998;

/// every answer but `ok` takes at least this long, counted from when the request arrived, so
/// guessing passwords is slow and an unknown user cannot be told from a wrong password.
pub const FAIL_DELAY: Duration = Duration::from_millis(1500);

/// checks a password, and that the account may log in now (not expired or locked).
pub trait Pam {
    /// `Ok(true)` when both pass, `Ok(false)` when either fails for any reason to do with the
    /// user, and `Err` when pam itself could not run.
    fn check(&self, user: &str, password: &[u8]) -> Result<bool, &'static str>;
}

/// the group database.
pub trait Groups {
    fn member(&self, user: &str, gid: u32) -> Result<bool, &'static str>;
}

/// what the helper asks of the system about itself and its caller.
pub trait System {
    /// whether the helper runs as root, which pam needs to check other users' passwords.
    fn is_root(&self) -> bool;
    /// the uid of the process on the other end of stdin when stdin is a socket, `None` when it
    /// is not (run by hand from a terminal or a pipe).
    fn peer_uid(&self) -> Result<Option<u32>, &'static str>;
    /// the uid of the account `user`, `None` when there is no such account.
    fn uid_of(&self, user: &str) -> Result<Option<u32>, &'static str>;
}

/// the command line: nothing, or `--caller <account>`, the only account allowed on the other
/// end of the socket.
pub fn caller(args: &[String]) -> Result<Option<String>, &'static str> {
    match args {
        [] => Ok(None),
        [flag, name] if flag == "--caller" && valid_user(name) => Ok(Some(name.clone())),
        _ => Err("usage"),
    }
}

/// over a socket, the peer must be `caller`; by hand there is no peer to check.
pub fn check_caller(caller: Option<&str>, system: &impl System) -> Result<(), &'static str> {
    let Some(peer) = system.peer_uid()? else {
        return Ok(());
    };
    let caller = caller.ok_or("usage")?;
    match system.uid_of(caller)? {
        Some(uid) if uid == peer => Ok(()),
        _ => Err("caller"),
    }
}

/// pam first, always, so an unknown user costs the same as a known one; the group only after
/// the password is right, so a wrong password says nothing about membership.
pub fn decide(request: &Request, pam: &impl Pam, groups: &impl Groups) -> Verdict {
    match pam.check(&request.user, &request.password) {
        Err(code) => Verdict::Error(code),
        Ok(false) => Verdict::Denied,
        Ok(true) => match groups.member(&request.user, ADMINS_GID) {
            Ok(true) => Verdict::Ok,
            Ok(false) => Verdict::Denied,
            Err(code) => Verdict::Error(code),
        },
    }
}

/// everything between starting and answering: who may ask, what was asked, and the verdict.
/// the journal (stderr) gets who and how, never the password.
pub fn answer(
    args: &[String],
    system: &impl System,
    input: &mut impl Read,
    pam: &impl Pam,
    groups: &impl Groups,
) -> Verdict {
    if !system.is_root() {
        return Verdict::Error("not-root");
    }
    let caller = match caller(args) {
        Ok(caller) => caller,
        Err(code) => return Verdict::Error(code),
    };
    if let Err(code) = check_caller(caller.as_deref(), system) {
        return Verdict::Error(code);
    }
    let request = match read_request(input) {
        Ok(request) => request,
        Err(code) => return Verdict::Error(code),
    };
    let verdict = decide(&request, pam, groups);
    eprintln!("web ui login for {}: {}", request.user, verdict.line());
    verdict
}

/// how much longer to wait before answering, `elapsed` after the request arrived.
pub fn padding(verdict: Verdict, elapsed: Duration) -> Duration {
    if verdict == Verdict::Ok {
        Duration::ZERO
    } else {
        FAIL_DELAY.saturating_sub(elapsed)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::protocol::parse;

    struct FakePam(Result<bool, &'static str>);
    impl Pam for FakePam {
        fn check(&self, _: &str, _: &[u8]) -> Result<bool, &'static str> {
            self.0
        }
    }

    struct FakeGroups {
        answer: Result<bool, &'static str>,
        asked: Cell<Option<u32>>,
    }
    impl FakeGroups {
        fn new(answer: Result<bool, &'static str>) -> Self {
            Self {
                answer,
                asked: Cell::new(None),
            }
        }
    }
    impl Groups for FakeGroups {
        fn member(&self, _: &str, gid: u32) -> Result<bool, &'static str> {
            self.asked.set(Some(gid));
            self.answer
        }
    }

    /// root, with the web ui account `lanlord` (uid 9999) on the other end unless told otherwise.
    struct FakeSystem {
        root: bool,
        peer: Result<Option<u32>, &'static str>,
        uid: Result<Option<u32>, &'static str>,
    }
    impl Default for FakeSystem {
        fn default() -> Self {
            Self {
                root: true,
                peer: Ok(Some(9999)),
                uid: Ok(Some(9999)),
            }
        }
    }
    impl System for FakeSystem {
        fn is_root(&self) -> bool {
            self.root
        }
        fn peer_uid(&self) -> Result<Option<u32>, &'static str> {
            self.peer
        }
        fn uid_of(&self, _: &str) -> Result<Option<u32>, &'static str> {
            self.uid
        }
    }

    fn alice() -> Request {
        parse(&mut b"alice\0secret\0".to_vec()).unwrap()
    }

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn run(system: &FakeSystem, args: &[String], input: &[u8], pam: bool, admin: bool) -> Verdict {
        answer(
            args,
            system,
            &mut &input[..],
            &FakePam(Ok(pam)),
            &FakeGroups::new(Ok(admin)),
        )
    }

    #[test]
    fn a_right_password_and_admins_membership_is_ok() {
        let groups = FakeGroups::new(Ok(true));
        assert_eq!(decide(&alice(), &FakePam(Ok(true)), &groups), Verdict::Ok);
        assert_eq!(groups.asked.get(), Some(ADMINS_GID));
    }

    #[test]
    fn a_right_password_outside_admins_is_denied() {
        let groups = FakeGroups::new(Ok(false));
        assert_eq!(
            decide(&alice(), &FakePam(Ok(true)), &groups),
            Verdict::Denied
        );
    }

    #[test]
    fn a_wrong_password_is_denied_without_looking_at_groups() {
        let groups = FakeGroups::new(Ok(true));
        assert_eq!(
            decide(&alice(), &FakePam(Ok(false)), &groups),
            Verdict::Denied
        );
        assert_eq!(groups.asked.get(), None);
    }

    #[test]
    fn broken_pam_or_groups_are_errors_not_denials() {
        let groups = FakeGroups::new(Ok(true));
        assert_eq!(
            decide(&alice(), &FakePam(Err("pam")), &groups),
            Verdict::Error("pam")
        );
        let groups = FakeGroups::new(Err("groups"));
        assert_eq!(
            decide(&alice(), &FakePam(Ok(true)), &groups),
            Verdict::Error("groups")
        );
    }

    #[test]
    fn the_command_line_takes_only_a_valid_caller() {
        assert_eq!(caller(&args(&[])), Ok(None));
        assert_eq!(
            caller(&args(&["--caller", "lanlord"])),
            Ok(Some("lanlord".into()))
        );
        assert_eq!(caller(&args(&["--caller", "Bad Name"])), Err("usage"));
        assert_eq!(caller(&args(&["--caller"])), Err("usage"));
        assert_eq!(caller(&args(&["--other", "x"])), Err("usage"));
        assert_eq!(caller(&args(&["--caller", "a", "b"])), Err("usage"));
    }

    #[test]
    fn over_a_socket_only_the_caller_may_ask() {
        let system = FakeSystem::default();
        assert_eq!(check_caller(Some("lanlord"), &system), Ok(()));
        let stranger = FakeSystem {
            peer: Ok(Some(1000)),
            ..FakeSystem::default()
        };
        assert_eq!(check_caller(Some("lanlord"), &stranger), Err("caller"));
        let no_account = FakeSystem {
            uid: Ok(None),
            ..FakeSystem::default()
        };
        assert_eq!(check_caller(Some("lanlord"), &no_account), Err("caller"));
        assert_eq!(check_caller(None, &system), Err("usage"));
    }

    #[test]
    fn by_hand_there_is_no_peer_to_check() {
        let by_hand = FakeSystem {
            peer: Ok(None),
            ..FakeSystem::default()
        };
        assert_eq!(check_caller(None, &by_hand), Ok(()));
    }

    #[test]
    fn system_lookups_that_fail_are_errors() {
        let broken_peer = FakeSystem {
            peer: Err("peer"),
            ..FakeSystem::default()
        };
        assert_eq!(check_caller(Some("lanlord"), &broken_peer), Err("peer"));
        let broken_passwd = FakeSystem {
            uid: Err("passwd"),
            ..FakeSystem::default()
        };
        assert_eq!(check_caller(Some("lanlord"), &broken_passwd), Err("passwd"));
    }

    #[test]
    fn answers_a_login_end_to_end() {
        let system = FakeSystem::default();
        let caller = args(&["--caller", "lanlord"]);
        let request = b"alice\0secret\0";
        assert_eq!(run(&system, &caller, request, true, true), Verdict::Ok);
        assert_eq!(run(&system, &caller, request, false, true), Verdict::Denied);
        assert_eq!(run(&system, &caller, request, true, false), Verdict::Denied);
    }

    #[test]
    fn refuses_before_reading_when_it_should_not_answer() {
        let caller = args(&["--caller", "lanlord"]);
        let request = b"alice\0secret\0";
        let not_root = FakeSystem {
            root: false,
            ..FakeSystem::default()
        };
        assert_eq!(
            run(&not_root, &caller, request, true, true),
            Verdict::Error("not-root")
        );
        let system = FakeSystem::default();
        assert_eq!(
            run(&system, &args(&["--nope"]), request, true, true),
            Verdict::Error("usage")
        );
        let stranger = FakeSystem {
            peer: Ok(Some(1000)),
            ..FakeSystem::default()
        };
        assert_eq!(
            run(&stranger, &caller, request, true, true),
            Verdict::Error("caller")
        );
        assert_eq!(
            run(&system, &caller, b"alice\0", true, true),
            Verdict::Error("bad-request")
        );
    }

    #[test]
    fn only_ok_answers_at_once() {
        let quick = Duration::from_millis(200);
        assert_eq!(padding(Verdict::Ok, quick), Duration::ZERO);
        assert_eq!(padding(Verdict::Denied, quick), FAIL_DELAY - quick);
        assert_eq!(padding(Verdict::Error("pam"), quick), FAIL_DELAY - quick);
        assert_eq!(
            padding(Verdict::Denied, FAIL_DELAY * 2),
            Duration::ZERO,
            "a slow pam is not made slower"
        );
    }
}
