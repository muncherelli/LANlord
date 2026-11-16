//! the linux system calls: the account and group databases through libc, the peer of the socket
//! systemd hands over, and the few lines that tie a run together.

use std::ffi::{c_char, c_int, CString};
use std::io::Write;
use std::mem::{size_of, MaybeUninit};
use std::ptr;
use std::time::Instant;

use crate::check::{answer, padding, Groups, System};
use crate::pam::SystemPam;
use crate::protocol::Verdict;

/// the uid and primary gid of `user`, or `None` when there is no such account.
fn lookup(user: &str) -> Result<Option<(u32, u32)>, &'static str> {
    let name = CString::new(user).map_err(|_| "bad-request")?;
    let mut buf = vec![0 as c_char; 4096];
    let mut pwd = MaybeUninit::<libc::passwd>::uninit();
    let mut found = ptr::null_mut();
    loop {
        // SAFETY: the buffers are sized as given and outlive the call.
        let rc = unsafe {
            libc::getpwnam_r(
                name.as_ptr(),
                pwd.as_mut_ptr(),
                buf.as_mut_ptr(),
                buf.len(),
                &mut found,
            )
        };
        if rc == libc::ERANGE && buf.len() < 1 << 20 {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        if rc != 0 {
            return Err("passwd");
        }
        if found.is_null() {
            return Ok(None);
        }
        // SAFETY: getpwnam_r filled `pwd` because it returned 0 with a result.
        let pwd = unsafe { pwd.assume_init() };
        return Ok(Some((pwd.pw_uid, pwd.pw_gid)));
    }
}

/// the group database through libc.
pub struct SystemGroups;

impl Groups for SystemGroups {
    fn member(&self, user: &str, gid: u32) -> Result<bool, &'static str> {
        let Some((_, primary)) = lookup(user)? else {
            return Ok(false);
        };
        let name = CString::new(user).map_err(|_| "bad-request")?;
        let mut groups: Vec<libc::gid_t> = vec![0; 64];
        loop {
            let mut count = c_int::try_from(groups.len()).map_err(|_| "groups")?;
            // SAFETY: `groups` holds `count` entries.
            let rc = unsafe {
                libc::getgrouplist(name.as_ptr(), primary, groups.as_mut_ptr(), &mut count)
            };
            let count = usize::try_from(count).map_err(|_| "groups")?;
            if rc >= 0 {
                return Ok(groups[..count].contains(&gid));
            }
            if count <= groups.len() || count > 65536 {
                return Err("groups");
            }
            groups.resize(count, 0);
        }
    }
}

/// this process, with the connection systemd hands over as stdin.
pub struct SystemCalls;

impl System for SystemCalls {
    fn is_root(&self) -> bool {
        // SAFETY: geteuid has no preconditions.
        unsafe { libc::geteuid() == 0 }
    }

    fn peer_uid(&self) -> Result<Option<u32>, &'static str> {
        peer_uid(0)
    }

    fn uid_of(&self, user: &str) -> Result<Option<u32>, &'static str> {
        Ok(lookup(user)?.map(|(uid, _)| uid))
    }
}

/// the uid on the other end of `fd` when it is a socket, `None` when it is something else.
fn peer_uid(fd: c_int) -> Result<Option<u32>, &'static str> {
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat fills `stat` when it returns 0.
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
        return Err("stdin");
    }
    let stat = unsafe { stat.assume_init() };
    if stat.st_mode & libc::S_IFMT != libc::S_IFSOCK {
        return Ok(None);
    }
    let mut cred = MaybeUninit::<libc::ucred>::uninit();
    let mut len = size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` is `len` bytes long.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            cred.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return Err("peer");
    }
    Ok(Some(unsafe { cred.assume_init() }.uid))
}

/// check one login and answer it, taking at least `FAIL_DELAY` for anything but `ok`.
pub fn run(args: &[String]) -> Verdict {
    let start = Instant::now();
    let verdict = answer(
        args,
        &SystemCalls,
        &mut std::io::stdin().lock(),
        &SystemPam,
        &SystemGroups,
    );
    std::thread::sleep(padding(verdict, start.elapsed()));
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", verdict.line());
    let _ = out.flush();
    verdict
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;

    use super::*;

    #[test]
    fn root_is_in_its_own_primary_group_and_unknown_users_are_in_none() {
        assert_eq!(SystemGroups.member("root", 0), Ok(true));
        assert_eq!(SystemGroups.member("root", 4_000_000), Ok(false));
        assert_eq!(
            SystemGroups.member("no-such-user-lanlord-test", 0),
            Ok(false)
        );
    }

    #[test]
    fn looks_up_accounts() {
        assert_eq!(lookup("root"), Ok(Some((0, 0))));
        assert_eq!(lookup("no-such-user-lanlord-test"), Ok(None));
        assert_eq!(lookup("a\0b"), Err("bad-request"));
        assert_eq!(SystemCalls.uid_of("root"), Ok(Some(0)));
    }

    #[test]
    fn finds_the_uid_on_the_other_end_of_a_socket() {
        let (ours, _theirs) = UnixStream::pair().unwrap();
        // SAFETY: getuid has no preconditions.
        let me = unsafe { libc::getuid() };
        assert_eq!(peer_uid(ours.as_raw_fd()), Ok(Some(me)));
    }

    #[test]
    fn a_file_or_pipe_has_no_peer_and_a_closed_descriptor_is_an_error() {
        let file = std::fs::File::open("/dev/null").unwrap();
        assert_eq!(peer_uid(file.as_raw_fd()), Ok(None));
        assert_eq!(peer_uid(-1), Err("stdin"));
    }

    #[test]
    fn tests_do_not_run_as_root_unless_the_runner_does() {
        // SAFETY: geteuid has no preconditions.
        assert_eq!(SystemCalls.is_root(), unsafe { libc::geteuid() } == 0);
    }
}
