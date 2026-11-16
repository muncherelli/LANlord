//! the linux side: pam through libpam, the group database through libc, and the check that the
//! caller on the other end of the socket is the web ui's account.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::io::Write;
use std::mem::{size_of, MaybeUninit};
use std::ptr;
use std::time::Instant;

use crate::check::{decide, Groups, Pam, FAIL_DELAY};
use crate::protocol::{read_request, valid_user, Verdict};

/// the pam service name; its rules live in /etc/pam.d/lanlord.
const SERVICE: &CStr = c"lanlord";

const PAM_SUCCESS: c_int = 0;
const PAM_SYSTEM_ERR: c_int = 4;
const PAM_BUF_ERR: c_int = 5;
const PAM_CONV_ERR: c_int = 19;
const PAM_ABORT: c_int = 26;
const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_ERROR_MSG: c_int = 3;
const PAM_TEXT_INFO: c_int = 4;
const PAM_SILENT: c_int = 0x8000;
const PAM_DISALLOW_NULL_AUTHTOK: c_int = 0x0001;

#[repr(C)]
struct PamMessage {
    msg_style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}

type Converse = extern "C" fn(
    num_msg: c_int,
    msg: *mut *const PamMessage,
    resp: *mut *mut PamResponse,
    appdata: *mut c_void,
) -> c_int;

#[repr(C)]
struct PamConv {
    conv: Converse,
    appdata_ptr: *mut c_void,
}

#[link(name = "pam")]
extern "C" {
    fn pam_start(
        service: *const c_char,
        user: *const c_char,
        conv: *const PamConv,
        pamh: *mut *mut c_void,
    ) -> c_int;
    fn pam_authenticate(pamh: *mut c_void, flags: c_int) -> c_int;
    fn pam_acct_mgmt(pamh: *mut c_void, flags: c_int) -> c_int;
    fn pam_end(pamh: *mut c_void, status: c_int) -> c_int;
}

/// what the conversation hands pam when it asks.
struct Answers<'a> {
    user: &'a CStr,
    password: &'a [u8],
}

/// a malloc'd, nul-terminated copy of `bytes`: pam frees the answers itself (pam_unix wipes
/// them first).
unsafe fn malloc_copy(bytes: &[u8]) -> *mut c_char {
    let p = libc::malloc(bytes.len() + 1).cast::<u8>();
    if !p.is_null() {
        ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        *p.add(bytes.len()) = 0;
    }
    p.cast()
}

/// answers pam's prompts: the password for a hidden prompt, the user name for a visible one,
/// nothing for messages; anything else ends the conversation.
extern "C" fn converse(
    num_msg: c_int,
    msg: *mut *const PamMessage,
    resp: *mut *mut PamResponse,
    appdata: *mut c_void,
) -> c_int {
    let Ok(n) = usize::try_from(num_msg) else {
        return PAM_CONV_ERR;
    };
    if n == 0 || msg.is_null() || resp.is_null() || appdata.is_null() {
        return PAM_CONV_ERR;
    }
    // SAFETY: pam passes `num_msg` message pointers and the `Answers` given to pam_start, and
    // owns (and frees) the response array allocated here.
    unsafe {
        let answers = &*appdata.cast::<Answers>();
        let replies = libc::calloc(n, size_of::<PamResponse>()).cast::<PamResponse>();
        if replies.is_null() {
            return PAM_BUF_ERR;
        }
        for i in 0..n {
            let m = &**msg.add(i);
            let answer = match m.msg_style {
                PAM_PROMPT_ECHO_OFF => malloc_copy(answers.password),
                PAM_PROMPT_ECHO_ON => malloc_copy(answers.user.to_bytes()),
                PAM_ERROR_MSG | PAM_TEXT_INFO => ptr::null_mut(),
                _ => {
                    free_replies(replies, i);
                    return PAM_CONV_ERR;
                }
            };
            if answer.is_null() && matches!(m.msg_style, PAM_PROMPT_ECHO_OFF | PAM_PROMPT_ECHO_ON) {
                free_replies(replies, i);
                return PAM_BUF_ERR;
            }
            (*replies.add(i)).resp = answer;
        }
        *resp = replies;
    }
    PAM_SUCCESS
}

/// wipe and free the first `filled` answers and the array, when a conversation gives up.
unsafe fn free_replies(replies: *mut PamResponse, filled: usize) {
    for i in 0..filled {
        let r = (*replies.add(i)).resp;
        if !r.is_null() {
            let len = libc::strlen(r);
            ptr::write_bytes(r, 0, len);
            libc::free(r.cast());
        }
    }
    libc::free(replies.cast());
}

/// pam through libpam, as the `lanlord` service.
pub struct SystemPam;

