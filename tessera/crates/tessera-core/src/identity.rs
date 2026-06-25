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

/// Normalise an RFC 3339 timestamp to UTC (`...Z`) so that the *same instant* written in
/// different offsets yields one id. Non-RFC3339 input is returned unchanged (lenient).
///
/// Without this, `2023-12-08T00:00:00Z` and `2023-12-08T01:00:00+01:00` — the same moment —
/// would produce two different product ids.
pub fn normalize_timestamp(ts: &str) -> String {
    use time::{format_description::well_known::Rfc3339, OffsetDateTime, UtcOffset};
    match OffsetDateTime::parse(ts, &Rfc3339) {
        Ok(dt) => dt
            .to_offset(UtcOffset::UTC)
            .format(&Rfc3339)
            .unwrap_or_else(|_| ts.to_string()),
        Err(_) => ts.to_string(),
    }
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
