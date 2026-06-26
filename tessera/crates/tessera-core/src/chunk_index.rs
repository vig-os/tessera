//! `{hash, stats}` chunk-index — the sub-block integrity + pruning structure (ADR-0028 §3, absorbing
//! ADR-0027). A block is split into ordered sub-blocks (array chunks / table row-groups); each carries a
//! content digest and a set of **monoid statistics**. The index delivers two things from one structure:
//!   - **integrity:** the block digest is the MMR root over the per-chunk digests (sub-block Merkle), so
//!     a single chunk is confirmable with a short inclusion proof — per-chunk, not whole-block;
//!   - **pruning:** a ranged/predicate read consults the per-chunk stats and skips chunks that cannot
//!     match — the same min/max/count/sum monoids that roll up into the multiscale pyramid.
//!
//! Stats are **monoids** (identity + associative `combine`): a chunk's stat is a fold over its values,
//! and a parent node's stat is the `combine` of its children — so the chunk-index, the Merkle tree, and
//! the pyramid are one fold over the data (ADR-0028 fused pass). New statistics extend the set by adding
//! a monoid; the roll-up and pruning machinery apply unchanged (the "extensible factory" of ADR-0028).

use serde::{Deserialize, Serialize};

use crate::hash::merkle_root;

/// A statistic that forms a **monoid** over a chunk's samples: an identity element and an associative
/// `combine`, so `stat(a ++ b) == combine(stat(a), stat(b))` — the law that lets chunk stats roll up a
/// tree (pyramid / aggregate) without re-reading the data.
pub trait Monoid: Sized {
    /// The identity element: `combine(identity(), x) == x == combine(x, identity())`.
    fn identity() -> Self;
    /// Associative merge: `combine(combine(a, b), c) == combine(a, combine(b, c))`.
    fn combine(&self, other: &Self) -> Self;
}

/// The built-in numeric chunk statistics over `i64` samples: `count`, `min`, `max`, `sum` (`i128` sum so
/// it never overflows). `min`/`max` are `None` only for an empty chunk. Float columns reduce canonically
/// before lifting (ADR-0024 determinism) — out of scope for this integer core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkStats {
    pub count: u64,
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub sum: i128,
}

impl ChunkStats {
    /// Fold a chunk's samples into its statistics (the leaf stat).
    pub fn from_values(values: &[i64]) -> Self {
        values.iter().fold(Self::identity(), |acc, &v| {
            acc.combine(&ChunkStats {
                count: 1,
                min: Some(v),
                max: Some(v),
                sum: v as i128,
            })
        })
    }

    /// True if some sample in the chunk *could* lie in the inclusive range `[lo, hi]` — i.e. the chunk
    /// cannot be pruned for that range. Exact for min/max pruning: never a false negative (it only ever
    /// *keeps* a chunk that might match), so pruning can never drop a real hit.
    pub fn overlaps(&self, lo: i64, hi: i64) -> bool {
        match (self.min, self.max) {
            (Some(mn), Some(mx)) => mn <= hi && mx >= lo,
            _ => false, // empty chunk contains nothing
        }
    }
}

impl Monoid for ChunkStats {
    fn identity() -> Self {
        ChunkStats {
            count: 0,
            min: None,
            max: None,
            sum: 0,
        }
    }

    fn combine(&self, other: &Self) -> Self {
        let min = match (self.min, other.min) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let max = match (self.max, other.max) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        ChunkStats {
            count: self.count + other.count,
            min,
            max,
            sum: self.sum + other.sum,
        }
    }
}

/// One entry in the chunk-index: a sub-block content digest + its statistics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkEntry {
    pub digest: String,
    pub stats: ChunkStats,
}

