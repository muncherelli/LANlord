//! the version a build shows: 0.1.5 for a release, 0.1.5-development for every other build.
//! the crate version itself stays a plain X.Y.Z; it is what download urls and compatibility
//! checks use.

/// the version to show people, without the leading v.
pub const fn display() -> &'static str {
    env!("LANLORD_DISPLAY_VERSION")
}

/// true unless this build was made with `LANLORD_CHANNEL=release`.
pub const fn is_development() -> bool {
    matches!(env!("LANLORD_IS_DEVELOPMENT").as_bytes(), b"true")
}

#[cfg(test)]
mod tests {
    use super::*;

    // run once as-is (development) and once with LANLORD_CHANNEL=release to cover both channels
    #[test]
    fn display_follows_the_channel() {
        let plain = env!("CARGO_PKG_VERSION");
        if option_env!("LANLORD_CHANNEL") == Some("release") {
            assert!(!is_development());
            assert_eq!(display(), plain);
        } else {
            assert!(is_development());
            assert_eq!(display(), format!("{plain}-development"));
        }
    }
}
