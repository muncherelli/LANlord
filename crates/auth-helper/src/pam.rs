//! pam through libpam, as the `lanlord` service. kept to the binding itself: the decision lives in
//! check.rs.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::mem::size_of;
use std::ptr;

use crate::check::Pam;

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
const PAM_RHOST: c_int = 4;
const PAM_FAIL_DELAY: c_int = 10;
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
    fn pam_set_item(pamh: *mut c_void, item_type: c_int, item: *const c_void) -> c_int;
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

/// wipe and free the first `filled` answers and the array, when a conversation gives up or pam
/// is done with them.
unsafe fn free_replies(replies: *mut PamResponse, filled: usize) {
    for i in 0..filled {
        let r = (*replies.add(i)).resp;
        if !r.is_null() {
            ptr::write_bytes(r, 0, libc::strlen(r));
            libc::free(r.cast());
        }
    }
    libc::free(replies.cast());
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
            let answer = match (**msg.add(i)).msg_style {
                PAM_PROMPT_ECHO_OFF => Some(answers.password),
                PAM_PROMPT_ECHO_ON => Some(answers.user.to_bytes()),
                PAM_ERROR_MSG | PAM_TEXT_INFO => None,
                _ => {
                    free_replies(replies, i);
                    return PAM_CONV_ERR;
                }
            };
            if let Some(bytes) = answer {
                let copy = malloc_copy(bytes);
                if copy.is_null() {
                    free_replies(replies, i);
                    return PAM_BUF_ERR;
                }
                (*replies.add(i)).resp = copy;
            }
        }
        *resp = replies;
    }
    PAM_SUCCESS
}

/// the failure delay libpam calls instead of sleeping: it does nothing, because the helper pads
/// every answer but `ok` to the same length itself (see `check::FAIL_DELAY`). libpam's own delay
/// only follows a wrong password, so it would set that apart from a right password for an
/// account that may not log in.
extern "C" fn no_delay(_status: c_int, _usec: c_uint, _appdata: *mut c_void) {}

/// what a finished pam transaction means for the login.
fn outcome(status: c_int) -> Result<bool, &'static str> {
    match status {
        PAM_SUCCESS => Ok(true),
        PAM_SYSTEM_ERR | PAM_BUF_ERR | PAM_CONV_ERR | PAM_ABORT => Err("pam"),
        // a wrong password, an unknown user, an expired or locked account
        _ => Ok(false),
    }
}

/// pam through libpam.
pub struct SystemPam;

