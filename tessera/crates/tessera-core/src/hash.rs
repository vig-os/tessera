//! Content hashing — blake3 over payloads, rolled into a recursive **Merkle Mountain Range** root.
//!
//! blake3 is chosen over SHA-256: faster and natively tree-structured, matching the per-block →
//! product roll-up we need for immutability + integrity.
//!
//! The product `content_hash` is the **MMR root** over the ordered per-block digests (ADR-0028,
//! superseding the flat-list `content_hash` of ADR-0020). "Recursive node-hash applied at every
//! level" — leaves and interior nodes are **domain-separated** (`0x00` leaf, `0x01` node) so the
//! construction is second-preimage resistant (an attacker cannot pass an interior hash off as a
//! leaf). The tree is built MMR-style — a list of **peaks** (roots of complete perfect subtrees)
//! merged on a binary carry as leaves arrive — which makes the streaming root identical to the batch
//! root at every watermark *and* yields cheap inclusion/consistency proofs (a later ADR-0028 step).

/// blake3 of a byte slice → algorithm-prefixed hex digest (`"blake3:<hex>"`). Block digests use this.
pub fn digest(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

/// Domain-separated **leaf** hash of a block digest string (`0x00` prefix).
fn leaf_hash(block_digest: &str) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&[0x00]);
    h.update(block_digest.as_bytes());
    *h.finalize().as_bytes()
}

/// Domain-separated **interior node** hash of an ordered child pair (`0x01` prefix).
fn node_hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&[0x01]);
    h.update(left);
    h.update(right);
    *h.finalize().as_bytes()
}

/// The MMR root of the empty product (0 blocks) — blake3 of the empty input (RFC 6962 MTH(∅)).
fn empty_root() -> [u8; 32] {
    *blake3::hash(b"").as_bytes()
}

/// `[u8; 32]` → `"blake3:<hex>"`.
fn fmt_root(root: [u8; 32]) -> String {
    format!("blake3:{}", blake3::Hash::from_bytes(root).to_hex())
}

/// Append one leaf to the MMR peak list, merging equal-height peaks on the binary carry. A peak is
/// `(height, hash)`; the existing equal-height peak is the **left** sibling of the incoming node.
fn push_peak(peaks: &mut Vec<(u32, [u8; 32])>, block_digest: &str) {
    let mut node = (0u32, leaf_hash(block_digest));
    while let Some(&(h, left)) = peaks.last() {
        if h != node.0 {
            break;
        }
        peaks.pop();
        node = (node.0 + 1, node_hash(&left, &node.1));
    }
    peaks.push(node);
}

/// "Bag the peaks" → the single MMR root. Folds the peaks **right-to-left** with the node hash, so a
/// perfect tree (one peak) returns that subtree root unchanged, and a single leaf returns its leaf
/// hash. Empty peak list → [`empty_root`].
fn bag(peaks: &[(u32, [u8; 32])]) -> [u8; 32] {
    let mut it = peaks.iter().rev();
    match it.next() {
        None => empty_root(),
        Some(&(_, last)) => {
            let mut acc = last;
            for &(_, p) in it {
                acc = node_hash(&p, &acc);
            }
            acc
        }
    }
}

/// Roll an *ordered* list of per-block digests into the product **MMR root**.
/// Order-sensitive: the manifest fixes block order, so the root is reproducible.
pub fn merkle_root(block_digests: &[String]) -> String {
    let mut peaks = Vec::new();
    for d in block_digests {
        push_peak(&mut peaks, d);
    }
    fmt_root(bag(&peaks))
}

/// Incremental **hash-on-write** MMR accumulator — the streaming-write counterpart of
/// [`merkle_root`]. A write engine pushes each block's digest the moment that block is durably
/// committed and reads the running content root at any **watermark** (the number of blocks folded
/// in so far). MMR is inherently incremental: pushing the same ordered digests yields exactly
/// `merkle_root(&digests)`, so a product sealed incrementally and one sealed in a single batch have
/// identical `content_hash` — the streaming and batch paths can never diverge.
///
/// This is the integrity core of the streaming write engine (ROADMAP P3 / S17): a crash leaves the
/// store consistent up to the last committed watermark, and recovery replays to it.
#[derive(Clone)]
pub struct MerkleAccumulator {
    /// MMR peaks `(height, hash)`, low→high index = left→right (older→newer) subtrees.
    peaks: Vec<(u32, [u8; 32])>,
    watermark: usize,
}

impl MerkleAccumulator {
    pub fn new() -> Self {
        MerkleAccumulator {
            peaks: Vec::new(),
            watermark: 0,
        }
    }

    /// Fold one block digest into the running MMR and advance the watermark by one.
    pub fn push(&mut self, block_digest: &str) {
        push_peak(&mut self.peaks, block_digest);
        self.watermark += 1;
    }

    /// The running MMR root over everything folded in so far (the value a `content_hash` would take
    /// if the product were sealed at this watermark). Non-consuming.
    pub fn root(&self) -> String {
        fmt_root(bag(&self.peaks))
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
    fn single_leaf_root_is_the_leaf_hash() {
        let d = digest(b"only");
        assert_eq!(
            merkle_root(std::slice::from_ref(&d)),
            fmt_root(leaf_hash(&d))
        );
    }

    #[test]
    fn empty_product_root_is_empty_hash() {
        assert_eq!(merkle_root(&[]), fmt_root(empty_root()));
    }

    #[test]
    fn root_is_a_recursive_tree_not_a_flat_concat() {
        // Four leaves must form the balanced tree node(node(a,b), node(c,d)) — proving the root is
        // recursive (ADR-0028), not the superseded flat blake3(concat(digests)) of ADR-0020.
        let d: Vec<String> = [b"a".as_ref(), b"b", b"c", b"d"]
            .iter()
            .map(|x| digest(x))
            .collect();
        let (la, lb, lc, ld) = (
            leaf_hash(&d[0]),
            leaf_hash(&d[1]),
            leaf_hash(&d[2]),
            leaf_hash(&d[3]),
        );
        let expected = node_hash(&node_hash(&la, &lb), &node_hash(&lc, &ld));
        assert_eq!(merkle_root(&d), fmt_root(expected));
    }

    #[test]
    fn odd_leaf_count_promotes_lone_peak() {
        // Three leaves → peaks [height-1 (a,b), height-0 (c)]; root = node(node(a,b), leaf(c)).
        let d: Vec<String> = [b"a".as_ref(), b"b", b"c"]
            .iter()
            .map(|x| digest(x))
            .collect();
        let expected = node_hash(
            &node_hash(&leaf_hash(&d[0]), &leaf_hash(&d[1])),
            &leaf_hash(&d[2]),
        );
        assert_eq!(merkle_root(&d), fmt_root(expected));
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
        // Exercised across a range that spans several binary carries (powers of two).
        let digests: Vec<String> = (0..9u8).map(|k| digest(&[k])).collect();
        let mut acc = MerkleAccumulator::new();
        for (k, d) in digests.iter().enumerate() {
            acc.push(d);
            assert_eq!(acc.root(), merkle_root(&digests[..=k]));
            assert_eq!(acc.watermark(), k + 1);
        }
    }
}
