//! the files lanlord installs on a gateway, built into the binary.

use anyhow::Result;
use base64::Engine;

/// a file under ansible/playbooks, paired with its path on the gateway.
macro_rules! playbook {
    ($path:literal) => {
        (
            $path,
            include_str!(concat!("../../../ansible/playbooks/", $path)),
        )
    };
}

/// (path under /opt/lanlord/playbooks, contents)
const FILES: &[(&str, &str)] = &[
    playbook!("site.yml"),
    playbook!("network.yml"),
    playbook!("docker.yml"),
    playbook!("nt4-domain.yml"),
    playbook!("web-proxy.yml"),
    playbook!("dns.yml"),
    playbook!("dns/dnsmasq.conf.j2"),
    playbook!("dns/image/Dockerfile"),
    playbook!("web-proxy/Caddyfile.j2"),
    playbook!("nt4-domain/smb.conf.j2"),
    playbook!("nt4-domain/image/Dockerfile"),
    playbook!("nt4-domain/image/entrypoint.sh"),
    playbook!("nt4-domain/image/lanlord-user"),
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
        for name in ["dns.yml", "dns/dnsmasq.conf.j2", "dns/image/Dockerfile"] {
            assert!(names.contains(&name), "{name} is embedded");
        }
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
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ansible/playbooks");
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