impl Pam for SystemPam {
    fn check(
        &self,
        user: &str,
        password: &[u8],
        rhost: Option<&str>,
    ) -> Result<bool, &'static str> {
        let user = CString::new(user).map_err(|_| "bad-request")?;
        let rhost = rhost
            .map(CString::new)
            .transpose()
            .map_err(|_| "bad-request")?;
        let mut answers = Answers {
            user: &user,
            password,
        };
        let conv = PamConv {
            conv: converse,
            appdata_ptr: ptr::addr_of_mut!(answers).cast(),
        };
        let delay: extern "C" fn(c_int, c_uint, *mut c_void) = no_delay;
        let mut handle = ptr::null_mut();
        // SAFETY: every pointer outlives the pam handle, which is ended before returning.
        unsafe {
            if pam_start(SERVICE.as_ptr(), user.as_ptr(), &conv, &mut handle) != PAM_SUCCESS {
                return Err("pam");
            }
            let mut status = pam_set_item(handle, PAM_FAIL_DELAY, delay as *const c_void);
            if status == PAM_SUCCESS {
                if let Some(rhost) = &rhost {
                    status = pam_set_item(handle, PAM_RHOST, rhost.as_ptr().cast());
                }
            }
            if status != PAM_SUCCESS {
                pam_end(handle, status);
                return Err("pam");
            }
            let flags = PAM_SILENT | PAM_DISALLOW_NULL_AUTHTOK;
            status = pam_authenticate(handle, flags);
            if status == PAM_SUCCESS {
                status = pam_acct_mgmt(handle, flags);
            }
            pam_end(handle, status);
            outcome(status)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// run the conversation the way pam does, returning its status and the answers as strings.
    fn talk(styles: &[c_int]) -> (c_int, Vec<Option<String>>) {
        let user = c"alice";
        let mut answers = Answers {
            user,
            password: b"secret",
        };
        let messages: Vec<PamMessage> = styles
            .iter()
            .map(|&msg_style| PamMessage {
                msg_style,
                msg: c"prompt".as_ptr(),
            })
            .collect();
        let mut pointers: Vec<*const PamMessage> =
            messages.iter().map(|m| m as *const PamMessage).collect();
        let mut replies: *mut PamResponse = ptr::null_mut();
        let status = converse(
            c_int::try_from(styles.len()).unwrap(),
            pointers.as_mut_ptr(),
            &mut replies,
            ptr::addr_of_mut!(answers).cast(),
        );
        let mut got = Vec::new();
        if status == PAM_SUCCESS {
            // SAFETY: converse filled `styles.len()` replies, freed here as pam would
            unsafe {
                for i in 0..styles.len() {
                    let r = (*replies.add(i)).resp;
                    got.push((!r.is_null()).then(|| CStr::from_ptr(r).to_string_lossy().into()));
                }
                free_replies(replies, styles.len());
            }
        }
        (status, got)
    }

    #[test]
    fn answers_the_password_prompt_with_the_password_and_messages_with_nothing() {
        let (status, got) = talk(&[PAM_TEXT_INFO, PAM_PROMPT_ECHO_OFF, PAM_ERROR_MSG]);
        assert_eq!(status, PAM_SUCCESS);
        assert_eq!(got, vec![None, Some("secret".into()), None]);
    }

    #[test]
    fn answers_a_visible_prompt_with_the_user_name() {
        let (status, got) = talk(&[PAM_PROMPT_ECHO_ON]);
        assert_eq!(status, PAM_SUCCESS);
        assert_eq!(got, vec![Some("alice".into())]);
    }

    #[test]
    fn gives_up_on_prompts_it_does_not_know() {
        assert_eq!(talk(&[PAM_PROMPT_ECHO_OFF, 99]).0, PAM_CONV_ERR);
    }

    #[test]
    fn refuses_an_empty_or_broken_conversation() {
        let mut replies: *mut PamResponse = ptr::null_mut();
        assert_eq!(
            converse(0, ptr::null_mut(), &mut replies, ptr::null_mut()),
            PAM_CONV_ERR
        );
        assert_eq!(
            converse(-1, ptr::null_mut(), &mut replies, ptr::null_mut()),
            PAM_CONV_ERR
        );
    }

    #[test]
    fn only_success_lets_in_and_only_pam_trouble_is_an_error() {
        assert_eq!(outcome(PAM_SUCCESS), Ok(true));
        for trouble in [PAM_SYSTEM_ERR, PAM_BUF_ERR, PAM_CONV_ERR, PAM_ABORT] {
            assert_eq!(outcome(trouble), Err("pam"));
        }
        // auth error, unknown user, expired account, password change needed
        for refusal in [7, 10, 13, 12] {
            assert_eq!(outcome(refusal), Ok(false));
        }
    }

    #[test]
    fn names_or_addresses_with_a_nul_never_reach_pam() {
        assert_eq!(SystemPam.check("a\0b", b"x", None), Err("bad-request"));
        assert_eq!(
            SystemPam.check("alice", b"x", Some("192.0.2.7\0")),
            Err("bad-request")
        );
    }

    #[test]
    fn the_failure_delay_handed_to_libpam_returns_at_once() {
        let start = std::time::Instant::now();
        no_delay(7, 2_000_000, ptr::null_mut());
        assert!(start.elapsed() < std::time::Duration::from_millis(50));
    }
}
