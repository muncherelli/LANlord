// the version the playbooks pin the component images to: see tools/image-version.sh. a release
// build gets it from LANLORD_IMAGE_VERSION; a dev build works it out from the git tags, so it
// pins the images trunk publishes. falls back to the crate version when the script cannot run
// (no sh, no git, a source tarball).
use std::path::{Path, PathBuf};
use std::process::Command;

// the output of a git command, when it works
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !text.is_empty()).then_some(text)
}

// only paths that exist: cargo reruns the build script on a missing one every time
fn watch(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn main() {
    let script = "../../tools/image-version.sh";
    println!("cargo:rerun-if-env-changed=LANLORD_IMAGE_VERSION");
    watch(Path::new(script));
    watch(Path::new("../../Cargo.toml"));
    // a worktree has a .git file: HEAD lives in its own git dir, the tags in the common one
    let cwd = std::env::current_dir().unwrap();
    let resolve = |p: String| -> PathBuf { cwd.join(p) };
    if let Some(dir) = git(&["rev-parse", "--git-dir"]).map(resolve) {
        watch(&dir.join("HEAD"));
    }
    if let Some(common) = git(&["rev-parse", "--git-common-dir"]).map(resolve) {
        watch(&common.join("packed-refs"));
        watch(&common.join("refs/tags"));
        // a commit on a branch moves the branch ref, not HEAD. a ref that is only in packed-refs is
        // not watched until it becomes a loose ref (the first commit on it does that)
        if let Some(head) = git(&["symbolic-ref", "-q", "HEAD"]) {
            watch(&common.join(head));
        }
    }
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
