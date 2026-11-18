//! a person's linked accounts and changing the password on all of them or none.
//!
//! passwords never go in an argument or a log: the linux link sends them base64 encoded inside
//! the script that reaches `sh -s` on stdin, and the secret type keeps them out of debug output.

use std::cell::RefCell;

use anyhow::{bail, Result};
use base64::Engine;

use lanlord_model::identity::{LinkInfo, LinkReport, LinkState, PasswordReport};
use lanlord_model::secret::Secret;

use crate::bootstrap::{capture_as_root, valid_user, AskPassword};
use crate::ssh::Session;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// the longest password accepted.
const MAX_PASSWORD: usize = 128;

/// what a link did for a password change.
pub enum Applied {
    /// it is changed; the secret puts the old state back.
    Changed(Secret),
    /// nothing was done, and why.
    Skipped(String),
}

/// one account a person has besides their lanlord login.
pub trait IdentityLink {
    /// what to call it, such as "Linux account".
    fn name(&self) -> String;
    /// a short status for the account window.
    fn status(&self) -> String;
    fn set_password(&self, user: &str, new: &Secret) -> Result<Applied>;
    /// put back what `set_password` changed.
    fn roll_back(&self, user: &str, undo: &Secret) -> Result<()>;
}

/// what is wrong with a new password, checked once for every link.
pub fn check_policy(old: Option<&Secret>, new: &Secret) -> Result<()> {
    let new = new.expose();
    if new.is_empty() {
        bail!("the new password is empty");
    }
    if new.len() > MAX_PASSWORD {
        bail!("the new password is longer than {MAX_PASSWORD} characters");
    }
    if new.chars().any(|c| matches!(c, '\n' | '\r' | '\0')) {
        bail!("the password cannot contain line breaks");
    }
    if old.is_some_and(|old| old.expose() == new) {
        bail!("the new password is the same as the old one");
    }
    Ok(())
}

