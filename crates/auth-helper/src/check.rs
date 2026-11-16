//! the decision itself and everything around it, with pam, the group database and the system
//! calls behind traits, so all of it can be tested without root, pam or a socket.

use std::io::{Read, Write};
use std::time::Duration;

use crate::protocol::{read_request, valid_user, Request, Verdict};

/// the gid of the `admins` group, whose members may administer lanlord. it is fixed by
/// lanlord's id scheme (built-in groups live in 9000-9998), the same on every gateway.
pub const ADMINS_GID: u32 = 9998;

/// its name, for the journal.
pub const ADMINS_GROUP: &str = "admins";

/// every answer but `ok` takes this long (plus up to `JITTER`), counted from when the helper
/// started. it is the only delay: libpam's own failure delay is switched off, because it only
/// applies to a wrong password and would tell that apart from a right password for an account
/// that may not log in or is not an admin.
pub const FAIL_DELAY: Duration = Duration::from_secs(2);

/// the most random time added to `FAIL_DELAY`.
pub const JITTER: Duration = Duration::from_millis(250);

/// checks a password, and that the account may log in now (not expired or locked).
pub trait Pam {
    /// `Ok(true)` when both pass, `Ok(false)` when either fails for any reason to do with the
    /// user, and `Err` when pam itself could not run. `rhost` is the client's address, if known.
    fn check(&self, user: &str, password: &[u8], rhost: Option<&str>)
        -> Result<bool, &'static str>;
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
    /// limit how long a read from or write to the caller may block, so a caller that stalls
    /// cannot hold a helper (and a connection slot) open.
    fn set_timeouts(&self) -> Result<(), &'static str>;
    /// the uid of the account `user`, `None` when there is no such account.
    fn uid_of(&self, user: &str) -> Result<Option<u32>, &'static str>;
}

/// how a login turned out, in more detail than the caller is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Ok,
    /// pam said no: wrong password, unknown user, expired or locked account.
    Refused,
    /// the password is right, but the user is not in the admins group.
    NotAdmin,
    Error(&'static str),
}

