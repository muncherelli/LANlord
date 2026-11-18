//! signing in to a gateway with your own ssh credentials. nothing is stored: an agent or key
//! signs you in silently; when the server offers passwords the caller asks for one, and it
//! reaches ssh through its askpass hook for that one attempt only.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::ssh::{control_args, Session};

pub use lanlord_model::signin::*;

/// the environment variable that carries the password to the askpass helper (this program,
/// started by ssh). it exists only in the environment of the one ssh run that needs it.
pub const ASKPASS_SECRET: &str = "LANLORD_ASKPASS_SECRET";

/// how one attempt authenticates.
#[derive(Default)]
pub struct Attempt<'a> {
    /// answer a password prompt with this; `None` tries the agent and keys only.
    pub password: Option<&'a str>,
    /// record the server's host key if it is new (the user has seen its fingerprint).
    pub trust_new_host: bool,
}

/// signs in through the system ssh client.
pub struct Signin {
    /// the ssh program and any leading arguments.
    program: Vec<String>,
    /// the program ssh starts to ask for a password: this program, in askpass mode.
    askpass: Option<PathBuf>,
    /// the fake ssh script, removed when the signer is dropped.
    #[cfg(all(test, unix))]
    #[allow(dead_code)]
    fake_script: Option<tempfile::TempPath>,
}

impl Signin {
    pub fn system() -> Self {
        Self {
            program: vec!["ssh".into()],
            askpass: std::env::current_exe().ok(),
            #[cfg(all(test, unix))]
            fake_script: None,
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.program[0]);
        cmd.args(&self.program[1..]);
        cmd
    }

    /// sign in to `target` (`user@host`) and say who the gateway thinks you are. the connection
    /// stays open in the background for a while, so later commands reuse it without asking again.
    pub fn attempt(&self, target: &str, how: &Attempt) -> Result<Identity, SigninError> {
        check_target(target)?;
        let mut cmd = self.command();
        cmd.args(["-o", "ConnectTimeout=8", "-o", "NumberOfPasswordPrompts=1"]);
        cmd.args([
            "-o",
            if how.trust_new_host {
                "StrictHostKeyChecking=accept-new"
            } else {
                "StrictHostKeyChecking=yes"
            },
        ]);
        cmd.args(control_args());
        cmd.stdin(Stdio::null());
        match how.password {
            None => {
                // never ask: the gui has no terminal to answer on
                cmd.args(["-o", "BatchMode=yes"]);
            }
            Some(password) => {
                let askpass = self
                    .askpass
                    .as_ref()
                    .ok_or_else(|| SigninError::Other("no password helper available".into()))?;
                cmd.args([
                    "-o",
                    "BatchMode=no",
                    "-o",
                    "PreferredAuthentications=password,keyboard-interactive",
                ]);
                cmd.env("SSH_ASKPASS", askpass)
                    .env("SSH_ASKPASS_REQUIRE", "force")
                    .env(ASKPASS_SECRET, password);
            }
        }
        cmd.arg("--").arg(target).arg("id -un && id -u && id -nG");
        let out = cmd.output().map_err(spawn_error)?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        match out.status.code() {
            Some(0) => parse_identity(&String::from_utf8_lossy(&out.stdout)),
            Some(255) => Err(classify(&stderr, how.password.is_some())),
            Some(code) => Err(SigninError::Other(format!(
                "the server ran the check with status {code}: {}",
                last_line(&stderr)
            ))),
            None => Err(SigninError::Other("ssh was terminated by a signal".into())),
        }
    }

