// how the shown version follows from the crate version and the channel. shared by build.rs (which
// bakes it in) and the unit tests in src/version.rs, so both run the same code.

/// the version a development build shows: the next patch of the crate version, plus
/// "-development", so 0.1.4 (the last release) becomes 0.1.5-development. anything that is not a
/// plain X.Y.Z is an error, never a guess.
pub fn development(version: &str) -> Result<String, String> {
    let parts: Vec<&str> = version.split('.').collect();
    let numbers: Option<Vec<u64>> = match parts.as_slice() {
        [_, _, _] => parts
            .iter()
            .map(|p| {
                let plain = !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
                if plain { p.parse().ok() } else { None }
            })
            .collect(),
        _ => None,
    };
    match numbers.as_deref() {
        Some(&[major, minor, patch]) => match patch.checked_add(1) {
            Some(next) => Ok(format!("{major}.{minor}.{next}-development")),
            None => Err(format!("the patch of version {version} cannot be raised")),
        },
        _ => Err(format!(
            "the crate version is {version:?}, but a development build needs a plain X.Y.Z to \
             work out the next patch from (Cargo.toml always holds the last release)"
        )),
    }
}

/// the version a build shows: the plain crate version for a release, `development` otherwise.
pub fn shown(version: &str, release: bool) -> Result<String, String> {
    if release {
        Ok(version.to_string())
    } else {
        development(version)
    }
}