impl Pam for SystemPam {
    fn check(&self, user: &str, password: &[u8]) -> Result<bool, &'static str> {
        let user = CString::new(user).map_err(|_| "bad-request")?;
        let mut answers = Answers {
            user: &user,
            password,
        };
        let conv = PamConv {
            conv: converse,
            appdata_ptr: ptr::addr_of_mut!(answers).cast(),
        };
        let mut handle = ptr::null_mut();
        // SAFETY: every pointer outlives the pam handle, which is ended before returning.
        unsafe {
            if pam_start(SERVICE.as_ptr(), user.as_ptr(), &conv, &mut handle) != PAM_SUCCESS {
                return Err("pam");
            }
            let flags = PAM_SILENT | PAM_DISALLOW_NULL_AUTHTOK;
            let mut status = pam_authenticate(handle, flags);
            if status == PAM_SUCCESS {
                status = pam_acct_mgmt(handle, flags);
            }
            pam_end(handle, status);
            match status {
                PAM_SUCCESS => Ok(true),
                PAM_SYSTEM_ERR | PAM_BUF_ERR | PAM_CONV_ERR | PAM_ABORT => Err("pam"),
                // a wrong password, an unknown user, an expired or locked account
                _ => Ok(false),
            }
        }
    }
}

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

/// the command line: `--caller <account>`, the only account allowed on the other end of the
/// socket.
fn caller(args: &[String]) -> Result<Option<String>, &'static str> {
    match args {
        [] => Ok(None),
        [flag, name] if flag == "--caller" && valid_user(name) => Ok(Some(name.clone())),
        _ => Err("usage"),
    }
}

/// when stdin is a socket (systemd hands one over per connection), the process on the other end
/// must run as `caller`. run by hand, from a terminal or a pipe, there is no peer to check.
fn check_peer(caller: Option<&str>) -> Result<(), &'static str> {
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat fills `stat` when it returns 0.
    if unsafe { libc::fstat(0, stat.as_mut_ptr()) } != 0 {
        return Err("stdin");
    }
    let stat = unsafe { stat.assume_init() };
    if stat.st_mode & libc::S_IFMT != libc::S_IFSOCK {
        return Ok(());
    }
    let caller = caller.ok_or("usage")?;
    let (uid, _) = lookup(caller)?.ok_or("caller")?;
    let mut cred = MaybeUninit::<libc::ucred>::uninit();
    let mut len = size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` is `len` bytes long.
    let rc = unsafe {
        libc::getsockopt(
            0,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            cred.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return Err("peer");
    }
    let cred = unsafe { cred.assume_init() };
    if cred.uid == uid {
        Ok(())
    } else {
        Err("caller")
    }
}

fn verdict(args: &[String]) -> Verdict {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        return Verdict::Error("not-root");
    }
    let caller = match caller(args) {
        Ok(caller) => caller,
        Err(code) => return Verdict::Error(code),
    };
    if let Err(code) = check_peer(caller.as_deref()) {
        return Verdict::Error(code);
    }
    let request = match read_request(&mut std::io::stdin().lock()) {
        Ok(request) => request,
        Err(code) => return Verdict::Error(code),
    };
    let verdict = decide(&request, &SystemPam, &SystemGroups);
    // the journal gets who and how, never the password
    eprintln!("web ui login for {}: {}", request.user, verdict.line());
    verdict
}

/// check one login and answer it, taking at least `FAIL_DELAY` for anything but `ok`.
pub fn run(args: &[String]) -> Verdict {
    let start = Instant::now();
    let verdict = verdict(args);
    if verdict != Verdict::Ok {
        std::thread::sleep(FAIL_DELAY.saturating_sub(start.elapsed()));
    }
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", verdict.line());
    let _ = out.flush();
    verdict
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_takes_only_a_valid_caller() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(caller(&args(&[])), Ok(None));
        assert_eq!(
            caller(&args(&["--caller", "lanlord"])),
            Ok(Some("lanlord".into()))
        );
        assert_eq!(caller(&args(&["--caller", "Bad Name"])), Err("usage"));
        assert_eq!(caller(&args(&["--caller"])), Err("usage"));
        assert_eq!(caller(&args(&["--other", "x"])), Err("usage"));
    }

    #[test]
    fn root_is_in_its_own_primary_group_and_unknown_users_are_in_none() {
        assert_eq!(SystemGroups.member("root", 0), Ok(true));
        assert_eq!(
            SystemGroups.member("no-such-user-lanlord-test", 0),
            Ok(false)
        );
    }

    #[test]
    fn looks_up_root() {
        assert_eq!(lookup("root"), Ok(Some((0, 0))));
        assert_eq!(lookup("no-such-user-lanlord-test"), Ok(None));
    }
}
