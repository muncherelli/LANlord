//! random passwords that are easy to read out and type on an old keyboard.

/// uppercase letters and digits with the look-alikes removed: no O/0,
/// I/L/1, B/8, S/5, Z/2 or U/V.
pub const ALPHABET: &str = "ACDEFGHJKMNPQRTWXY3479";

pub const DEFAULT_LEN: usize = 12;

/// the largest multiple of the alphabet size that fits in a byte; bytes at
/// or above it are thrown away so every character is equally likely.
const LIMIT: usize = 256 / ALPHABET.len() * ALPHABET.len();

/// a password of `len` characters (`DEFAULT_LEN` when zero) from the
/// operating system's cryptographic random source.
pub fn generate(len: usize) -> String {
    generate_with(len, |bytes| {
        getrandom::fill(bytes).expect("the operating system has no random source")
    })
}

/// like `generate`, with the random bytes coming from `fill`.
pub fn generate_with(len: usize, mut fill: impl FnMut(&mut [u8])) -> String {
    let len = if len == 0 { DEFAULT_LEN } else { len };
    let alphabet = ALPHABET.as_bytes();
    let mut out = String::with_capacity(len);
    let mut bytes = [0u8; 32];
    while out.len() < len {
        fill(&mut bytes);
        for byte in bytes {
            if out.len() < len && usize::from(byte) < LIMIT {
                out.push(alphabet[usize::from(byte) % alphabet.len()] as char);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_alphabet_has_no_look_alikes() {
        for c in "O0IL1B8S5Z2UV".chars() {
            assert!(!ALPHABET.contains(c), "{c}");
        }
        assert_eq!(ALPHABET.len(), 22);
        assert!(ALPHABET
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()));
    }

    #[test]
    fn output_stays_in_the_alphabet_and_uses_all_of_it() {
        let text = generate(10_000);
        assert_eq!(text.len(), 10_000);
        assert!(text.chars().all(|c| ALPHABET.contains(c)));
        assert!(ALPHABET.chars().all(|c| text.contains(c)));
    }

    #[test]
    fn the_default_length_is_twelve_and_calls_differ() {
        assert_eq!(generate(0).len(), 12);
        assert_ne!(generate(24), generate(24));
    }

    #[test]
    fn the_given_source_is_the_one_used_and_biased_bytes_are_dropped() {
        let mut calls = 0;
        let text = generate_with(3, |bytes| {
            calls += 1;
            // 255 and 242 are dropped; 0, 1 and 22 map to A, C and A
            bytes.fill(255);
            bytes[..4].copy_from_slice(&[242, 0, 1, 22]);
        });
        assert_eq!(text, "ACA");
        assert_eq!(calls, 1);
    }
}
