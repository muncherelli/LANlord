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
    let mut script = String::from("# stored restore\nrc=0\n");
    for kind in FILES {
        let file = path(kind);
        script.push_str(&format!(
            "b=$(ls {file}.v*.{stamp}.bak 2>/dev/null | head -n 1); [ -z \"$b\" ] || {{ cp -p \"$b\" {file}.tmp && mv -f {file}.tmp {file} || {{ rc=1; rm -f {file}.tmp; }}; }}\n"
        ));
    }
    script.push_str("sync\nexit $rc\n");
    script
}

/// what has to be written for one file.
struct Plan {
    kind: Kind,
    old: u32,
    new: u32,
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
        new: version_of(&new),
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
    let admins_unknown = admins.is_none();
    let context = Context { admins };
    let plain = config::pull_plain_raw_with(session, &mut *ask)?;
    let plain_version = plain
        .as_deref()
        .map(|bytes| version_of(&String::from_utf8_lossy(bytes)));
    let sealed = config::pull_raw_with(session, identity, &mut *ask)?;
    // reading all of it first: a newer or unreadable file stops everything before a write
    let plans: Vec<Plan> = [
        plan(Kind::Plain, plain, &context)?,
        plan(Kind::Sealed, sealed, &context)?,
    ]
    .into_iter()
    .flatten()
    .collect();
    // without the administrators the role assignments cannot be stored: the file stops short
    let plain_now = plans
        .iter()
        .find(|p| p.kind == Kind::Plain)
        .map(|p| p.new)
        .or(plain_version);
    let warning =
        (admins_unknown && plain_now.is_some_and(|v| v < Kind::Plain.current())).then(|| {
            format!(
                "settings.yml stopped at format {}: could not read the administrators; \
                 run lanlord upgrade again",
                plain_now.unwrap_or(1)
            )
        });
    if plans.is_empty() {
        return Ok(vec![warning.unwrap_or_else(|| {
            "the stored files are in the current format".into()
        })]);
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
                p.new,
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
        return Err(error).with_context(|| format!("storing the migrated files failed; {outcome}"));
    }
    lines.push("migrated".into());
    lines.extend(warning);
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
        std::fs::write(gw.dir.join("age"), &sealed).unwrap();
        let error =
            config::push_with(&gw.session, "gw", &id, b"a: 1\n", &mut no_password).unwrap_err();
        assert!(format!("{error:#}").contains("format 99"), "{error:#}");
    }
    #[test]
    fn a_newer_sealed_file_is_left_as_it_is_after_the_refusal() {
        let (gw, id) = (
            Gateway::new("stored-newer-age"),
            identity("stored-newer-age"),
        );
        let newer = "lanlord_format: 99\nlanlord_version: \"9.9\"\nnt4-domain: {}\n";
        let sealed = crate::crypto::encrypt(&id, newer.as_bytes()).unwrap();
        std::fs::write(gw.dir.join("age"), &sealed).unwrap();
        let error =
            config::push_with(&gw.session, "gw", &id, b"a: 1\n", &mut no_password).unwrap_err();
        assert!(format!("{error:#}").contains("format 99"), "{error:#}");
        assert_eq!(std::fs::read(gw.dir.join("age")).unwrap(), sealed);
    }

    #[test]
    fn an_old_sealed_file_is_stamped_and_keeps_a_copy() {
        let (gw, id) = (Gateway::new("stored-sealed"), identity("stored-sealed"));
        gw.seed_sealed(&id, "nt4-domain:\n  admin_password: hunter2\n");
        let before = std::fs::read(gw.dir.join("age")).unwrap();
        let lines = migrate_with(&gw.session, "gw", &id, STAMP, &mut no_password).unwrap();
        assert!(
            lines[0].starts_with("settings.age: format 1 to 1"),
            "{lines:?}"
        );
        let text = gw.sealed_raw(&id).unwrap();
        assert!(
            text.contains("lanlord_format: 1") && text.contains("hunter2"),
            "{text}"
        );
        assert_eq!(
            std::fs::read(gw.dir.join(format!("age.v1.{STAMP}.bak"))).unwrap(),
            before
        );
    }

    #[test]
    fn a_failure_part_way_puts_the_files_back() {
        let (gw, id) = (Gateway::new("stored-partway"), identity("stored-partway"));
        seed_plain(&gw, OLD);
        gw.seed_sealed(&id, "nt4-domain:\n  admin_password: hunter2\n");
        let sealed = std::fs::read(gw.dir.join("age")).unwrap();
        std::fs::write(gw.dir.join("fail-age"), "").unwrap();
        let error = migrate_with(&gw.session, "gw", &id, STAMP, &mut no_password).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("the old files are back"), "{message}");
        // the plain file was written before the sealed one failed, and is back
        assert_eq!(std::fs::read_to_string(gw.dir.join("yml")).unwrap(), OLD);
        assert_eq!(std::fs::read(gw.dir.join("age")).unwrap(), sealed);
    }
    #[test]
    fn a_restore_that_cannot_copy_says_so_and_leaves_no_temp_file() {
        let dir = std::env::temp_dir().join(format!("lanlord-restore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // a copy that cannot be read: cp fails
        std::os::unix::fs::symlink(
            dir.join("nowhere"),
            dir.join(format!("settings.yml.v1.{STAMP}.bak")),
        )
        .unwrap();
        let script = restore_script(STAMP).replace(DIR, &dir.display().to_string());
        let status = std::process::Command::new("sh")
            .args(["-c", &script])
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(1));
        assert!(!dir.join("settings.yml.tmp").exists());
        // and a restore with nothing to put back succeeds
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let status = std::process::Command::new("sh")
            .args(["-c", &script])
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_restore_that_fails_is_reported_not_called_done() {
        let (gw, id) = (
            Gateway::new("stored-restore-fails"),
            identity("stored-restore-fails"),
        );
        seed_plain(&gw, OLD);
        gw.seed_sealed(&id, "nt4-domain:\n  admin_password: hunter2\n");
        std::fs::write(gw.dir.join("fail-age"), "").unwrap();
        std::fs::write(gw.dir.join("fail-restore"), "").unwrap();
        let error = migrate_with(&gw.session, "gw", &id, STAMP, &mut no_password).unwrap_err();
        let message = format!("{error:#}");
        assert!(!message.contains("the old files are back"), "{message}");
        assert!(message.contains("put them back"), "{message}");
    }

    #[test]
    fn unknown_administrators_are_said_out_loud() {
        let (gw, id) = (
            Gateway::new("stored-no-admins"),
            identity("stored-no-admins"),
        );
        seed_plain(&gw, OLD);
        std::fs::write(gw.dir.join("fail-admins"), "").unwrap();
        let lines = migrate_with(&gw.session, "gw", &id, STAMP, &mut no_password).unwrap();
        let last = lines.last().unwrap();
        assert!(
            last.contains("stopped at format 2") && last.contains("run lanlord upgrade again"),
            "{lines:?}"
        );
    }
}