impl Decision {
    /// what the caller hears: `Refused` and `NotAdmin` are the same to it.
    pub fn verdict(self) -> Verdict {
        match self {
            Decision::Ok => Verdict::Ok,
            Decision::Refused | Decision::NotAdmin => Verdict::Denied,
            Decision::Error(code) => Verdict::Error(code),
        }
    }
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

/// pam first, always, so an unknown user costs the same as a known one; the group only after
/// the password is right, so a wrong password says nothing about membership.
pub fn decide(request: &Request, pam: &impl Pam, groups: &impl Groups) -> Decision {
    match pam.check(&request.user, &request.password, request.rhost.as_deref()) {
        Err(code) => Decision::Error(code),
        Ok(false) => Decision::Refused,
        Ok(true) => match groups.member(&request.user, ADMINS_GID) {
            Ok(true) => Decision::Ok,
            Ok(false) => Decision::NotAdmin,
            Err(code) => Decision::Error(code),
        },
    }
}

/// everything between starting and answering: who may ask, what was asked, and the verdict.
/// `log` (the journal) gets who and how; never a password, and never the name typed for an
/// account that does not exist (it may be a password typed into the wrong field).
pub fn answer(
    args: &[String],
    system: &impl System,
    input: &mut impl Read,
    pam: &impl Pam,
    groups: &impl Groups,
    log: &mut impl Write,
) -> Verdict {
    let mut say = |line: String| {
        let _ = writeln!(log, "{line}");
    };
    if !system.is_root() {
        say("refused: the helper is not running as root".into());
        return Verdict::Error("not-root");
    }
    let caller = match caller(args) {
        Ok(caller) => caller,
        Err(code) => {
            say("refused: the command line is not --caller <account>".into());
            return Verdict::Error(code);
        }
    };
    let peer = match system.peer_uid() {
        Ok(peer) => peer,
        Err(code) => {
            say(format!("refused: cannot tell who is asking ({code})"));
            return Verdict::Error(code);
        }
    };
    if let Some(uid) = peer {
        if let Err(code) = system.set_timeouts() {
            say(format!("refused uid {uid}: cannot set socket timeouts"));
            return Verdict::Error(code);
        }
        let Some(caller) = caller else {
            say(format!(
                "refused uid {uid}: no --caller account to compare with"
            ));
            return Verdict::Error("usage");
        };
        match system.uid_of(&caller) {
            Ok(Some(allowed)) if allowed == uid => {}
            Ok(_) => {
                say(format!("refused uid {uid}: only {caller} may ask"));
                return Verdict::Error("caller");
            }
            Err(code) => {
                say(format!(
                    "refused uid {uid}: cannot look up {caller} ({code})"
                ));
                return Verdict::Error(code);
            }
        }
    }
    let from = peer.map_or_else(|| "a terminal".to_string(), |uid| format!("uid {uid}"));
    let request = match read_request(input) {
        Ok(request) => request,
        Err(code) => {
            say(format!("bad request from {from}: {code}"));
            return Verdict::Error(code);
        }
    };
    let decision = decide(&request, pam, groups);
    let user = match decision {
        Decision::Refused | Decision::Error(_)
            if !matches!(system.uid_of(&request.user), Ok(Some(_))) =>
        {
            "an unknown user".to_string()
        }
        _ => request.user.clone(),
    };
    let client = request
        .rhost
        .as_deref()
        .map_or_else(String::new, |rhost| format!(" from {rhost}"));
    say(match decision {
        Decision::Ok => format!("web ui login for {user}{client}: ok"),
        Decision::Refused => format!("web ui login for {user}{client}: denied"),
        Decision::NotAdmin => format!(
            "web ui login for {user}{client}: denied, right password but not in group \
             {ADMINS_GROUP} (gid {ADMINS_GID})"
        ),
        Decision::Error(code) => format!("web ui login for {user}{client}: error {code}"),
    });
    decision.verdict()
}

/// how much longer to wait before answering `verdict`, `elapsed` after the helper started, with
/// `jitter` (at most `JITTER`) added to every answer but `ok`.
pub fn padding(verdict: Verdict, elapsed: Duration, jitter: Duration) -> Duration {
    if verdict == Verdict::Ok {
        Duration::ZERO
    } else {
        (FAIL_DELAY + jitter.min(JITTER)).saturating_sub(elapsed)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::protocol::parse;

    struct FakePam(Result<bool, &'static str>);
    impl Pam for FakePam {
        fn check(&self, _: &str, _: &[u8], _: Option<&str>) -> Result<bool, &'static str> {
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

    /// root, with the web ui account `lanlord` (uid 9999) on the other end unless told otherwise;
    /// `alice` exists, nobody else does.
    struct FakeSystem {
        root: bool,
        peer: Result<Option<u32>, &'static str>,
        timeouts: Result<(), &'static str>,
        timeouts_set: Cell<bool>,
        lanlord: Result<Option<u32>, &'static str>,
    }
    impl Default for FakeSystem {
        fn default() -> Self {
            Self {
                root: true,
                peer: Ok(Some(9999)),
                timeouts: Ok(()),
                timeouts_set: Cell::new(false),
                lanlord: Ok(Some(9999)),
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
        fn set_timeouts(&self) -> Result<(), &'static str> {
            self.timeouts_set.set(true);
            self.timeouts
        }
        fn uid_of(&self, user: &str) -> Result<Option<u32>, &'static str> {
            match user {
                "lanlord" => self.lanlord,
                "alice" => Ok(Some(10000)),
                _ => Ok(None),
            }
        }
    }

    fn alice() -> Request {
        parse(&mut b"alice\0secret\0".to_vec()).unwrap()
    }

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    /// answer `input` as the web ui would send it, returning the verdict and the journal line.
    fn run(
        system: &FakeSystem,
        args: &[String],
        input: &[u8],
        pam: Result<bool, &'static str>,
        admin: bool,
    ) -> (Verdict, String) {
        let mut log = Vec::new();
        let verdict = answer(
            args,
            system,
            &mut &input[..],
            &FakePam(pam),
            &FakeGroups::new(Ok(admin)),
            &mut log,
        );
        (verdict, String::from_utf8(log).unwrap())
    }

    fn lanlord() -> Vec<String> {
        args(&["--caller", "lanlord"])
    }

    #[test]
    fn a_right_password_and_admins_membership_is_ok() {
        let groups = FakeGroups::new(Ok(true));
        assert_eq!(decide(&alice(), &FakePam(Ok(true)), &groups), Decision::Ok);
        assert_eq!(groups.asked.get(), Some(ADMINS_GID));
    }

    #[test]
    fn a_right_password_outside_admins_is_not_admin() {
        let groups = FakeGroups::new(Ok(false));
        assert_eq!(
            decide(&alice(), &FakePam(Ok(true)), &groups),
            Decision::NotAdmin
        );
    }

    #[test]
    fn a_wrong_password_is_refused_without_looking_at_groups() {
        let groups = FakeGroups::new(Ok(true));
        assert_eq!(
            decide(&alice(), &FakePam(Ok(false)), &groups),
            Decision::Refused
        );
        assert_eq!(groups.asked.get(), None);
    }

    #[test]
    fn broken_pam_or_groups_are_errors_not_denials() {
        let groups = FakeGroups::new(Ok(true));
        assert_eq!(
            decide(&alice(), &FakePam(Err("pam")), &groups),
            Decision::Error("pam")
        );
        let groups = FakeGroups::new(Err("groups"));
        assert_eq!(
            decide(&alice(), &FakePam(Ok(true)), &groups),
            Decision::Error("groups")
        );
    }

    #[test]
    fn the_caller_cannot_tell_a_wrong_password_from_a_non_admin() {
        assert_eq!(Decision::Refused.verdict(), Verdict::Denied);
        assert_eq!(Decision::NotAdmin.verdict(), Verdict::Denied);
        assert_eq!(Decision::Ok.verdict(), Verdict::Ok);
        assert_eq!(Decision::Error("pam").verdict(), Verdict::Error("pam"));
    }

    #[test]
    fn the_command_line_takes_only_a_valid_caller() {
        assert_eq!(caller(&args(&[])), Ok(None));
        assert_eq!(caller(&lanlord()), Ok(Some("lanlord".into())));
        assert_eq!(caller(&args(&["--caller", "Bad Name"])), Err("usage"));
        assert_eq!(caller(&args(&["--caller"])), Err("usage"));
        assert_eq!(caller(&args(&["--other", "x"])), Err("usage"));
        assert_eq!(caller(&args(&["--caller", "a", "b"])), Err("usage"));
    }

    #[test]
    fn answers_a_login_end_to_end_and_logs_how() {
        let system = FakeSystem::default();
        let request = b"alice\0secret\x00192.0.2.7\0";
        let (verdict, log) = run(&system, &lanlord(), request, Ok(true), true);
        assert_eq!(verdict, Verdict::Ok);
        assert_eq!(log, "web ui login for alice from 192.0.2.7: ok\n");
        assert!(system.timeouts_set.get(), "a socket gets timeouts");

        let (verdict, log) = run(&system, &lanlord(), request, Ok(false), true);
        assert_eq!(verdict, Verdict::Denied);
        assert_eq!(log, "web ui login for alice from 192.0.2.7: denied\n");

        let (verdict, log) = run(&system, &lanlord(), request, Ok(true), false);
        assert_eq!(verdict, Verdict::Denied);
        assert_eq!(
            log,
            "web ui login for alice from 192.0.2.7: denied, right password but not in group \
             admins (gid 9998)\n"
        );

        let (verdict, log) = run(&system, &lanlord(), b"alice\0secret\0", Err("pam"), true);
        assert_eq!(verdict, Verdict::Error("pam"));
        assert_eq!(log, "web ui login for alice: error pam\n");
    }

    #[test]
    fn never_logs_a_password_or_the_name_typed_for_an_unknown_account() {
        let system = FakeSystem::default();
        for pam in [Ok(false), Err("pam")] {
            let (_, log) = run(&system, &lanlord(), b"hunter_two\0hunter2\0", pam, true);
            assert!(log.contains("an unknown user"), "{log}");
            assert!(!log.contains("hunter"), "{log}");
        }
        let (_, log) = run(&system, &lanlord(), b"alice\0hunter2\0", Ok(false), true);
        assert!(!log.contains("hunter2"), "{log}");
    }

    #[test]
    fn refuses_callers_other_than_the_web_ui_and_says_who() {
        let stranger = FakeSystem {
            peer: Ok(Some(1000)),
            ..FakeSystem::default()
        };
        let (verdict, log) = run(&stranger, &lanlord(), b"alice\0secret\0", Ok(true), true);
        assert_eq!(verdict, Verdict::Error("caller"));
        assert_eq!(log, "refused uid 1000: only lanlord may ask\n");

        let no_account = FakeSystem {
            lanlord: Ok(None),
            ..FakeSystem::default()
        };
        let (verdict, _) = run(&no_account, &lanlord(), b"alice\0secret\0", Ok(true), true);
        assert_eq!(verdict, Verdict::Error("caller"));

        let system = FakeSystem::default();
        let (verdict, log) = run(&system, &[], b"alice\0secret\0", Ok(true), true);
        assert_eq!(verdict, Verdict::Error("usage"));
        assert_eq!(
            log,
            "refused uid 9999: no --caller account to compare with\n"
        );
    }

    #[test]
    fn by_hand_there_is_no_peer_and_no_timeouts() {
        let by_hand = FakeSystem {
            peer: Ok(None),
            ..FakeSystem::default()
        };
        let (verdict, _) = run(&by_hand, &[], b"alice\0secret\0", Ok(true), true);
        assert_eq!(verdict, Verdict::Ok);
        assert!(!by_hand.timeouts_set.get());
    }

    #[test]
    fn system_failures_are_errors_and_are_logged() {
        let cases = [
            (
                FakeSystem {
                    root: false,
                    ..FakeSystem::default()
                },
                "not-root",
                "refused: the helper is not running as root\n",
            ),
            (
                FakeSystem {
                    peer: Err("peer"),
                    ..FakeSystem::default()
                },
                "peer",
                "refused: cannot tell who is asking (peer)\n",
            ),
            (
                FakeSystem {
                    timeouts: Err("timeouts"),
                    ..FakeSystem::default()
                },
                "timeouts",
                "refused uid 9999: cannot set socket timeouts\n",
            ),
            (
                FakeSystem {
                    lanlord: Err("passwd"),
                    ..FakeSystem::default()
                },
                "passwd",
                "refused uid 9999: cannot look up lanlord (passwd)\n",
            ),
        ];
        for (system, code, line) in cases {
            let (verdict, log) = run(&system, &lanlord(), b"alice\0secret\0", Ok(true), true);
            assert_eq!(verdict, Verdict::Error(code));
            assert_eq!(log, line);
        }
        let (verdict, log) = run(
            &FakeSystem::default(),
            &args(&["--nope"]),
            b"",
            Ok(true),
            true,
        );
        assert_eq!(verdict, Verdict::Error("usage"));
        assert_eq!(log, "refused: the command line is not --caller <account>\n");
    }

    #[test]
    fn a_bad_request_is_logged_with_the_caller_and_without_its_content() {
        let system = FakeSystem::default();
        let (verdict, log) = run(&system, &lanlord(), b"alice\0hunter2", Ok(true), true);
        assert_eq!(verdict, Verdict::Error("bad-request"));
        assert_eq!(log, "bad request from uid 9999: bad-request\n");
        let by_hand = FakeSystem {
            peer: Ok(None),
            ..FakeSystem::default()
        };
        let (_, log) = run(&by_hand, &[], b"", Ok(true), true);
        assert_eq!(log, "bad request from a terminal: bad-request\n");
    }

    #[test]
    fn every_answer_but_ok_waits_the_same() {
        let elapsed = Duration::from_millis(300);
        let jitter = Duration::from_millis(100);
        let waits: Vec<Duration> = [
            Decision::Refused,
            Decision::NotAdmin,
            Decision::Error("pam"),
            Decision::Error("bad-request"),
        ]
        .iter()
        .map(|d| padding(d.verdict(), elapsed, jitter))
        .collect();
        assert!(
            waits.iter().all(|&w| w == FAIL_DELAY + jitter - elapsed),
            "{waits:?}"
        );
        assert_eq!(padding(Verdict::Ok, elapsed, jitter), Duration::ZERO);
    }

    #[test]
    fn jitter_is_capped_and_a_slow_answer_is_not_made_slower() {
        let none = Duration::ZERO;
        assert_eq!(
            padding(Verdict::Denied, none, Duration::from_secs(60)),
            FAIL_DELAY + JITTER
        );
        assert_eq!(
            padding(Verdict::Denied, FAIL_DELAY * 2, none),
            Duration::ZERO
        );
    }
}
