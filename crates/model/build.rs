// the version people see: a release build (LANLORD_CHANNEL=release, set by the release workflow)
// shows the plain crate version, any other build says it is a development build.
fn main() {
    println!("cargo:rerun-if-env-changed=LANLORD_CHANNEL");
    let release = std::env::var("LANLORD_CHANNEL").as_deref() == Ok("release");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let shown = if release {
        version
    } else {
        format!("{version}-development")
    };
    println!("cargo:rustc-env=LANLORD_DISPLAY_VERSION={shown}");
    println!("cargo:rustc-env=LANLORD_IS_DEVELOPMENT={}", !release);
}
