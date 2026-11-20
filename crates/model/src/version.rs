//! the version a build shows: the plain 0.1.4 for a release, and for every other build the next
//! patch with a suffix, 0.1.5-development, because Cargo.toml holds the last release. the crate
//! version itself stays a plain X.Y.Z; it is what download urls and compatibility checks use.

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

    mod rule {
        include!("../version_rule.rs");
    }

    #[test]
    fn a_development_build_shows_the_next_patch() {
        assert_eq!(rule::shown("0.1.4", false).unwrap(), "0.1.5-development");
        assert_eq!(rule::shown("0.1.9", false).unwrap(), "0.1.10-development");
        assert_eq!(rule::shown("1.9.99", false).unwrap(), "1.9.100-development");
    }

    #[test]
    fn a_release_shows_the_plain_version() {
        assert_eq!(rule::shown("0.1.4", true).unwrap(), "0.1.4");
    }

    #[test]
    fn an_odd_version_is_refused_not_guessed() {
        for odd in [
            "0.1",
            "0.1.4-rc1",
            "0.1.4+b",
            "1.2.x",
            "",
            "0.1.4.1",
            "0.-1.4",
            "0.1.+4",
        ] {
            assert!(rule::development(odd).is_err(), "{odd}");
            assert!(rule::shown(odd, true).is_err(), "{odd}");
        }
    }

    // run once as-is (development) and once with LANLORD_CHANNEL=release to cover both channels
    #[test]
    fn display_follows_the_channel() {
        let plain = env!("CARGO_PKG_VERSION");
        if option_env!("LANLORD_CHANNEL") == Some("release") {
            assert!(!is_development());
            assert_eq!(display(), plain);
        } else {
            assert!(is_development());
            assert_eq!(display(), rule::development(plain).unwrap());
        }
    }
}
