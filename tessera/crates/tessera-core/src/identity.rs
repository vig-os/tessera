//! Stable product identity (see ADR-0020).
//!
//! `id = blake3(` canonical-JSON(`id_inputs`) `)`, algorithm-prefixed. The `id_inputs` are a
//! declared key→value map (default keys: `product`, `name`, `timestamp`). Hashing over the
//! canonical (RFC 8785 JCS) bytes makes the id reproducible by any implementation and
//! independent of map ordering. Identity is **logical**: it survives a rename or a byte-level
//! re-encode (those don't touch the inputs), and re-deriving the same logical product yields
//! the same id — while different identity inputs yield a different id. It is deliberately
//! *not* the Merkle root (that is `content_hash`, the data fingerprint).

use std::collections::BTreeMap;

/// The default declared identity-input keys, in conceptual order. Schemas may extend identity
/// with additional keys later (e.g. a device serial); whatever is in the map is what's hashed.
pub const DEFAULT_ID_INPUT_KEYS: [&str; 3] = ["product", "name", "timestamp"];

/// Build the default identity-input map from the core identity-bearing fields.
pub fn default_id_inputs(product: &str, name: &str, timestamp: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("product".to_string(), product.to_string()),
        ("name".to_string(), name.to_string()),
        ("timestamp".to_string(), timestamp.to_string()),
    ])
}

/// Compute a stable id from a declared identity-input map: `blake3:` over its canonical
/// (RFC 8785 JCS) bytes. Infallible in practice (string maps always canonicalize).
pub fn compute_id(id_inputs: &BTreeMap<String, String>) -> crate::Result<String> {
    crate::canonical::hash(id_inputs)
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
        let a = compute_id(&default_id_inputs("recon", "DP06", "2023-12-08T00:00:00Z")).unwrap();
        let b = compute_id(&default_id_inputs("recon", "DP06", "2023-12-08T00:00:00Z")).unwrap();
        let c = compute_id(&default_id_inputs("recon", "DP07", "2023-12-08T00:00:00Z")).unwrap();
        assert!(a.starts_with("blake3:"));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn id_is_canonical_not_insertion_ordered() {
        // A map built in a different insertion order must hash identically (JCS sorts keys).
        let mut m = BTreeMap::new();
        m.insert("timestamp".to_string(), "2023-12-08T00:00:00Z".to_string());
        m.insert("name".to_string(), "DP06".to_string());
        m.insert("product".to_string(), "recon".to_string());
        assert_eq!(
            compute_id(&m).unwrap(),
            compute_id(&default_id_inputs("recon", "DP06", "2023-12-08T00:00:00Z")).unwrap()
        );
    }

    #[test]
    fn timestamp_offsets_normalise_to_same_instant() {
        assert_eq!(
            normalize_timestamp("2023-12-08T01:00:00+01:00"),
            normalize_timestamp("2023-12-08T00:00:00Z")
        );
    }
}
