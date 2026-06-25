//! Content hashing — blake3 over payloads, rolled into a Merkle root for the product.
//!
//! blake3 is chosen over SHA-256: it is faster and natively tree/Merkle-structured, which
//! matches the per-block → product roll-up we need for immutability + integrity.

/// Hash a byte slice, returning an algorithm-prefixed hex digest (`"blake3:<hex>"`).
pub fn digest(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

/// Roll an *ordered* list of per-block digests into a single product Merkle root.
/// Order-sensitive: the manifest fixes block order, so the root is reproducible.
pub fn merkle_root(block_digests: &[String]) -> String {
    let mut h = blake3::Hasher::new();
    for d in block_digests {
        h.update(d.as_bytes());
    }
    format!("blake3:{}", h.finalize().to_hex())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_prefixed_and_stable() {
        assert!(digest(b"hello").starts_with("blake3:"));
        assert_eq!(digest(b"hello"), digest(b"hello"));
        assert_ne!(digest(b"hello"), digest(b"world"));
    }

    #[test]
    fn merkle_is_order_sensitive() {
        let a = digest(b"a");
        let b = digest(b"b");
        assert_ne!(merkle_root(&[a.clone(), b.clone()]), merkle_root(&[b, a]));
    }
}
