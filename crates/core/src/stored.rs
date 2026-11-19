//! bring the files a gateway stores up to the format this lanlord writes, once, as part of an
//! upgrade. it runs where the gateway's key is (the operations service): decrypt, migrate,
//! seal again. every file that changes is first copied beside itself as
//! `<file>.v<old version>.<stamp>.bak`, and when anything fails the copies are put back.
//! see docs/stored-formats.md.

use anyhow::{bail, Context as _, Result};
use lanlord_model::format::Context;
use lanlord_model::settings::Settings;
use lanlord_model::stored::{self, Kind};

use crate::bootstrap::{run_as_root, AskPassword};
use crate::{admins, config};

const DIR: &str = "/opt/lanlord/config";

/// the files this lanlord stores on a gateway, in the order they are written.
const FILES: [Kind; 2] = [Kind::Plain, Kind::Sealed];

/// the stamp of an upgrade's backups: the gateway's clock as `20260101T120000Z`.
pub fn stamp_script() -> String {
    "# stored stamp\ndate -u +%Y%m%dT%H%M%SZ\n".to_string()
}

fn path(kind: Kind) -> String {
    format!("{DIR}/{}", kind.file())
}

/// the script that copies the files of `old` (the kind and the version it is in) beside
/// themselves.
fn backup_script(stamp: &str, old: &[(Kind, u32)]) -> String {
    let mut script = String::from("# stored backup\nset -e\n");
    for (kind, version) in old {
        let file = path(*kind);
        script.push_str(&format!("cp -p {file} {file}.v{version}.{stamp}.bak\n"));
    }
    script
}

/// the script that puts every file back from the copies an upgrade with `stamp` made. a file
/// with no copy was not changed and is left alone.
pub fn restore_script(stamp: &str) -> String {
    let mut script = String::from("# stored restore\n");
    for kind in FILES {
        let file = path(kind);
        script.push_str(&format!(
            "b=$(ls {file}.v*.{stamp}.bak 2>/dev/null | head -n 1); [ -z \"$b\" ] || cp -p \"$b\" {file}\n"
        ));
    }
    script
}

/// what has to be written for one file.
struct Plan {
    kind: Kind,
    old: u32,
    text: String,
}

fn plan(kind: Kind, stored: Option<Vec<u8>>, context: &Context) -> Result<Option<Plan>> {
    let Some(bytes) = stored else {
        return Ok(None);
    };
    let text = String::from_utf8(bytes).with_context(|| format!("{} is not text", kind.file()))?;
    let loaded = stored::load(kind, &text, context).with_context(|| kind.file().to_string())?;
    let old = version_of(&text);
    let new = loaded.text();
    Ok((new != text).then_some(Plan {
        kind,
        old,
        text: new,
    }))
}

/// the version a text says it has (1 when it says nothing).
fn version_of(text: &str) -> u32 {
    Settings::parse(text)
        .ok()
        .and_then(|settings| settings.format().ok())
        .and_then(|(version, _)| version)
        .unwrap_or(1)
}

