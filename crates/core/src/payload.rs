//! the files lanlord installs on a gateway, built into the binary.

use anyhow::Result;
use base64::Engine;

/// (path under /opt/lanlord/playbooks, contents)
const FILES: &[(&str, &str)] = &[
    ("site.yml", include_str!("../payload/playbooks/site.yml")),
    (
        "network.yml",
        include_str!("../payload/playbooks/network.yml"),
    ),
    (
        "docker.yml",
        include_str!("../payload/playbooks/docker.yml"),
    ),
    (
        "nt4-domain.yml",
        include_str!("../payload/playbooks/nt4-domain.yml"),
    ),
    (
        "web-proxy.yml",
        include_str!("../payload/playbooks/web-proxy.yml"),
    ),
    (
        "web-proxy/Caddyfile.j2",
        include_str!("../payload/playbooks/web-proxy/Caddyfile.j2"),
    ),
    (
        "nt4-domain/smb.conf.j2",
        include_str!("../payload/playbooks/nt4-domain/smb.conf.j2"),
    ),
    (
        "nt4-domain/image/Dockerfile",
        include_str!("../payload/playbooks/nt4-domain/image/Dockerfile"),
    ),
    (
        "nt4-domain/image/entrypoint.sh",
        include_str!("../payload/playbooks/nt4-domain/image/entrypoint.sh"),
    ),
    (
        "nt4-domain/image/lanlord-user",
        include_str!("../payload/playbooks/nt4-domain/image/lanlord-user"),
    ),
];

/// the playbooks as a tar archive.
pub fn archive() -> Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, contents) in FILES {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o640);
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
        let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, FILES.iter().map(|(n, _)| *n).collect::<Vec<_>>());
        assert!(names.contains(&"nt4-domain/image/Dockerfile"));
        assert!(found[0].1.contains("import_playbook: docker.yml"));
        let mode = tar::Archive::new(bytes.as_slice())
            .entries()
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .header()
            .mode()
            .unwrap();
        assert_eq!(mode, 0o640, "playbooks are only for the lanlord user");
        assert!(found[0].1.contains("lanlord baseline"));
    }

    #[test]
    fn every_playbook_file_on_disk_is_embedded() {
        fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, base, out);
                } else {
                    // the embedded names use forward slashes on every platform
                    let relative = path.strip_prefix(base).unwrap();
                    let parts: Vec<_> = relative
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy())
                        .collect();
                    out.push(parts.join("/"));
                }
            }
        }
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("payload/playbooks");
        let mut on_disk = Vec::new();
        walk(&base, &base, &mut on_disk);
        on_disk.sort();
        let mut embedded: Vec<String> = FILES.iter().map(|(n, _)| n.to_string()).collect();
        embedded.sort();
        assert_eq!(on_disk, embedded);
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
