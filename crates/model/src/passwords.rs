//! random passwords that are easy to read out and type on an old keyboard.

/// uppercase letters and digits with the look-alikes removed: no O/0,
/// I/L/1, B/8, S/5, Z/2 or U/V.
pub const ALPHABET: &str = "ACDEFGHJKMNPQRTWXY3479";

pub const DEFAULT_LEN: usize = 12;

/// the largest multiple of the alphabet size that fits in a byte; bytes at
/// or above it are thrown away so every character is equally likely.
const LIMIT: usize = 256 / ALPHABET.len() * ALPHABET.len();

/// a password of `len` characters (`DEFAULT_LEN` when zero), with the random bytes coming from
/// `fill`; the caller picks the source (the os on a desktop, the browser's crypto on the web).
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