    /// the host key `target` presents, as `ssh-ed25519 SHA256:...`, read without sending
    /// any credentials, so the user can compare it before trusting the server.
    pub fn fingerprint(&self, target: &str) -> Result<String, SigninError> {
        check_target(target)?;
        let mut cmd = self.command();
        cmd.args([
            "-v",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            // only to read the key: no credentials are offered
            "-o",
            "PreferredAuthentications=none",
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "ControlMaster=no",
            "-o",
            "ControlPath=none",
            "-o",
        ]);
        cmd.arg(if cfg!(windows) {
            "UserKnownHostsFile=NUL"
        } else {
            "UserKnownHostsFile=/dev/null"
        });
        cmd.arg("--").arg(target).arg("true").stdin(Stdio::null());
        let out = cmd.output().map_err(spawn_error)?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        match host_key(&stderr) {
            Some(key) => Ok(key),
            None => Err(match classify(&stderr, false) {
                SigninError::Other(_) | SigninError::NeedsPassword | SigninError::KeysOnly(_) => {
                    SigninError::Other("the server did not present a host key".into())
                }
                known => known,
            }),
        }
    }

    /// close the shared connection to `target`, so nothing stays signed in after a log off.
    pub fn sign_out(&self, target: &str) {
        if check_target(target).is_err() || control_args().is_empty() {
            return;
        }
        let _ = self
            .command()
            .args(control_args())
            .args(["-O", "exit", "--", target])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// the public keys your ssh agent holds, one `type key comment` line each; empty when there is
/// no agent, it is locked, or it holds none. nothing is asked of the agent but its list.
pub fn agent_keys() -> Vec<String> {
    let out = Command::new("ssh-add")
        .arg("-L")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(out) if out.status.success() => parse_agent_keys(&String::from_utf8_lossy(&out.stdout)),
        _ => Vec::new(),
    }
}

/// the key lines in `ssh-add -L` output, skipping its messages.
fn parse_agent_keys(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|l| {
            ["ssh-", "ecdsa-", "sk-ssh-", "sk-ecdsa-"]
                .iter()
                .any(|p| l.starts_with(p))
                && l.split_whitespace().count() >= 2
        })
        .map(String::from)
        .collect()
}

/// the same rules as every other ssh use, and a user is required.
fn check_target(target: &str) -> Result<(), SigninError> {
    Session::new(target).map_err(|err| SigninError::Other(err.to_string()))?;
    match target.split_once('@') {
        Some((user, host)) if !user.is_empty() && !host.is_empty() => Ok(()),
        _ => Err(SigninError::Other("type a username and a hostname".into())),
    }
}

fn spawn_error(err: std::io::Error) -> SigninError {
    if err.kind() == std::io::ErrorKind::NotFound {
        SigninError::NotInstalled
    } else {
        SigninError::Other(err.to_string())
    }
}