fn put_back(
    links: &[Box<dyn IdentityLink + '_>],
    user: &str,
    done: Vec<(usize, Secret)>,
    reports: &mut [LinkReport],
) {
    for (i, undo) in done.into_iter().rev() {
        reports[i].state = match links[i].roll_back(user, &undo) {
            Ok(()) => LinkState::RolledBack,
            Err(e) => LinkState::RollbackFailed(format!("{e:#}")),
        };
    }
}

/// set the password on every link; when one fails, put back the ones already changed. the
/// report says what happened to each.
pub fn change_all(
    links: &[Box<dyn IdentityLink + '_>],
    user: &str,
    new: &Secret,
) -> PasswordReport {
    let mut reports: Vec<LinkReport> = links
        .iter()
        .map(|l| LinkReport {
            link: l.name(),
            state: LinkState::NotTried,
        })
        .collect();
    let mut done = Vec::new();
    for (i, link) in links.iter().enumerate() {
        match link.set_password(user, new) {
            Ok(Applied::Changed(undo)) => {
                reports[i].state = LinkState::Changed;
                done.push((i, undo));
            }
            Ok(Applied::Skipped(why)) => reports[i].state = LinkState::Skipped(why),
            Err(e) => {
                reports[i].state = LinkState::Failed(format!("{e:#}"));
                put_back(links, user, done, &mut reports);
                return PasswordReport {
                    ok: false,
                    links: reports,
                };
            }
        }
    }
    PasswordReport {
        ok: true,
        links: reports,
    }
}

pub fn infos(links: &[Box<dyn IdentityLink + '_>]) -> Vec<LinkInfo> {
    links
        .iter()
        .map(|l| LinkInfo {
            link: l.name(),
            status: l.status(),
        })
        .collect()
}

/// the linux (pam) account lanlord logs people in with.
pub struct PamLink<'a> {
    session: &'a Session,
    admin: String,
    ask: RefCell<AskPassword<'a>>,
}

impl<'a> PamLink<'a> {
    pub fn new(session: &'a Session, admin: &str, ask: AskPassword<'a>) -> Self {
        Self {
            session,
            admin: admin.into(),
            ask: RefCell::new(ask),
        }
    }

    fn root(&self, script: String) -> Result<String> {
        let mut ask = self.ask.borrow_mut();
        capture_as_root(self.session, &self.admin, script, &mut **ask)
    }
}

fn set_script(user: &str, new: &Secret) -> String {
    let pw = B64.encode(new.expose());
    format!(
        "set -eu\nprintf 'OLD:%s\\n' \"$(getent shadow {user} | cut -d: -f2)\"\n\
         pw=$(printf %s '{pw}' | base64 -d)\nprintf '%s:%s\\n' {user} \"$pw\" | chpasswd\nunset pw\n"
    )
}

fn restore_script(user: &str, hash: &Secret) -> String {
    let hash = B64.encode(hash.expose());
    format!(
        "set -eu\nh=$(printf %s '{hash}' | base64 -d)\nprintf '%s:%s\\n' {user} \"$h\" | chpasswd -e\n"
    )
}

impl IdentityLink for PamLink<'_> {
    fn name(&self) -> String {
        "Linux account".into()
    }

    fn status(&self) -> String {
        "active".into()
    }

    fn set_password(&self, user: &str, new: &Secret) -> Result<Applied> {
        if !valid_user(user) {
            bail!("not a valid user name: {user:?}");
        }
        let out = self.root(set_script(user, new))?;
        let hash = out
            .lines()
            .find_map(|l| l.strip_prefix("OLD:"))
            .unwrap_or("");
        Ok(Applied::Changed(Secret::new(hash)))
    }

    fn roll_back(&self, user: &str, undo: &Secret) -> Result<()> {
        if undo.expose().is_empty() {
            bail!("the old password hash is not known");
        }
        self.root(restore_script(user, undo)).map(drop)
    }
}

/// the nt4 domain account. todo: set the password inside the samba container once the domain
/// component's image ships (it is in another change); until then it reports that it did nothing.
/// it is only linked when the domain component is installed.
pub struct Nt4Link;

impl IdentityLink for Nt4Link {
    fn name(&self) -> String {
        "NT4 domain account".into()
    }

    fn status(&self) -> String {
        "not available yet".into()
    }

    fn set_password(&self, _user: &str, _new: &Secret) -> Result<Applied> {
        Ok(Applied::Skipped("not available yet".into()))
    }

    fn roll_back(&self, _user: &str, _undo: &Secret) -> Result<()> {
        Ok(())
    }
}

/// the links a person has: always the linux account, and the domain one when the component is
/// installed.
pub fn links_for<'a>(
    session: &'a Session,
    admin: &str,
    domain_installed: bool,
    ask: AskPassword<'a>,
) -> Vec<Box<dyn IdentityLink + 'a>> {
    let mut links: Vec<Box<dyn IdentityLink + 'a>> =
        vec![Box::new(PamLink::new(session, admin, ask))];
    if domain_installed {
        links.push(Box::new(Nt4Link));
    }
    links
}

/// where the helper that checks a password through pam is installed.
const AUTH_HELPER: &str = "/usr/local/libexec/lanlord-auth-helper";

