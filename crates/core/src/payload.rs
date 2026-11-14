//! the files lanlord installs on a gateway, built into the binary.

use anyhow::Result;
use base64::Engine;

/// (path under /opt/lanlord/playbooks, contents)
const FILES: &[(&str, &str)] = &[("site.yml", include_str!("../payload/playbooks/site.yml"))];

/// the playbooks as a tar archive.
pub fn archive() -> Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, contents) in FILES {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_cksum();
        builder.append_data(&mut header, path, contents.as_bytes())?;
    }
    Ok(builder.into_inner()?)
}

/// the archive as base64 wrapped to 76 columns, safe to embed in a script.
pub fn archive_base64() -> Result<String> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(archive()?);
    Ok(encoded
        .as_bytes()
        .chunks(76)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn archive_contains_the_playbook() {
        let bytes = archive().unwrap();
        let mut found = Vec::new();
        for entry in tar::Archive::new(bytes.as_slice()).entries().unwrap() {
            let mut entry = entry.unwrap();
            let mut text = String::new();
            entry.read_to_string(&mut text).unwrap();
            found.push((entry.path().unwrap().display().to_string(), text));
        }
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "site.yml");
        assert!(found[0].1.contains("lanlord baseline"));
    }

    #[test]
    fn base64_round_trips() {
        let encoded = archive_base64().unwrap();
        assert!(encoded.lines().all(|l| l.len() <= 76));
        let joined: String = encoded.lines().collect();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(joined)
            .unwrap();
        assert_eq!(decoded, archive().unwrap());
    }
}
