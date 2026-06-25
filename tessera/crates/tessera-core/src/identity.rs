//! Stable product identity.
//!
//! `id = blake3(identity inputs)`, algorithm-prefixed. Stable across re-ingests as long as
//! the identity inputs (product type, name, timestamp, ...) match — so re-deriving the same
//! product yields the same id, while different content yields a different id.

/// Compute a stable id from ordered identity inputs.
/// Inputs are joined with an ASCII unit separator to avoid ambiguity.
pub fn compute_id(inputs: &[&str]) -> String {
    let joined = inputs.join("\u{1f}");
    crate::hash::digest(joined.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_stable_and_distinct() {
        let a = compute_id(&["recon", "DP06", "2023-12-08T00:00:00+00:00"]);
        let b = compute_id(&["recon", "DP06", "2023-12-08T00:00:00+00:00"]);
        let c = compute_id(&["recon", "DP07", "2023-12-08T00:00:00+00:00"]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