/// The ordered `{hash, stats}` index over a block's sub-blocks (ADR-0028 §3).
///
/// ```
/// use tessera_core::chunk_index::ChunkIndex;
///
/// let mut idx = ChunkIndex::new();
/// idx.push("blake3:chunk-a", &[1, 2, 3]); //   values in [1, 3]
/// idx.push("blake3:chunk-b", &[10, 20, 30]); // values in [10, 30]
///
/// // a ranged read for [5, 15] provably skips chunk-a (its max 3 < 5) and keeps only chunk-b:
/// assert_eq!(idx.prune(5, 15), vec![1]);
///
/// // the block's content digest is the MMR root over the per-chunk digests (ADR-0028 §1):
/// assert!(idx.root().starts_with("blake3:"));
///
/// // stats roll up: the block aggregate is the combine of every chunk's stats.
/// assert_eq!(idx.aggregate().count, 6);
/// assert_eq!(idx.aggregate().min, Some(1));
/// assert_eq!(idx.aggregate().max, Some(30));
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChunkIndex {
    pub entries: Vec<ChunkEntry>,
}

impl ChunkIndex {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Append a sub-block by its content `digest` and its samples (which fold into the leaf stat).
    pub fn push(&mut self, digest: impl Into<String>, values: &[i64]) {
        self.entries.push(ChunkEntry {
            digest: digest.into(),
            stats: ChunkStats::from_values(values),
        });
    }

    /// Append a sub-block whose stats are already computed (the streaming path — stats come off the
    /// fused encode+hash+stat pass, not a re-read).
    pub fn push_entry(&mut self, digest: impl Into<String>, stats: ChunkStats) {
        self.entries.push(ChunkEntry {
            digest: digest.into(),
            stats,
        });
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The block's content digest = the **MMR root** over the ordered per-chunk digests — the sub-block
    /// Merkle root that ties this index into the product integrity hierarchy (ADR-0028 §1/§2). A product
    /// using a chunk-index sets the block's `BlockRef.digest` to this root.
    pub fn root(&self) -> String {
        let digests: Vec<String> = self.entries.iter().map(|e| e.digest.clone()).collect();
        merkle_root(&digests)
    }

    /// The block-level aggregate stat (the level-0 pyramid root) = `combine` of every chunk stat.
    pub fn aggregate(&self) -> ChunkStats {
        self.entries
            .iter()
            .fold(ChunkStats::identity(), |acc, e| acc.combine(&e.stats))
    }

    /// Pruning: the indices of the chunks that *could* contain a value in the inclusive range
    /// `[lo, hi]`; every other chunk is provably skippable. No false negatives (see [`ChunkStats::overlaps`]).
    /// `lo <= hi` is required (an empty/inverted range matches nothing) — debug-asserted to catch caller bugs.
    pub fn prune(&self, lo: i64, hi: i64) -> Vec<usize> {
        debug_assert!(
            lo <= hi,
            "prune called with inverted range lo={lo} > hi={hi}"
        );
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.stats.overlaps(lo, hi))
            .map(|(i, _)| i)
            .collect()
    }

    /// Serialize to **deterministic** bytes (compact JSON; the struct's field order is fixed and there
    /// are no maps, so the bytes are a pure function of the index). This is the payload an emitted
    /// chunk-index block carries; its digest is `digest(to_bytes())` and feeds the product's content
    /// hash like any other block (ADR-0028 §3/§4).
    pub fn to_bytes(&self) -> crate::Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    /// Reconstruct an index from [`Self::to_bytes`] output.
    pub fn from_bytes(bytes: &[u8]) -> crate::Result<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::{digest, merkle_root};

    fn stats(vs: &[i64]) -> ChunkStats {
        ChunkStats::from_values(vs)
    }

    #[test]
    fn from_values_is_correct() {
        let s = stats(&[3, -1, 7, 7, 0]);
        assert_eq!(s.count, 5);
        assert_eq!(s.min, Some(-1));
        assert_eq!(s.max, Some(7));
        assert_eq!(s.sum, 16);
        // empty chunk → identity
        assert_eq!(stats(&[]), ChunkStats::identity());
    }

    #[test]
    fn monoid_identity_law() {
        let id = ChunkStats::identity();
        let x = stats(&[5, 2, 9]);
        assert_eq!(id.combine(&x), x);
        assert_eq!(x.combine(&id), x);
    }

