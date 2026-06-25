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
    let mut acc = MerkleAccumulator::new();
    for d in block_digests {
        acc.push(d);
    }
    acc.root()
}

/// Incremental **hash-on-write** Merkle accumulator — the streaming-write counterpart of
/// [`merkle_root`]. A write engine pushes each block's digest the moment that block is durably
/// committed and reads the running content root at any **watermark** (the number of blocks folded
/// in so far). By construction, pushing the same ordered digests yields exactly
/// `merkle_root(&digests)`, so a product sealed incrementally and one sealed in a single batch have
/// identical `content_hash` — the streaming and batch paths can never diverge.
///
/// This is the integrity core of the streaming write engine (ROADMAP P3 / S17): a crash leaves the
/// store consistent up to the last committed watermark, and recovery replays to it.
#[derive(Clone)]
pub struct MerkleAccumulator {
    hasher: blake3::Hasher,
    watermark: usize,
}

impl MerkleAccumulator {
    pub fn new() -> Self {
        MerkleAccumulator {
            hasher: blake3::Hasher::new(),
            watermark: 0,
        }
    }

    /// Fold one block digest into the running root and advance the watermark by one.
    pub fn push(&mut self, block_digest: &str) {
        self.hasher.update(block_digest.as_bytes());
        self.watermark += 1;
    }

    /// The running content Merkle root over everything folded in so far (the value a `content_hash`
    /// would take if the product were sealed at this watermark). Non-consuming.
    pub fn root(&self) -> String {
        format!("blake3:{}", self.hasher.clone().finalize().to_hex())
    }

    /// How many block digests have been committed (the crash-recovery watermark).
    pub fn watermark(&self) -> usize {
        self.watermark
    }

    pub fn is_empty(&self) -> bool {
        self.watermark == 0
    }
}

impl Default for MerkleAccumulator {
    fn default() -> Self {
        Self::new()
    }
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

    #[test]
    fn incremental_accumulator_matches_batch_root() {
        let digests: Vec<String> = [b"x".as_ref(), b"y", b"z"]
            .iter()
            .map(|d| digest(d))
            .collect();
        let mut acc = MerkleAccumulator::new();
        assert!(acc.is_empty());
        for d in &digests {
            acc.push(d);
        }
        // streaming root == batch root, and the watermark counts committed blocks
        assert_eq!(acc.root(), merkle_root(&digests));
        assert_eq!(acc.watermark(), 3);
        // empty accumulator == empty-list root (a 0-block product)
        assert_eq!(MerkleAccumulator::new().root(), merkle_root(&[]));
    }

    #[test]
    fn watermark_root_is_consistent_at_each_step() {
        // The running root at watermark k equals the batch root over the first k digests — so
        // crash-recovery to a watermark yields exactly the root a batch seal would have produced.
        let digests: Vec<String> = (0..5u8).map(|k| digest(&[k])).collect();
        let mut acc = MerkleAccumulator::new();
        for (k, d) in digests.iter().enumerate() {
            acc.push(d);
            assert_eq!(acc.root(), merkle_root(&digests[..=k]));
            assert_eq!(acc.watermark(), k + 1);
        }
    }
}
