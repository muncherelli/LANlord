// how the shown version follows from the crate version and the channel. shared by build.rs (which
// bakes it in) and the unit tests in src/version.rs, so both run the same code.

/// the numbers of a plain X.Y.Z version. anything else (a pre-release, a missing part, a sign) is
/// an error, never a guess: Cargo.toml always holds a plain X.Y.Z.
pub fn parse(version: &str) -> Result<[u64; 3], String> {
    let odd = || {
        format!(
            "the crate version is {version:?}, but lanlord needs a plain X.Y.Z here \
             (Cargo.toml always holds the last release)"
        )
    };
    let parts: Vec<&str> = version.split('.').collect();
    let [major, minor, patch] = parts.as_slice() else {
        return Err(odd());
    };
    let mut numbers = [0u64; 3];
    for (slot, part) in numbers.iter_mut().zip([major, minor, patch]) {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(odd());
        }
        *slot = part.parse().map_err(|_| odd())?;
    }
    Ok(numbers)
}

/// the version a development build shows: the next patch of the crate version, plus
/// "-development", so 0.1.4 (the last release) becomes 0.1.5-development.
pub fn development(version: &str) -> Result<String, String> {
    let [major, minor, patch] = parse(version)?;
    match patch.checked_add(1) {
        Some(next) => Ok(format!("{major}.{minor}.{next}-development")),
        None => Err(format!("the patch of version {version} cannot be raised")),
    }
}

/// the version a build shows: the plain crate version for a release, `development` otherwise.
/// both refuse a version that is not a plain X.Y.Z.
pub fn shown(version: &str, release: bool) -> Result<String, String> {
    if release {
        parse(version).map(|_| version.to_string())
    } else {
        development(version)
    }
}
