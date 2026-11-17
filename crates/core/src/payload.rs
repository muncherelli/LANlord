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
    playbook!("networks.yml"),
    playbook!("network-address.yml"),
    playbook!("docker.yml"),
    playbook!("doctor-probe.sh"),
    playbook!("nt4-domain.yml"),
    playbook!("web-proxy.yml"),
    playbook!("dns.yml"),
    playbook!("dns-facts.yml"),
    playbook!("dns-render.yml"),
    playbook!("dns-network.yml"),
    playbook!("dns-migrate.yml"),
    playbook!("dns-cleanup.yml"),
    playbook!("filter_plugins/lanlord.py"),
    playbook!("docker/firewall.nft.j2"),
    playbook!("replace-container.yml"),
    playbook!("webui-login.yml"),
    playbook!("dns/dnsmasq.conf.j2"),
    playbook!("dns/image/Dockerfile"),
    playbook!("web-proxy/Caddyfile.j2"),
    playbook!("status/lanlord-status.sh"),
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
    fn the_default_web_ui_socket_passes_its_own_check() {
        // the default upstream and the assert that checks it once disagreed on the slashes, so
        // every install with the web ui failed
        let (_, proxy) = FILES
            .iter()
            .find(|(n, _)| n.ends_with("web-proxy.yml"))
            .unwrap();
        let default = "unix//run/lanlord-web/web.sock";
        assert!(proxy.contains(&format!("default('{default}')")));
        let pattern = proxy
            .split("webui_upstream is match('^")
            .skip(1)
            .filter_map(|rest| rest.split('[').next())
            .find(|p| p.contains("lanlord-web/"))
            .unwrap();
        assert!(
            default.starts_with(pattern),
            "{pattern} does not match {default}"
        );
    }

    #[test]
    fn every_embedded_yaml_file_parses() {
        // a task name ending in a colon, for example, would break every apply on the gateway
        for (name, contents) in FILES.iter().filter(|(n, _)| n.ends_with(".yml")) {
            if let Err(err) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(contents) {
                panic!("{name} is not valid yaml: {err}");
            }
        }
    }

    #[test]
    fn the_entrypoint_pins_the_builtin_group_ids() {
        let (_, entrypoint) = FILES
            .iter()
            .find(|(n, _)| *n == "nt4-domain/image/entrypoint.sh")
            .unwrap();
        let line = entrypoint
            .lines()
            .find_map(|l| l.strip_prefix("for entry in "))
            .unwrap();
        let pinned: Vec<(&str, u32)> = line
            .trim_end_matches("; do")
            .split_whitespace()
            .map(|e| {
                let (name, gid) = e.split_once(':').unwrap();
                (name, gid.parse().unwrap())
            })
            .collect();
        assert_eq!(pinned, crate::admins::BUILTIN_GROUPS);
        assert!(pinned.iter().all(|(_, gid)| (9000..=9998).contains(gid)));
    }

    #[test]
    fn the_old_group_names_are_gone() {
        // the admins group is one group everywhere; only the entrypoint may still read the old file
        let legacy =
            |line: &str| line.contains("private/domadmins") || line.contains("called domadmins");
        for (name, contents) in FILES {
            for line in contents.lines().filter(|l| !legacy(l)) {
                assert!(
                    !line.contains("lanlord-admins") && !line.contains("domadmins"),
                    "{name} still names the old group: {line}"
                );
            }
        }
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for entry in std::fs::read_dir(src).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().is_some_and(|n| n == "payload.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(
                !text.contains("lanlord-admins") && !text.contains("domadmins"),
                "{} still names the old group",
                path.display()
            );
        }
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

    #[cfg(unix)]
    fn run_status(tag: &str, with_inputs: bool) -> String {
        let dir = std::env::temp_dir().join(format!("lanlord-status-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = |name: &str| dir.join(name);
        if with_inputs {
            std::fs::write(
                path("status.conf"),
                "interface=eth1\nip=172.16.120.1\nzone=retronet\nsite=nostalgia.lan 172.16.120.20:80\n",
            )
            .unwrap();
            std::fs::write(
                path("leases"),
                "1999999999 aa:bb:cc:dd:ee:01 172.16.120.20 <script>alert(1)</script> *\n0 aa:bb:cc:dd:ee:02 172.16.120.9 w98 *\n",
            )
            .unwrap();
            std::fs::write(path("applied"), "2026-01-01 10:00:00\n").unwrap();
            std::fs::write(path("version"), "1.2.3\n").unwrap();
            std::fs::write(
                path("smb.conf"),
                "[global]\n   workgroup = RETRO\n   server role = classic primary domain controller\n",
            )
            .unwrap();
        }
        let status = std::process::Command::new("bash")
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../ansible/playbooks/status/lanlord-status.sh"),
            )
            .env("LANLORD_STATUS_CONF", path("status.conf"))
            .env("LANLORD_STATUS_WWW", path("www"))
            .env("LANLORD_STATE_DIR", &dir)
            .env("LANLORD_LEASES", path("leases"))
            .env("LANLORD_SMB_CONF", path("smb.conf"))
            .env("LANLORD_DOCKER", "lanlord-no-such-docker")
            .env("LANLORD_IP", "lanlord-no-such-ip")
            .env("LANLORD_SYSTEMCTL", "lanlord-no-such-systemctl")
            .status()
            .unwrap();
        assert!(status.success());
        let page = std::fs::read_to_string(path("www/index.html")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        page
    }

    #[cfg(unix)]
    #[test]
    fn status_page_escapes_lease_hostnames() {
        let page = run_status("full", true);
        assert!(page.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!page.contains("<script>"));
        assert!(page.contains("172.16.120.9"));
        assert!(page.contains("http://nostalgia.lan/"));
        assert!(page.contains("RETRO"));
        assert!(page.contains("1.2.3"));
    }

    #[cfg(unix)]
    #[test]
    fn status_page_renders_without_inputs() {
        let page = run_status("empty", false);
        assert!(page.contains("<title>LANlord"));
        assert!(page.contains("No machines were found"));
        assert!(page.contains("No NT4 domain is configured"));
        assert!(page.contains("No sites are hosted"));
    }
}
