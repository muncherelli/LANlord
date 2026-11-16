//! the decision itself, with pam and the group database behind traits so it can be tested
//! without either.

use std::time::Duration;

use crate::protocol::{Request, Verdict};

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

    fn alice() -> Request {
        parse(&mut b"alice\0secret\0".to_vec()).unwrap()
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
}
