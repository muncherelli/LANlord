//! the dhcp leases dnsmasq hands out, read from the gateway.

use anyhow::Result;

use crate::bootstrap::{capture_as_root, AskPassword};
use crate::config;
use crate::ssh::Session;

pub use lanlord_model::leases::*;

pub const FILE: &str = "/opt/lanlord/dns/dnsmasq.leases";

/// every network has its own dnsmasq and leases file in /opt/lanlord/dns/<name>; the single file
/// of a server from before that is read too.
const READ_DIRECT: &str = "for f in /opt/lanlord/dns/*/dnsmasq.leases /opt/lanlord/dns/dnsmasq.leases; do if [ -r \"$f\" ]; then echo present; cat /opt/lanlord/dns/*/dnsmasq.leases /opt/lanlord/dns/dnsmasq.leases 2>/dev/null; break; fi; done\n";
const READ_ROOT: &str =
    "cat /opt/lanlord/dns/*/dnsmasq.leases /opt/lanlord/dns/dnsmasq.leases 2>/dev/null || true\n";

/// the current leases on `host`; none when there is no dhcp server yet.
pub fn read_on(host: &str, ask_password: AskPassword) -> Result<Vec<Lease>> {
    read_with(&Session::new(host)?, ask_password)
}

pub(crate) fn read_with(session: &Session, ask_password: AskPassword) -> Result<Vec<Lease>> {
    let direct = session.capture(&["sh".into(), "-c".into(), READ_DIRECT.into()]);
    if let Some(body) = direct
        .ok()
        .and_then(|out| out.strip_prefix("present\n").map(String::from))
    {
        return Ok(parse(&body));
    }
    let admin = config::admin(session)?;
    let text = capture_as_root(session, &admin, READ_ROOT.to_string(), ask_password)?;
    Ok(parse(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leases_are_read_and_broken_lines_skipped() {
        let text = "1700000000 AA:BB:CC:DD:EE:FF 172.16.120.100 win98 01:aa\n0 aa:bb:cc:dd:ee:00 172.16.120.101 * *\nnonsense\n12 onlymac\n";
        let leases = parse(text);
        assert_eq!(leases.len(), 2);
        assert_eq!(leases[0].mac, "aa:bb:cc:dd:ee:ff");
        assert_eq!(leases[0].name, "win98");
        assert_eq!(leases[0].expires, 1_700_000_000);
        assert_eq!((leases[1].expires, leases[1].name.as_str()), (0, ""));
        assert!(parse("").is_empty());
    }

    #[cfg(unix)]
    mod gateway {
        use super::*;
        use crate::testgw::no_password;

        const LINE: &str = "99 aa:bb:cc:dd:ee:ff 172.16.120.100 nt4 *\n";

        fn session(direct: &str, root: &str) -> Session {
            Session::fake(&format!(
                r#"for a; do last=$a; done
case "$last" in
  "id -un") echo tester ;;
  "sudo -n true") exit 0 ;;
  "sh -c "*) {direct} ;;
  *) cat >/dev/null; {root} ;;
esac"#
            ))
        }

        #[test]
        fn a_readable_file_is_read_directly() {
            let session = session(&format!("printf 'present\\n{}'", LINE.trim_end()), "exit 1");
            let leases = read_with(&session, &mut no_password).unwrap();
            assert_eq!(leases[0].address, "172.16.120.100");
        }

        #[test]
        fn an_unreadable_or_missing_file_falls_back_to_root() {
            let session = session("exit 0", &format!("printf '{}'", LINE.trim_end()));
            assert_eq!(read_with(&session, &mut no_password).unwrap().len(), 1);
            let session = self::session("exit 1", "exit 0");
            assert!(read_with(&session, &mut no_password).unwrap().is_empty());
        }
    }
}