fn last_line(text: &str) -> &str {
    text.lines()
        .rfind(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
}

/// who this machine's own account is, for the logon box's "This server" choice: no ssh, the
/// account that runs lanlord.
pub fn local_identity() -> Result<Identity, SigninError> {
    let out = Session::local()
        .capture(&["sh".into(), "-c".into(), "id -un && id -u && id -nG".into()])
        .map_err(|err| SigninError::Other(err.to_string()))?;
    parse_identity(&out)
}

fn parse_identity(stdout: &str) -> Result<Identity, SigninError> {
    let mut lines = stdout.lines().map(str::trim).filter(|l| !l.is_empty());
    let (Some(user), Some(uid)) = (lines.next(), lines.next()) else {
        return Err(SigninError::Other(
            "the server did not say who you are".into(),
        ));
    };
    let uid = uid
        .parse()
        .map_err(|_| SigninError::Other("the server did not say who you are".into()))?;
    // the groups come last; without them nobody is taken for an administrator
    let admin = lines.next().is_some_and(|groups| {
        groups
            .split_whitespace()
            .any(|g| g == crate::admins::ADMINS_GROUP)
    });
    Ok(Identity {
        user: user.to_string(),
        uid,
        admin,
        grants: None,
    })
}

/// the methods in ssh's `Permission denied (publickey,password).` line.
fn denied_methods(stderr: &str) -> Option<Vec<String>> {
    let line = stderr.lines().find(|l| l.contains("Permission denied ("))?;
    let inside = line.split_once("Permission denied (")?.1.split_once(')')?.0;
    Some(inside.split(',').map(|m| m.trim().to_string()).collect())
}

/// map ssh's stderr to a specific failure; `used_password` says a password was offered.
fn classify(stderr: &str, used_password: bool) -> SigninError {
    if let Some(methods) = denied_methods(stderr) {
        let password_ok = methods
            .iter()
            .any(|m| m == "password" || m == "keyboard-interactive");
        return if used_password {
            SigninError::WrongPassword
        } else if password_ok {
            SigninError::NeedsPassword
        } else {
            SigninError::KeysOnly(methods)
        };
    }
    let has = |text: &str| stderr.contains(text);
    if has("REMOTE HOST IDENTIFICATION HAS CHANGED")
        || has("has changed and you have requested strict checking")
    {
        SigninError::HostKeyChanged
    } else if has("Host key verification failed") {
        SigninError::HostKeyUnknown
    } else if has("communication with agent failed") {
        SigninError::AgentLocked
    } else if has("Could not resolve hostname")
        || has("Name or service not known")
        || has("nodename nor servname")
    {
        SigninError::Unreachable("That hostname could not be found.".into())
    } else if has("timed out") {
        SigninError::Timeout
    } else if has("Connection refused") {
        SigninError::Refused
    } else if has("No route to host") || has("Network is unreachable") {
        SigninError::Unreachable("There is no route to that host.".into())
    } else {
        SigninError::Other(last_line(stderr).to_string())
    }
}

/// the key from ssh's `-v` line `Server host key: ssh-ed25519 SHA256:...`.
fn host_key(stderr: &str) -> Option<String> {
    let line = stderr.lines().find(|l| l.contains("Server host key: "))?;
    let rest = line.split_once("Server host key: ")?.1;
    let mut words = rest.split_whitespace();
    Some(format!("{} {}", words.next()?, words.next()?))
}

/// what ssh's askpass hook should answer to `prompt`: the password for a password prompt and
/// nothing for anything else, so a key's passphrase or a host key question never gets it.
pub fn askpass_answer(prompt: &str, secret: &str) -> Option<String> {
    prompt
        .to_lowercase()
        .contains("password")
        .then(|| secret.to_string())
}

/// when ssh started this program as its askpass helper, print the password and say what to
/// exit with; `None` for a normal start.
pub fn askpass_main() -> Option<i32> {
    let secret = std::env::var(ASKPASS_SECRET).ok()?;
    let prompt = std::env::args().nth(1).unwrap_or_default();
    Some(match askpass_answer(&prompt, &secret) {
        Some(answer) => {
            println!("{answer}");
            0
        }
        None => 1,
    })
}

#[cfg(all(test, unix))]
impl Signin {
    /// a signer that runs `script` instead of ssh, to test without a server.
    fn fake(script: &str) -> Signin {
        let path = tempfile::Builder::new()
            .prefix("lanlord-fake-signin-")
            .tempfile()
            .unwrap();
        std::fs::write(path.path(), format!("{script}\n")).unwrap();
        let path = path.into_temp_path();
        Signin {
            // run it through sh so we never exec a file we just wrote (ETXTBSY)
            program: vec!["sh".into(), path.to_string_lossy().into_owned()],
            askpass: Some(PathBuf::from("/opt/lanlord/askpass-test")),
            fake_script: Some(path),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denied_lines_name_the_methods() {
        let line = "alice@gw: Permission denied (publickey,password,keyboard-interactive).";
        assert_eq!(
            denied_methods(line).unwrap(),
            ["publickey", "password", "keyboard-interactive"]
        );
        assert!(denied_methods("boom").is_none());
        assert!(denied_methods("Permission denied (broken").is_none());
    }

    #[test]
    fn classification_tells_the_failures_apart() {
        let denied = |m: &str| format!("alice@gw: Permission denied ({m}).");
        assert_eq!(
            classify(&denied("publickey,password"), false),
            SigninError::NeedsPassword
        );
        assert_eq!(
            classify(&denied("publickey,keyboard-interactive"), false),
            SigninError::NeedsPassword
        );
        assert_eq!(
            classify(&denied("publickey"), false),
            SigninError::KeysOnly(vec!["publickey".into()])
        );
        assert_eq!(
            classify(&denied("publickey,password"), true),
            SigninError::WrongPassword
        );
        let cases = [
            ("Host key verification failed.", SigninError::HostKeyUnknown),
            (
                "@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @\nHost key verification failed.",
                SigninError::HostKeyChanged,
            ),
            (
                "Host key for gw has changed and you have requested strict checking.",
                SigninError::HostKeyChanged,
            ),
            (
                "sign_and_send_pubkey: signing failed: communication with agent failed",
                SigninError::AgentLocked,
            ),
            (
                "ssh: connect to host gw port 22: Operation timed out",
                SigninError::Timeout,
            ),
            (
                "ssh: connect to host gw port 22: Connection timed out",
                SigninError::Timeout,
            ),
            (
                "ssh: connect to host gw port 22: Connection refused",
                SigninError::Refused,
            ),
        ];
        for (stderr, want) in cases {
            assert_eq!(classify(stderr, false), want, "{stderr}");
        }
        for stderr in [
            "ssh: Could not resolve hostname gw: Name or service not known",
            "ssh: Could not resolve hostname gw: nodename nor servname provided",
        ] {
            assert!(matches!(
                classify(stderr, false),
                SigninError::Unreachable(_)
            ));
        }
        assert!(matches!(
            classify("connect: No route to host", false),
            SigninError::Unreachable(_)
        ));
        assert!(matches!(
            classify("connect: Network is unreachable", false),
            SigninError::Unreachable(_)
        ));
        assert_eq!(
            classify("one\nsomething odd\n\n", false),
            SigninError::Other("something odd".into())
        );
    }

    #[test]
    fn every_failure_reads_as_a_sentence() {
        let all = [
            SigninError::NotInstalled,
            SigninError::AgentLocked,
            SigninError::NeedsPassword,
            SigninError::KeysOnly(vec!["publickey".into(), "gssapi".into()]),
            SigninError::WrongPassword,
            SigninError::HostKeyUnknown,
            SigninError::HostKeyChanged,
            SigninError::Timeout,
            SigninError::Refused,
            SigninError::Unreachable("There is no route.".into()),
            SigninError::Other("odd.".into()),
        ];
        for error in all {
            let text = error.to_string();
            assert!(text.ends_with('.') || text.ends_with('?'), "{text}");
        }
        assert!(SigninError::KeysOnly(vec!["publickey".into()])
            .to_string()
            .contains("only accepts keys (publickey)"));
    }

    #[test]
    fn the_identity_comes_from_two_lines() {
        let id = parse_identity("alice\n1000\n").unwrap();
        assert_eq!(
            (id.user.as_str(), id.uid, id.is_root()),
            ("alice", 1000, false)
        );
        assert!(parse_identity("root\n0\n").unwrap().is_root());
        // an administrator is who is in the admins group, not whoever signs in
        assert!(!parse_identity("alice\n1000\nalice users\n").unwrap().admin);
        assert!(!parse_identity("alice\n1000\n").unwrap().admin);
        assert!(parse_identity("alice\n1000\nalice admins\n").unwrap().admin);
        assert!(parse_identity("alice\n").is_err());
        assert!(parse_identity("alice\nnope\n").is_err());
    }

    #[test]
    fn the_host_key_is_read_from_the_verbose_output() {
        let stderr =
            "debug1: Connecting\ndebug1: Server host key: ssh-ed25519 SHA256:abc123 extra\nnext";
        assert_eq!(host_key(stderr).unwrap(), "ssh-ed25519 SHA256:abc123");
        assert!(host_key("nothing here").is_none());
        assert!(host_key("Server host key: lonely").is_none());
    }

    #[test]
    fn agent_keys_are_the_key_lines_only() {
        let output = "ssh-ed25519 AAAAC3Nza me@laptop\n\
                      ecdsa-sha2-nistp256 AAAAE2Vj\n\
                      The agent has no identities.\n\
                      ssh-rsa\n\n";
        assert_eq!(
            parse_agent_keys(output),
            [
                "ssh-ed25519 AAAAC3Nza me@laptop",
                "ecdsa-sha2-nistp256 AAAAE2Vj"
            ]
        );
        assert!(parse_agent_keys("The agent has no identities.").is_empty());
        // whatever the machine's agent says, listing never panics
        let _ = agent_keys();
    }

    #[test]
    fn only_password_prompts_get_the_password() {
        let secret = "hunter2";
        for prompt in [
            "alice@gw's password: ",
            "(alice@gw) Password: ",
            "Password:",
        ] {
            assert_eq!(askpass_answer(prompt, secret).as_deref(), Some(secret));
        }
        for prompt in [
            "Enter passphrase for key '/home/a/.ssh/id_ed25519': ",
            "Are you sure you want to continue connecting (yes/no/[fingerprint])? ",
            "Verification code: ",
            "",
        ] {
            assert!(askpass_answer(prompt, secret).is_none(), "{prompt}");
        }
    }

    #[test]
    fn targets_need_a_user_and_a_host() {
        let signin = Signin::system();
        for bad in ["", "gw", "@gw", "alice@", "-oProxyCommand=x@y", "a b@c"] {
            assert!(
                matches!(
                    signin.attempt(bad, &Attempt::default()),
                    Err(SigninError::Other(_))
                ),
                "{bad}"
            );
            assert!(signin.fingerprint(bad).is_err());
        }
        // a log off of something that is not a target does nothing
        signin.sign_out("");
    }

    #[test]
    fn a_missing_ssh_is_reported() {
        let signin = Signin {
            program: vec!["/nonexistent/ssh".into()],
            askpass: None,
            #[cfg(all(test, unix))]
            fake_script: None,
        };
        assert_eq!(
            signin.attempt("alice@gw", &Attempt::default()),
            Err(SigninError::NotInstalled)
        );
        assert_eq!(
            signin.fingerprint("alice@gw"),
            Err(SigninError::NotInstalled)
        );
    }

    #[cfg(unix)]
    mod fake_ssh {
        use super::*;

        fn silent(signin: &Signin) -> Result<Identity, SigninError> {
            signin.attempt("alice@gw", &Attempt::default())
        }

        #[test]
        fn a_working_key_signs_you_in_without_asking() {
            // the script fails the test run if ssh could prompt
            let signin = Signin::fake(
                r#"case "$*" in *BatchMode=yes*) ;; *) exit 9 ;; esac
case "$*" in *StrictHostKeyChecking=yes*) ;; *) exit 8 ;; esac
case "$*" in *ControlPersist*) ;; *) exit 7 ;; esac
for a; do last=$a; done
case "$last" in "id -un && id -u && id -nG") printf 'alice\n1000\nalice admins\n' ;; *) exit 6 ;; esac"#,
            );
            let id = silent(&signin).unwrap();
            assert_eq!((id.user.as_str(), id.is_root()), ("alice", false));
        }

        #[test]
        fn root_is_noticed() {
            let id = silent(&Signin::fake("printf 'root\\n0\\n'")).unwrap();
            assert!(id.is_root());
        }

        #[test]
        fn a_server_that_offers_passwords_asks_for_one() {
            let signin = Signin::fake(
                "echo 'alice@gw: Permission denied (publickey,password).' >&2; exit 255",
            );
            assert_eq!(silent(&signin), Err(SigninError::NeedsPassword));
        }

        #[test]
        fn a_keys_only_server_is_an_error_not_a_prompt() {
            let signin =
                Signin::fake("echo 'alice@gw: Permission denied (publickey).' >&2; exit 255");
            assert_eq!(
                silent(&signin),
                Err(SigninError::KeysOnly(vec!["publickey".into()]))
            );
        }

        #[test]
        fn the_password_goes_through_askpass_and_never_through_the_arguments() {
            let signin = Signin::fake(
                r#"case "$*" in *hunter2*) echo leaked >&2; exit 9 ;; esac
case "$*" in *BatchMode=no*) ;; *) exit 8 ;; esac
case "$*" in *PreferredAuthentications=password,keyboard-interactive*) ;; *) exit 7 ;; esac
[ "$SSH_ASKPASS_REQUIRE" = force ] || exit 6
[ "$SSH_ASKPASS" = /opt/lanlord/askpass-test ] || exit 5
[ "$LANLORD_ASKPASS_SECRET" = hunter2 ] || exit 4
printf 'alice\n1000\n'"#,
            );
            let how = Attempt {
                password: Some("hunter2"),
                trust_new_host: false,
            };
            assert_eq!(signin.attempt("alice@gw", &how).unwrap().user, "alice");
        }

        #[test]
        fn a_rejected_password_is_reported_as_such() {
            let signin = Signin::fake(
                "echo 'alice@gw: Permission denied (publickey,password).' >&2; exit 255",
            );
            let how = Attempt {
                password: Some("nope"),
                trust_new_host: false,
            };
            assert_eq!(
                signin.attempt("alice@gw", &how),
                Err(SigninError::WrongPassword)
            );
        }

        #[test]
        fn a_password_needs_the_helper_program() {
            let mut signin = Signin::fake("exit 0");
            signin.askpass = None;
            let how = Attempt {
                password: Some("x"),
                trust_new_host: false,
            };
            assert!(matches!(
                signin.attempt("alice@gw", &how),
                Err(SigninError::Other(_))
            ));
        }

        #[test]
        fn a_new_host_is_only_recorded_when_the_user_agreed() {
            let signin = Signin::fake(
                r#"case "$*" in
  *StrictHostKeyChecking=accept-new*) printf 'alice\n1000\n' ;;
  *) echo 'Host key verification failed.' >&2; exit 255 ;;