/// migrate the stored settings. it says what it did, one line each; nothing is written (and
/// nothing is copied) when everything is current already, and a file from a newer lanlord stops
/// it before anything is touched.
pub(crate) fn migrate_with(
    session: &crate::ssh::Session,
    host: &str,
    identity: &age::x25519::Identity,
    stamp: &str,
    ask: AskPassword,
) -> Result<Vec<String>> {
    if !lanlord_model::format::backup_stamp(stamp) {
        bail!("the backup stamp must look like 20260101T120000Z");
    }
    let admins = admins::list_with(session, &mut *ask).ok();
    let context = Context { admins };
    let plain = config::pull_plain_raw_with(session, &mut *ask)?;
    let sealed = config::pull_raw_with(session, identity, &mut *ask)?;
    // reading all of it first: a newer or unreadable file stops everything before a write
    let plans: Vec<Plan> = [
        plan(Kind::Plain, plain, &context)?,
        plan(Kind::Sealed, sealed, &context)?,
    ]
    .into_iter()
    .flatten()
    .collect();
    if plans.is_empty() {
        return Ok(vec!["the stored files are in the current format".into()]);
    }
    let old: Vec<(Kind, u32)> = plans.iter().map(|p| (p.kind, p.old)).collect();
    let admin = config::admin(session)?;
    if run_as_root(session, &admin, backup_script(stamp, &old), &mut *ask)? != 0 {
        bail!("could not keep copies of the stored files; nothing was changed");
    }
    let mut lines: Vec<String> = plans
        .iter()
        .map(|p| {
            format!(
                "{}: format {} to {}; the old copy is {}.v{}.{stamp}.bak",
                p.kind.file(),
                p.old,
                p.kind.current(),
                path(p.kind),
                p.old
            )
        })
        .collect();
    let written = plans.iter().try_for_each(|p| match p.kind {
        Kind::Plain => config::push_plain_with(session, host, p.text.as_bytes(), &mut *ask),
        Kind::Sealed => config::push_with(session, host, identity, p.text.as_bytes(), &mut *ask),
    });
    if let Err(error) = written {
        let code = run_as_root(session, &admin, restore_script(stamp), &mut *ask);
        let outcome = match code {
            Ok(0) => "the old files are back".to_string(),
            _ => {
                format!("the old copies are beside the files ({DIR}/*.{stamp}.bak); put them back")
            }
        };
        return Err(error.context(format!("storing the migrated files failed; {outcome}")));
    }
    lines.push("migrated".into());
    Ok(lines)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::testgw::{identity, no_password, Gateway};

    const STAMP: &str = "20260101T120000Z";
    const OLD: &str = "docker:\n  internal_interface: eth1\n  internal_address: 192.0.2.1/24\ndns:\n  zone: retronet\n";

    fn seed_plain(gw: &Gateway, text: &str) {
        std::fs::write(gw.dir.join("yml"), text).unwrap();
    }

    #[test]
    fn old_files_are_migrated_once_and_keep_a_copy() {
        let (gw, id) = (Gateway::new("stored-migrate"), identity("stored-migrate"));
        seed_plain(&gw, OLD);
        let lines = migrate_with(&gw.session, "gw", &id, STAMP, &mut no_password).unwrap();
        assert!(
            lines[0].starts_with("settings.yml: format 1 to "),
            "{lines:?}"
        );
        let stored = std::fs::read_to_string(gw.dir.join("yml")).unwrap();
        assert!(stored.contains("networks:"), "{stored}");
        let copy = std::fs::read_to_string(gw.dir.join(format!("yml.v1.{STAMP}.bak"))).unwrap();
        assert_eq!(copy, OLD);
        let again = migrate_with(&gw.session, "gw", &id, STAMP, &mut no_password).unwrap();
        assert_eq!(again, ["the stored files are in the current format"]);
    }

    #[test]
    fn a_restore_puts_the_old_copy_back() {
        let (gw, id) = (Gateway::new("stored-restore"), identity("stored-restore"));
        seed_plain(&gw, OLD);
        migrate_with(&gw.session, "gw", &id, STAMP, &mut no_password).unwrap();
        let admin = config::admin(&gw.session).unwrap();
        let code = run_as_root(&gw.session, &admin, restore_script(STAMP), &mut no_password);
        assert_eq!(code.unwrap(), 0);
        assert_eq!(std::fs::read_to_string(gw.dir.join("yml")).unwrap(), OLD);
    }

    #[test]
    fn a_newer_file_stops_everything_before_a_write() {
        let (gw, id) = (Gateway::new("stored-newer"), identity("stored-newer"));
        let newer = "lanlord_format: 99\nlanlord_version: \"9.9\"\ndns: {}\n";
        seed_plain(&gw, newer);
        let error = migrate_with(&gw.session, "gw", &id, STAMP, &mut no_password).unwrap_err();
        assert!(
            format!("{error:#}")
                .contains("written by LANlord 9.9 (format 99); upgrade LANlord to use it"),
            "{error:#}"
        );
        assert_eq!(std::fs::read_to_string(gw.dir.join("yml")).unwrap(), newer);
        assert!(!gw.dir.join(format!("yml.v99.{STAMP}.bak")).exists());
    }
    #[test]
    fn a_save_never_overwrites_files_from_a_newer_lanlord() {
        let (gw, id) = (Gateway::new("stored-save"), identity("stored-save"));
        let newer = "lanlord_format: 99\nlanlord_version: \"9.9\"\ndns: {}\n";
        seed_plain(&gw, newer);
        let error =
            config::push_plain_with(&gw.session, "gw", b"dns: {}\n", &mut no_password).unwrap_err();
        assert!(format!("{error:#}").contains("format 99"), "{error:#}");
        assert_eq!(std::fs::read_to_string(gw.dir.join("yml")).unwrap(), newer);
        let sealed = crate::crypto::encrypt(&id, newer.as_bytes()).unwrap();
        std::fs::write(gw.dir.join("age"), sealed).unwrap();
        let error =
            config::push_with(&gw.session, "gw", &id, b"a: 1\n", &mut no_password).unwrap_err();
        assert!(format!("{error:#}").contains("format 99"), "{error:#}");
    }
}
