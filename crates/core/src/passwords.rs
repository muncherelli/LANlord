//! random passwords that are easy to read out and type on an old keyboard; the alphabet and the
//! rules live in lanlord-model, the operating system's random source here.

pub use lanlord_model::passwords::*;

/// a password of `len` characters (`DEFAULT_LEN` when zero) from the
/// operating system's cryptographic random source.
pub fn generate(len: usize) -> String {
    generate_with(len, |bytes| {
        getrandom::fill(bytes).expect("the operating system has no random source")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