esac"#,
            );
            assert_eq!(silent(&signin), Err(SigninError::HostKeyUnknown));
            let trust = Attempt {
                password: None,
                trust_new_host: true,
            };
            assert!(signin.attempt("alice@gw", &trust).is_ok());
        }

        #[test]
        fn a_changed_host_key_is_never_accepted() {
            let signin = Signin::fake(
                "echo '@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @' >&2; exit 255",
            );
            assert_eq!(silent(&signin), Err(SigninError::HostKeyChanged));
        }

        #[test]
        fn the_remote_check_failing_is_reported() {
            let signin = Signin::fake("echo 'no id here' >&2; exit 127");
            let err = silent(&signin).unwrap_err().to_string();
            assert!(
                err.contains("status 127") && err.contains("no id here"),
                "{err}"
            );
            let signal = Signin::fake("kill -9 $$");
            assert!(matches!(silent(&signal), Err(SigninError::Other(_))));
        }

        #[test]
        fn the_fingerprint_is_read_without_credentials() {
            let signin = Signin::fake(
                r#"case "$*" in *PreferredAuthentications=none*) ;; *) exit 9 ;; esac
case "$*" in *UserKnownHostsFile=/dev/null*) ;; *) exit 8 ;; esac
echo 'debug1: Server host key: ssh-ed25519 SHA256:AbCd' >&2
echo 'alice@gw: Permission denied (none).' >&2
exit 255"#,
            );
            assert_eq!(
                signin.fingerprint("alice@gw").unwrap(),
                "ssh-ed25519 SHA256:AbCd"
            );
        }

        #[test]
        fn a_fingerprint_that_cannot_be_read_says_why() {
            let refused = Signin::fake("echo 'connect: Connection refused' >&2; exit 255");
            assert_eq!(refused.fingerprint("alice@gw"), Err(SigninError::Refused));
            let odd = Signin::fake("echo 'alice@gw: Permission denied (publickey).' >&2; exit 255");
            assert!(matches!(
                odd.fingerprint("alice@gw"),
                Err(SigninError::Other(_))
            ));
        }

        #[test]
        fn signing_out_closes_the_shared_connection() {
            let record =
                std::env::temp_dir().join(format!("lanlord-signout-{}", std::process::id()));
            let signin = Signin::fake(&format!("echo \"$*\" > '{}'", record.display()));
            signin.sign_out("alice@gw");
            let sent = std::fs::read_to_string(&record).unwrap();
            assert!(
                sent.contains("-O exit") && sent.contains("alice@gw"),
                "{sent}"
            );
            let _ = std::fs::remove_file(record);
        }
    }
}
