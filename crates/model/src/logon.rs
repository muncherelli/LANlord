//! the logon scripts of the nt4 domain: `logon.bat` runs for everyone and `<user>.bat` for one
//! user, both called by the generated `lanlord.bat`. they live in the netlogon share, which
//! domain admins may also edit from windows; this module reads and writes them from the gateway.

use crate::users::valid_name;

/// the script that runs for everyone.
pub const COMMON: &str = "logon";

/// the generated script that calls the others; it is rewritten on every apply.
pub const GENERATED: &str = "lanlord";

/// a script is a few lines of batch; anything bigger is a mistake.
pub const MAX_BYTES: usize = 64 * 1024;

/// `logon` or the name of a login; the generated script is not editable.
pub fn valid_script_name(name: &str) -> bool {
    name != GENERATED && valid_name(name)
}

/// dos line endings, as nt4 and windows 9x expect; a missing final newline is added.
pub fn to_dos(text: &str) -> String {
    let unix = text.replace("\r\n", "\n");
    let mut dos = unix.replace('\n', "\r\n");
    if !dos.is_empty() && !dos.ends_with("\r\n") {
        dos.push_str("\r\n");
    }
    dos
}

/// unix line endings, for editing.
pub fn to_unix(text: &str) -> String {
    text.replace("\r\n", "\n")
}
