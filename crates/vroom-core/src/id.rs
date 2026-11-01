//! Recording ids. They end up in public share links, so they carry 100 random
//! bits: nobody can guess one from another.

const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
pub const LEN: usize = 20;

pub fn generate() -> String {
    let mut bytes = [0u8; LEN];
    getrandom::fill(&mut bytes).expect("the OS random source is unavailable");
    bytes.iter().map(|b| ALPHABET[(b & 31) as usize] as char).collect()
}

/// Ids also name folders and object keys, so anything else is refused.
pub fn is_valid(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_are_valid_and_distinct() {
        let a = generate();
        let b = generate();
        assert_eq!(a.len(), LEN);
        assert!(is_valid(&a));
        assert_ne!(a, b);
    }

    #[test]
    fn rejects_paths() {
        assert!(!is_valid("../etc/passwd"));
        assert!(!is_valid("abc"));
        assert!(!is_valid("ABCDEFGHIJ"));
        assert!(is_valid("loom-0123456789abcdef"));
    }
}
