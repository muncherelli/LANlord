// the version people see: a release build (LANLORD_CHANNEL=release, set by the release workflow)
// shows the plain crate version; any other build shows the next patch with "-development", since
// Cargo.toml holds the last release (see version_rule.rs).
include!("version_rule.rs");

fn main() {
    println!("cargo:rerun-if-env-changed=LANLORD_CHANNEL");
    println!("cargo:rerun-if-changed=version_rule.rs");
    let release = std::env::var("LANLORD_CHANNEL").as_deref() == Ok("release");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    match shown(&version, release) {
        Ok(shown) => {
            println!("cargo:rustc-env=LANLORD_DISPLAY_VERSION={shown}");
            println!("cargo:rustc-env=LANLORD_IS_DEVELOPMENT={}", !release);
        }
        Err(why) => {
            eprintln!("error: {why}");
            std::process::exit(1);
        }
    }
}
