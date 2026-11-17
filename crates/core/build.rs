// the version the playbooks pin the component images to: see tools/image-version.sh. a release
// build gets it from LANLORD_IMAGE_VERSION; a dev build works it out from the git tags, so it
// pins the images trunk publishes. falls back to the crate version when the script cannot run
// (no sh, no git, a source tarball).
use std::process::Command;

fn main() {
    let script = "../../tools/image-version.sh";
    println!("cargo:rerun-if-env-changed=LANLORD_IMAGE_VERSION");
    println!("cargo:rerun-if-changed={script}");
    println!("cargo:rerun-if-changed=../../Cargo.toml");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/packed-refs");
    println!("cargo:rerun-if-changed=../../.git/refs/tags");
    let fallback = std::env::var("CARGO_PKG_VERSION").unwrap();
    let version = Command::new("sh")
        .arg(script)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or(fallback);
    println!("cargo:rustc-env=LANLORD_IMAGE_VERSION={version}");
}
