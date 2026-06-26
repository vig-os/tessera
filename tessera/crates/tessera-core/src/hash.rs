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
///
/// ```
/// use tessera_core::hash::{digest, merkle_root};
///
/// let root = merkle_root(&[digest(b"block-0"), digest(b"block-1")]);
/// assert!(root.starts_with("blake3:"));
///
/// // order matters (the manifest fixes block order), and the root is reproducible:
/// let a = digest(b"a");
/// let b = digest(b"b");
/// assert_ne!(merkle_root(&[a.clone(), b.clone()]), merkle_root(&[b.clone(), a.clone()]));
/// assert_eq!(merkle_root(&[a.clone(), b.clone()]), merkle_root(&[a, b]));
/// ```
pub fn merkle_root(block_digests: &[String]) -> String {
    let mut peaks = Vec::new();
    for d in block_digests {
        push_peak(&mut peaks, d);
    }
    fmt_root(bag(&peaks))
}

/// One step of an inclusion proof: a sibling hash and which side it sits on relative to the running
/// accumulator. `left == true` → the sibling is the **left** child, so combine as `node(sibling, acc)`;
/// `false` → `node(acc, sibling)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofStep {
    pub sibling: [u8; 32],
    pub left: bool,
}

/// Root of the perfect subtree over leaf-hashes `[lo, hi)` (`hi - lo` is a power of two).
fn subtree_root(lh: &[[u8; 32]], lo: usize, hi: usize) -> [u8; 32] {
    if hi - lo == 1 {
        lh[lo]
    } else {
        let mid = lo + (hi - lo) / 2;
        node_hash(&subtree_root(lh, lo, mid), &subtree_root(lh, mid, hi))
    }
}

/// Push the audit path for `target` within the perfect subtree `[lo, hi)` into `out`, **bottom-up**
/// (the recurse-then-push order yields leaf→root, which is the verification order).
fn subtree_path(lh: &[[u8; 32]], lo: usize, hi: usize, target: usize, out: &mut Vec<ProofStep>) {
    if hi - lo == 1 {
        return;
    }
    let mid = lo + (hi - lo) / 2;
    if target < mid {
        subtree_path(lh, lo, mid, target, out);
        out.push(ProofStep {
            sibling: subtree_root(lh, mid, hi),
            left: false,
        });
    } else {
        subtree_path(lh, mid, hi, target, out);
        out.push(ProofStep {
            sibling: subtree_root(lh, lo, mid),
            left: true,
        });
    }
}

/// The MMR peak leaf-ranges for `n` leaves, left→right (largest/oldest first) — the set bits of `n`,
/// matching [`push_peak`]'s carry.
fn peak_ranges(n: usize) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut pos = 0;
    for bit in (0..usize::BITS).rev() {
        let size = 1usize << bit;
        if n & size != 0 {
            ranges.push((pos, pos + size));
            pos += size;
        }
    }
    ranges
}

/// Build an **inclusion proof** that the leaf at `index` is in the MMR over the ordered `block_digests`
/// — the audit path from `leaf(index)` up to `content_hash` (ADR-0028 §6). `None` if `index` is out of
/// range. Verify with [`verify_inclusion`]. The proof is the within-peak Merkle path, then the bagging
/// path (the peak to the right bagged as one sibling, then each peak to the left).
pub fn inclusion_proof(block_digests: &[String], index: usize) -> Option<Vec<ProofStep>> {
    let n = block_digests.len();
    if index >= n {
        return None;
    }
    let lh: Vec<[u8; 32]> = block_digests.iter().map(|d| leaf_hash(d)).collect();
    let peaks = peak_ranges(n);
    let k = peaks
        .iter()
        .position(|&(lo, hi)| index >= lo && index < hi)
        .expect("index in range → some peak contains it");
    let (lo, hi) = peaks[k];

    let mut proof = Vec::new();
    // 1. within-peak audit path (bottom-up) → leaf rises to its peak's root.
    subtree_path(&lh, lo, hi, index, &mut proof);

    // 2. bagging. Peak roots, left→right.
    let peak_roots: Vec<[u8; 32]> = peaks
        .iter()
        .map(|&(l, h)| subtree_root(&lh, l, h))
        .collect();
    let m = peaks.len();
    // The bag of all peaks to the RIGHT of k is one sibling (on the right), if any exist.
    if k + 1 < m {
        let mut acc = peak_roots[m - 1];
        for j in (k + 1..m - 1).rev() {
            acc = node_hash(&peak_roots[j], &acc);
        }
        proof.push(ProofStep {
            sibling: acc,
            left: false,
        });
    }
    // Each peak to the LEFT of k wraps the accumulator as the left sibling, nearest-first.
    for j in (0..k).rev() {
        proof.push(ProofStep {
            sibling: peak_roots[j],
            left: true,
        });
    }
    Some(proof)
}

/// Verify an [`inclusion_proof`]: fold `block_digest`'s leaf up through the proof and check it
/// reproduces `root` (a `content_hash`). Tampering with the leaf, any sibling, or any side fails.
///
/// This proves **membership** (the leaf is *somewhere* in the tree with this root), not membership *at a
/// specific index* — the proof's side-pattern fixes the position, but this function does not take an
/// `index`/`n` to cross-check it. A caller that needs position-binding should regenerate the proof for
/// the expected `index` via [`inclusion_proof`] and compare, or carry the index alongside.
pub fn verify_inclusion(block_digest: &str, proof: &[ProofStep], root: &str) -> bool {
    let mut acc = leaf_hash(block_digest);
    for step in proof {
        acc = if step.left {
            node_hash(&step.sibling, &acc)
        } else {
            node_hash(&acc, &step.sibling)
        };
    }
    fmt_root(acc) == root
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
#[derive(Clone, Debug)]
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
    fn inclusion_proofs_verify_for_every_leaf() {
        // Sizes spanning several peak structures: 11 = 8+2+1, 7 = 4+2+1, 5 = 4+1, 13 = 8+4+1, …
        for n in [1usize, 2, 3, 4, 5, 6, 7, 8, 11, 13, 16] {
            let digests: Vec<String> = (0..n as u8).map(|k| digest(&[k, n as u8])).collect();
            let root = merkle_root(&digests);
            for i in 0..n {
                let proof = inclusion_proof(&digests, i).unwrap();
                assert!(
                    verify_inclusion(&digests[i], &proof, &root),
                    "n={n} i={i}: valid proof must verify against the content_hash"
                );
                // the same proof must NOT verify a different leaf at that position
                assert!(
                    !verify_inclusion(&digest(b"not-the-leaf"), &proof, &root),
                    "n={n} i={i}: a wrong leaf must not verify"
                );
            }
            assert!(
                inclusion_proof(&digests, n).is_none(),
                "out-of-range → None"
            );
        }
    }

    #[test]
    fn tampering_with_a_proof_step_fails_verification() {
        let digests: Vec<String> = (0..7u8).map(|k| digest(&[k])).collect();
        let root = merkle_root(&digests);
        let mut proof = inclusion_proof(&digests, 3).unwrap();
        assert!(verify_inclusion(&digests[3], &proof, &root));
        // corrupt a sibling hash → must fail
        proof[0].sibling[0] ^= 0xff;
        assert!(!verify_inclusion(&digests[3], &proof, &root));
        // flipping a side also breaks it (restore the hash first)
        proof[0].sibling[0] ^= 0xff;
        proof[0].left = !proof[0].left;
        assert!(!verify_inclusion(&digests[3], &proof, &root));
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