    #[test]
    fn monoid_associativity_law() {
        let (a, b, c) = (stats(&[1, 2]), stats(&[-4, 8]), stats(&[3]));
        assert_eq!(a.combine(&b).combine(&c), a.combine(&b.combine(&c)));
    }

    #[test]
    fn combine_equals_recompute_over_concat() {
        // The roll-up law: stat(a ++ b) == combine(stat(a), stat(b)) — what makes one fold serve the
        // chunk-index, the Merkle tree, and the pyramid.
        let a = [3i64, 1, 4, 1, 5];
        let b = [9i64, -2, 6];
        let concat: Vec<i64> = a.iter().chain(b.iter()).copied().collect();
        assert_eq!(stats(&a).combine(&stats(&b)), stats(&concat));
    }

    #[test]
    fn pruning_keeps_overlapping_chunks_only_and_never_drops_a_hit() {
        let mut idx = ChunkIndex::new();
        idx.push(digest(b"c0"), &[0, 1, 2]); // [0,2]
        idx.push(digest(b"c1"), &[10, 11, 12]); // [10,12]
        idx.push(digest(b"c2"), &[20, 25, 30]); // [20,30]
                                                // range [11,21] overlaps c1 (11∈[10,12]) and c2 (20∈[20,30]); excludes c0.
        assert_eq!(idx.prune(11, 21), vec![1, 2]);
        // a range outside everything prunes all.
        assert_eq!(idx.prune(100, 200), Vec::<usize>::new());
        // exhaustive no-false-negative check: every value's point-range keeps its own chunk.
        for (ci, vals) in [[0, 1, 2], [10, 11, 12], [20, 25, 30]].iter().enumerate() {
            for &v in vals {
                assert!(
                    idx.prune(v, v).contains(&ci),
                    "value {v} must keep chunk {ci}"
                );
            }
        }
    }

    #[test]
    fn root_is_the_mmr_over_chunk_digests() {
        let mut idx = ChunkIndex::new();
        let (d0, d1) = (digest(b"chunk-0"), digest(b"chunk-1"));
        idx.push(d0.clone(), &[1, 2]);
        idx.push(d1.clone(), &[3, 4]);
        // the index root IS the product's sub-block Merkle root (ties to ADR-0028 §1/§2)
        assert_eq!(idx.root(), merkle_root(&[d0, d1]));
        // empty index → empty MMR root
        assert_eq!(ChunkIndex::new().root(), merkle_root(&[]));
    }

    #[test]
    fn aggregate_is_combine_of_all_chunks() {
        let mut idx = ChunkIndex::new();
        idx.push(digest(b"a"), &[1, 2, 3]);
        idx.push(digest(b"b"), &[4, 5]);
        // block-level stat == stat over the whole concatenation
        assert_eq!(idx.aggregate(), stats(&[1, 2, 3, 4, 5]));
        assert_eq!(idx.aggregate().count, 5);
        assert_eq!(idx.aggregate().min, Some(1));
        assert_eq!(idx.aggregate().max, Some(5));
    }

    #[test]
    fn index_to_bytes_is_deterministic_and_roundtrips() {
        let mut idx = ChunkIndex::new();
        idx.push(digest(b"c0"), &[1, 2, 3]);
        idx.push(digest(b"c1"), &[-5, 9]);
        let bytes = idx.to_bytes().unwrap();
        // same index → identical bytes (the block-digest determinism requirement)
        assert_eq!(idx.to_bytes().unwrap(), bytes);
        // roundtrip preserves entries and the MMR root
        let back = ChunkIndex::from_bytes(&bytes).unwrap();
        assert_eq!(back.entries, idx.entries);
        assert_eq!(back.root(), idx.root());
    }

    #[test]
    fn entry_roundtrips_through_serde() {
        let mut idx = ChunkIndex::new();
        idx.push(digest(b"x"), &[7, 8, 9]);
        let json = serde_json::to_string(&idx).unwrap();
        let back: ChunkIndex = serde_json::from_str(&json).unwrap();
        assert_eq!(back.entries, idx.entries);
        assert_eq!(back.root(), idx.root());
    }
}