/// is `password` the current password of `user`? asked of the same helper the web logon uses,
/// with the request on stdin.
pub fn verify_password(
    session: &Session,
    admin: &str,
    user: &str,
    password: &Secret,
    ask: AskPassword,
) -> Result<bool> {
    if !valid_user(user) {
        return Ok(false);
    }
    let (u, p) = (B64.encode(user), B64.encode(password.expose()));
    let script = format!(
        "set -u\nprintf '%s\\0%s\\0' \"$(printf %s '{u}' | base64 -d)\" \"$(printf %s '{p}' | base64 -d)\" | {AUTH_HELPER} || true\n"
    );
    let out = capture_as_root(session, admin, script, ask)?;
    Ok(out.lines().any(|l| l == "ok" || l == "ok user"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    struct Fake {
        name: &'static str,
        fail: bool,
        skip: bool,
        fail_undo: bool,
        undone: Rc<Cell<bool>>,
    }

    fn fake(name: &'static str) -> Fake {
        Fake {
            name,
            fail: false,
            skip: false,
            fail_undo: false,
            undone: Rc::new(Cell::new(false)),
        }
    }

    impl IdentityLink for Fake {
        fn name(&self) -> String {
            self.name.into()
        }
        fn status(&self) -> String {
            "active".into()
        }
        fn set_password(&self, _: &str, _: &Secret) -> Result<Applied> {
            match (self.fail, self.skip) {
                (true, _) => bail!("boom"),
                (_, true) => Ok(Applied::Skipped("later".into())),
                _ => Ok(Applied::Changed(Secret::new("old"))),
            }
        }
        fn roll_back(&self, _: &str, undo: &Secret) -> Result<()> {
            assert_eq!(undo.expose(), "old");
            self.undone.set(true);
            if self.fail_undo {
                bail!("stuck");
            }
            Ok(())
        }
    }

    fn run(links: Vec<Fake>) -> (PasswordReport, Vec<bool>) {
        let undone: Vec<_> = links.iter().map(|l| l.undone.clone()).collect();
        let boxed: Vec<Box<dyn IdentityLink>> =
            links.into_iter().map(|l| Box::new(l) as _).collect();
        let report = change_all(&boxed, "dana", &Secret::new("new"));
        (report, undone.iter().map(|c| c.get()).collect())
    }

    fn states(report: &PasswordReport) -> Vec<LinkState> {
        report.links.iter().map(|l| l.state.clone()).collect()
    }

    #[test]
    fn every_link_changes_or_is_skipped() {
        let mut b = fake("b");
        b.skip = true;
        let (report, undone) = run(vec![fake("a"), b]);
        assert!(report.ok);
        assert_eq!(
            states(&report),
            [LinkState::Changed, LinkState::Skipped("later".into())]
        );
        assert_eq!(undone, [false, false]);
    }

    #[test]
    fn a_failure_rolls_back_the_earlier_links_and_leaves_the_later_ones() {
        let mut c = fake("c");
        c.fail = true;
        let (report, undone) = run(vec![fake("a"), fake("b"), c, fake("d")]);
        assert!(!report.ok);
        assert_eq!(
            states(&report),
            [
                LinkState::RolledBack,
                LinkState::RolledBack,
                LinkState::Failed("boom".into()),
                LinkState::NotTried
            ]
        );
        assert_eq!(undone, [true, true, false, false]);
    }

    #[test]
    fn a_rollback_that_fails_is_reported_and_the_others_still_go_back() {
        let mut a = fake("a");
        a.fail_undo = true;
        let mut c = fake("c");
        c.fail = true;
        let (report, undone) = run(vec![a, fake("b"), c]);
        assert_eq!(
            states(&report)[..2],
            [
                LinkState::RollbackFailed("stuck".into()),
                LinkState::RolledBack
            ]
        );
        assert_eq!(undone[..2], [true, true]);
    }

    #[test]
    fn the_policy_is_checked_once() {
        let old = Secret::new("old");
        assert!(check_policy(Some(&old), &Secret::new("")).is_err());
        assert!(check_policy(Some(&old), &Secret::new("old")).is_err());
        assert!(check_policy(None, &Secret::new("a\nb")).is_err());
        assert!(check_policy(None, &Secret::new("x".repeat(129))).is_err());
        assert!(check_policy(Some(&old), &Secret::new("fresh")).is_ok());
    }

    #[test]
    fn the_domain_link_does_nothing_yet_and_says_so() {
        let applied = Nt4Link.set_password("dana", &Secret::new("x")).unwrap();
        assert!(matches!(applied, Applied::Skipped(_)));
    }

    #[test]
    fn the_scripts_carry_the_password_encoded_and_never_plain() {
        let script = set_script("dana", &Secret::new("p@ss w0rd'\"$"));
        assert!(!script.contains("p@ss"));
        assert!(script.contains("chpasswd") && !script.contains("chpasswd "));
        let restore = restore_script("dana", &Secret::new("$6$salt$hash"));
        assert!(!restore.contains("$6$") && restore.contains("chpasswd -e"));
    }
}
