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

/// The built-in numeric chunk statistics over `i64` samples: `count`, `min`, `max`, `sum`, `sum_sq`
/// (`i128` sums so they never overflow). `min`/`max` are `None` only for an empty chunk. Float columns
/// reduce canonically before lifting (ADR-0024 determinism) — out of scope for this integer core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkStats {
    pub count: u64,
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub sum: i128,
    /// Sum of squares — an associative monoid field, so adding it costs nothing in the roll-up but
    /// unlocks `variance`/`std_dev` (ADR-0028 §3: extend the factory by adding a monoid).
    #[serde(default)]
    pub sum_sq: i128,
}

impl ChunkStats {
    /// Fold a chunk's samples into its statistics (the leaf stat), or `None` if the fold cannot be
    /// exact.
    ///
    /// **Fails closed** (ADR-0059 M2, #523). `sum_sq` accumulates in `i128`, which 64-bit samples
    /// overflow: `i64::MAX²` is ~8.5e37 against an `i128::MAX` of ~1.7e38, so three such samples are
    /// enough. Before this returned `Self` and used plain `+`, which panics in debug and — far worse
    /// — **wraps silently in release**, where the wrapped `sum_sq` was then written into a sealed
    /// `.cidx` and content-hashed. A wrong statistic with a hash over it is indistinguishable from a
    /// right one.
    ///
    /// `None` means "this chunk has no exact statistics", and every caller turns that into *no
    /// index* rather than an approximate one — the same fail-closed shape as `ArrayData::as_i64`
    /// declining a `u64` array that will not fit `i64`.
    pub fn from_values(values: &[i64]) -> Option<Self> {
        values.iter().try_fold(Self::identity(), |acc, &v| {
            let v128 = v as i128;
            acc.checked_combine(&ChunkStats {
                count: 1,
                min: Some(v),
                max: Some(v),
                sum: v128,
                sum_sq: v128.checked_mul(v128)?,
            })
        })
    }

    /// The arithmetic mean of the chunk's samples, or `None` for an empty chunk. **Derived** from the
    /// `sum`/`count` base monoids with no extra state — the canonical example of the ADR-0028 §3
    /// extensible-stats factory: new statistics that are functions of the monoids cost nothing to add.
    pub fn mean(&self) -> Option<f64> {
        (self.count > 0).then(|| self.sum as f64 / self.count as f64)
    }

    /// Population variance, or `None` when it cannot be computed exactly (an empty chunk, or an
    /// overflow — see below).
    ///
    /// Computed as `(n·Σx² − (Σx)²) / n²`, with the **numerator evaluated in `i128`**, not as the
    /// textbook `E[x²] − E[x]²`. The textbook form converts `Σx²` and `Σx` to `f64` *before*
    /// subtracting two nearly-equal huge numbers, and catastrophic cancellation then destroys the
    /// answer — not merely its last bits. For `[10¹⁵, 10¹⁵+1]` the true variance is `0.25` and the
    /// `f64` form returns **`0.0`**. Doing the subtraction in exact integers and dividing once at the
    /// end removes the cancellation entirely.
    ///
    /// `None` on overflow rather than a wrapped answer (the #523 rule: fail closed, never report a
    /// wrong statistic as if it were right). `n·Σx²` and `(Σx)²` can both exceed `i128` for 64-bit
    /// samples.
    pub fn variance(&self) -> Option<f64> {
        if self.count == 0 {
            return None;
        }
        let n = self.count as i128;
        // Exact numerator: n·Σx² − (Σx)². Checked, because either term can overflow i128 for
        // 64-bit samples (see #523).
        let n_sum_sq = n.checked_mul(self.sum_sq)?;
        let sum_sq_of_sum = self.sum.checked_mul(self.sum)?;
        let numerator = n_sum_sq.checked_sub(sum_sq_of_sum)?;
        // The numerator is non-negative in exact arithmetic (Cauchy–Schwarz); clamp defensively.
        let numerator = numerator.max(0);
        // One division, at the end, in f64. n² is exact here: n ≤ 2^64 so n² ≤ 2^128, and the
        // f64 conversion of each side is a single correctly-rounded step.
        Some(numerator as f64 / (n as f64 * n as f64))
    }

    /// Population standard deviation = `sqrt(variance)`; `None` when [`Self::variance`] is `None`.
    pub fn std_dev(&self) -> Option<f64> {
        self.variance().map(f64::sqrt)
    }

    /// [`Monoid::combine`] with overflow **detected** rather than wrapped (#523).
    ///
    /// `combine` uses plain `+`, which panics in debug and **wraps silently in release** once
    /// `sum_sq` exceeds `i128` — and a wrapped `sum_sq` written into a sealed `.cidx` is a wrong
    /// statistic with a content hash over it. Callers that must not serve a wrong number use this
    /// and fall back when it returns `None`.
    pub fn checked_combine(&self, other: &Self) -> Option<Self> {
        let min = match (self.min, other.min) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let max = match (self.max, other.max) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        Some(ChunkStats {
            count: self.count.checked_add(other.count)?,
            min,
            max,
            sum: self.sum.checked_add(other.sum)?,
            sum_sq: self.sum_sq.checked_add(other.sum_sq)?,
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
            sum_sq: 0,
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
            sum_sq: self.sum_sq + other.sum_sq,
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
    /// Append a sub-block by its content `digest` and its samples. `false` — and nothing appended —
    /// when the samples cannot be folded exactly (ADR-0059 M2); the caller must then emit no index.
    #[must_use = "a refused push means the index is incomplete and must not be emitted"]
    pub fn push(&mut self, digest: impl Into<String>, values: &[i64]) -> bool {
        match ChunkStats::from_values(values) {
            Some(stats) => {
                self.entries.push(ChunkEntry {
                    digest: digest.into(),
                    stats,
                });
                true
            }
            None => false,
        }
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

    /// An inclusion proof that the sub-block at position `i` is committed under this index's
    /// [`Self::root`] — the **per-chunk confirmation** of ADR-0028 §3, built on the §6 MMR inclusion
    /// proofs. Verify the returned path with [`crate::hash::verify_inclusion`] against the chunk's
    /// `digest` and `root()`. `None` if `i` is out of range. Lets a reader confirm a single chunk
    /// against the seal without re-reading the whole block.
    pub fn chunk_proof(&self, i: usize) -> Option<Vec<crate::hash::ProofStep>> {
        let digests: Vec<String> = self.entries.iter().map(|e| e.digest.clone()).collect();
        crate::hash::inclusion_proof(&digests, i)
    }

    /// The block-level aggregate stat (the level-0 pyramid root) = `combine` of every chunk stat.
    pub fn aggregate(&self) -> ChunkStats {
        self.entries
            .iter()
            .fold(ChunkStats::identity(), |acc, e| acc.combine(&e.stats))
    }

    /// [`Self::aggregate`] with overflow detected rather than wrapped — `None` if any step would
    /// exceed `i128` (#523). A reader that reports statistics as *exact* must use this: `aggregate`
    /// wraps silently in release, and a wrapped total is a wrong number with no outward sign.
    pub fn checked_aggregate(&self) -> Option<ChunkStats> {
        self.entries
            .iter()
            .try_fold(ChunkStats::identity(), |acc, e| {
                acc.checked_combine(&e.stats)
            })
    }

    /// The **aggregate stat pyramid** (ADR-0028 §3, the table multiscale overview): level 0 is each
    /// chunk's stats; each higher level `combine`s adjacent pairs (an odd trailing node carries up
    /// unchanged) until a single summary node. A reader can answer a coarse query at level *L* without
    /// touching the data. The top node equals [`Self::aggregate`]; an empty index yields no levels. This
    /// is the *stats* overview (the integrity hash tree is the sub-block MMR, [`Self::root`]).
    pub fn stat_pyramid(&self) -> Vec<Vec<ChunkStats>> {
        if self.entries.is_empty() {
            return Vec::new();
        }
        let mut levels = vec![self
            .entries
            .iter()
            .map(|e| e.stats.clone())
            .collect::<Vec<_>>()];
        while levels.last().unwrap().len() > 1 {
            let prev = levels.last().unwrap();
            let mut next = Vec::with_capacity(prev.len().div_ceil(2));
            let mut i = 0;
            while i < prev.len() {
                if i + 1 < prev.len() {
                    next.push(prev[i].combine(&prev[i + 1]));
                    i += 2;
                } else {
                    next.push(prev[i].clone());
                    i += 1;
                }
            }
            levels.push(next);
        }
        levels
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

/// ADR-0028 §5 — the **fused streaming fold**: the committer's single serial pass that folds **both**
/// the integrity hash (the MMR) and the rolled-up monoid stats up the tree as each leaf is appended, so a
/// **live root** and a **live aggregate** advance per row-group without a second pass. It pairs the hash
/// [`crate::hash::MerkleAccumulator`] (peaks binary-carry) with a parallel stat peak-carry keyed on the
/// **same** leaf sequence, so the two stay in lockstep and the streamed result is **byte-identical to a
/// batch [`ChunkIndex`]** over the same leaves (the determinism guarantee — the fold is push-ordered and
/// reconciles with the sealed identity). Pure and order-faithful; no I/O — the streaming engine
/// (`tessera-io`) drives it from the ordered committer.
///
/// Each appended leaf builds `popcount-carry(k)` interior `{hash, stats}` merges (the trailing-1-bits of
/// the prior leaf count) — amortized O(1), worst-case O(log n). Trace for 4 leaves: L1→peak P0; L2 merges
/// →P01; L3→peak P2; L4 merges →P23 then →P0123 (the root). The peaks are exactly the roots of the
/// complete perfect subtrees (sizes = the set bits of the leaf count).
///
/// ```
/// use tessera_core::chunk_index::{ChunkIndex, ChunkStats, MerkleStatsAccumulator};
/// use tessera_core::hash::digest;
///
/// // stream five leaves through the fused fold while a batch index sees the same leaves.
/// let mut acc = MerkleStatsAccumulator::new();
/// let mut batch = ChunkIndex::new();
/// for i in 0..5i64 {
///     let d = digest(format!("chunk{i}").as_bytes());
///     // `from_values` / `push` return None / false if the fold cannot be exact (ADR-0059 M2).
///     acc.push(&d, ChunkStats::from_values(&[i, i * 2]).unwrap()); // live: hash + stats fold up
///     assert!(batch.push(d, &[i, i * 2]));
/// }
/// // the live streamed root + aggregate equal the batch index over the same leaves (§5 determinism).
/// assert_eq!(acc.leaves(), 5);
/// assert_eq!(acc.root(), batch.root());
/// assert_eq!(acc.aggregate(), batch.aggregate());
/// ```
#[derive(Debug, Default)]
pub struct MerkleStatsAccumulator {
    hash: crate::hash::MerkleAccumulator,
    /// Stat peaks, one per complete perfect subtree — mirrors the hash peaks (each covers `2^k` leaves).
    stat_peaks: Vec<ChunkStats>,
    leaves: u64,
}

impl MerkleStatsAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one leaf `{digest, stats}` (a per-chunk hash + its monoid stats), folding both up the tree.
    /// The stat carry mirrors the MMR hash carry exactly (same merges on the same leaf count), so the
    /// peaks stay in lockstep.
    pub fn push(&mut self, digest: &str, stats: ChunkStats) {
        self.hash.push(digest);
        // binary-carry the stat peaks: merge equal-height peaks while the prior count's low bits are set.
        let mut carry = stats;
        let mut k = self.leaves;
        while k & 1 == 1 {
            let left = self
                .stat_peaks
                .pop()
                .expect("a peak exists for each set bit");
            carry = left.combine(&carry);
            k >>= 1;
        }
        self.stat_peaks.push(carry);
        self.leaves += 1;
    }

    /// The **live** integrity root over every leaf appended so far — identical to the sealed MMR root and
    /// to [`ChunkIndex::root`] over the same leaves.
    pub fn root(&self) -> String {
        self.hash.root()
    }

    /// The **live** aggregate stats over every leaf so far — bag the stat peaks (monoid-combine),
    /// matching [`ChunkIndex::aggregate`]. Order-independent: the stats monoid is commutative.
    pub fn aggregate(&self) -> ChunkStats {
        self.stat_peaks
            .iter()
            .fold(ChunkStats::identity(), |acc, p| acc.combine(p))
    }

    /// Number of leaves folded so far.
    pub fn leaves(&self) -> u64 {
        self.leaves
    }

    pub fn is_empty(&self) -> bool {
        self.leaves == 0
    }
}

#[cfg(test)]
mod tests {

    /// `from_values` must FAIL CLOSED on overflow, never wrap (ADR-0059 M2, #523).
    ///
    /// `i64::MAX²` is ~8.5e37 against an `i128::MAX` of ~1.7e38, so two such samples fit and the
    /// third overflows `sum_sq`. Before this, the fold used plain `+`: a debug build panicked and a
    /// **release build wrapped silently**, writing the wrapped value into a sealed, content-hashed
    /// `.cidx`. A wrong statistic under a hash is indistinguishable from a right one.
    #[test]
    fn from_values_fails_closed_rather_than_wrapping() {
        let two = vec![i64::MAX, i64::MAX];
        assert!(
            ChunkStats::from_values(&two).is_some(),
            "two i64::MAX values still fit i128"
        );

        let three = vec![i64::MAX, i64::MAX, i64::MAX];
        assert_eq!(
            ChunkStats::from_values(&three),
            None,
            "the third must be refused, not wrapped"
        );

        // And the refusal propagates through `push`, so an index cannot be built from it.
        let mut idx = ChunkIndex::new();
        assert!(
            !idx.push("blake3:x", &three),
            "push must report the refusal"
        );
        assert!(idx.is_empty(), "a refused push must append nothing");

        // i64::MIN squares to the same magnitude — the negative side overflows too.
        assert_eq!(
            ChunkStats::from_values(&[i64::MIN, i64::MIN, i64::MIN]),
            None
        );
    }

    /// Variance must not be computed as `E[x²] − E[x]²` in `f64` — that cancels catastrophically.
    ///
    /// For `[10¹⁵, 10¹⁵+1]` the true population variance is exactly 1/4. The textbook form converts
    /// `Σx²` (2e30) and `Σx` to `f64` and subtracts two nearly-equal huge numbers, which returns
    /// **0.0** — not a rounding error, the wrong answer. The exact numerator `n·Σx² − (Σx)²` is 1,
    /// and 1/n² = 0.25.
    #[test]
    fn variance_is_exact_where_the_textbook_f64_form_cancels_to_zero() {
        let v = vec![1_000_000_000_000_000i64, 1_000_000_000_000_001];
        let s = ChunkStats::from_values(&v).expect("fits i128");

        // Independent exact reference, in integers only.
        let n = v.len() as i128;
        let sum: i128 = v.iter().map(|&x| x as i128).sum();
        let sum_sq: i128 = v.iter().map(|&x| (x as i128) * (x as i128)).sum();
        assert_eq!(n * sum_sq - sum * sum, 1, "exact numerator is 1");

        assert_eq!(s.variance(), Some(0.25));
        assert_eq!(s.std_dev(), Some(0.5));

        // What the old formula produced, for contrast — it is not a near miss.
        let mean_f = sum as f64 / n as f64;
        let old = (sum_sq as f64 / n as f64 - mean_f * mean_f).max(0.0);
        assert_eq!(old, 0.0, "the f64 form cancels to zero on this input");
    }

    /// Overflow must be reported as "cannot compute", never wrapped (#523).
    #[test]
    fn checked_combine_and_variance_fail_closed_on_overflow() {
        let big = ChunkStats {
            count: 1,
            min: Some(i64::MAX),
            max: Some(i64::MAX),
            sum: i64::MAX as i128,
            sum_sq: (i64::MAX as i128) * (i64::MAX as i128),
        };
        // Two fit; three overflow i128 on sum_sq.
        let two = big.checked_combine(&big).expect("two must fit");
        assert_eq!(two.count, 2);
        assert_eq!(
            two.checked_combine(&big),
            None,
            "a third must fail closed, not wrap"
        );

        // And a variance whose numerator overflows reports None rather than a wrapped number.
        let wide = ChunkStats {
            count: u64::MAX,
            min: Some(0),
            max: Some(i64::MAX),
            sum: 0,
            sum_sq: i128::MAX,
        };
        assert_eq!(wide.variance(), None, "n·sum_sq overflows -> None");

        let mut idx = ChunkIndex::new();
        idx.push_entry("blake3:a", big.clone());
        idx.push_entry("blake3:b", big.clone());
        idx.push_entry("blake3:c", big);
        assert_eq!(
            idx.checked_aggregate(),
            None,
            "checked_aggregate must refuse an overflowing roll-up"
        );
    }
    use super::*;
    use crate::hash::{digest, merkle_root};

    fn stats(vs: &[i64]) -> ChunkStats {
        ChunkStats::from_values(vs).expect("test values fit i128")
    }

    #[test]
    fn fused_streaming_fold_matches_batch_for_root_and_aggregate() {
        // ADR-0028 §5: streaming the leaves through MerkleStatsAccumulator must reconcile EXACTLY with a
        // batch ChunkIndex over the same leaves — for BOTH the integrity root and the aggregate stats.
        let leaves: Vec<(String, Vec<i64>)> = (0..13)
            .map(|i| {
                (
                    digest(format!("chunk{i}").as_bytes()),
                    vec![i, i * 2, -i, 100 - i],
                )
            })
            .collect();

        let mut batch = ChunkIndex::new();
        let mut acc = MerkleStatsAccumulator::new();
        assert!(acc.is_empty());
        for (i, (d, vs)) in leaves.iter().enumerate() {
            assert!(batch.push(d.clone(), vs), "test values fit i128");
            acc.push(d, stats(vs)); // the live values advance per leaf and always equal a batch index over the prefix so far.
            let prefix = {
                let mut b = ChunkIndex::new();
                for (pd, pvs) in &leaves[..=i] {
                    assert!(b.push(pd.clone(), pvs), "test values fit i128");
                }
                b
            };
            assert_eq!(
                acc.root(),
                prefix.root(),
                "live root after {} leaves",
                i + 1
            );
            assert_eq!(
                acc.aggregate(),
                prefix.aggregate(),
                "live aggregate after {} leaves",
                i + 1
            );
        }
        assert_eq!(acc.leaves(), 13);
        // and the final streamed result equals the full batch.
        assert_eq!(acc.root(), batch.root());
        assert_eq!(acc.aggregate(), batch.aggregate());
        // empty fold is the identity stats + the empty MMR root.
        let empty = MerkleStatsAccumulator::new();
        assert_eq!(empty.aggregate(), ChunkStats::identity());
        assert_eq!(empty.root(), merkle_root(&[]));
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
    fn mean_is_derived_from_sum_and_count() {
        assert_eq!(stats(&[2, 4, 6]).mean(), Some(4.0));
        assert_eq!(stats(&[-5, 5]).mean(), Some(0.0));
        assert_eq!(ChunkStats::identity().mean(), None); // empty chunk
                                                         // derived stat respects the roll-up: mean over the combine == mean over the concatenation
        let agg = stats(&[1, 2, 3]).combine(&stats(&[10, 20]));
        assert_eq!(agg.mean(), stats(&[1, 2, 3, 10, 20]).mean());
    }

    #[test]
    fn variance_and_stddev_extend_via_the_sum_sq_monoid() {
        // [2,4,6]: mean 4, population variance = (4+0+4)/3 = 8/3.
        let s = stats(&[2, 4, 6]);
        assert!((s.variance().unwrap() - 8.0 / 3.0).abs() < 1e-9);
        assert!((s.std_dev().unwrap() - (8.0_f64 / 3.0).sqrt()).abs() < 1e-9);
        assert_eq!(ChunkStats::identity().variance(), None);
        // sum_sq is associative → variance survives the roll-up (combine == recompute over concat).
        let agg = stats(&[1, 2, 3]).combine(&stats(&[10, 20]));
        let direct = stats(&[1, 2, 3, 10, 20]);
        assert!((agg.variance().unwrap() - direct.variance().unwrap()).abs() < 1e-9);
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
        assert!(idx.push(digest(b"c0"), &[0, 1, 2]), "test values fit i128"); // [0,2]
        assert!(
            idx.push(digest(b"c1"), &[10, 11, 12]),
            "test values fit i128"
        ); // [10,12]
        assert!(
            idx.push(digest(b"c2"), &[20, 25, 30]),
            "test values fit i128"
        ); // [20,30]
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
        assert!(idx.push(d0.clone(), &[1, 2]), "test values fit i128");
        assert!(idx.push(d1.clone(), &[3, 4]), "test values fit i128"); // the index root IS the product's sub-block Merkle root (ties to ADR-0028 §1/§2)
        assert_eq!(idx.root(), merkle_root(&[d0, d1]));
        // empty index → empty MMR root
        assert_eq!(ChunkIndex::new().root(), merkle_root(&[]));
    }

    #[test]
    fn aggregate_is_combine_of_all_chunks() {
        let mut idx = ChunkIndex::new();
        assert!(idx.push(digest(b"a"), &[1, 2, 3]), "test values fit i128");
        assert!(idx.push(digest(b"b"), &[4, 5]), "test values fit i128"); // block-level stat == stat over the whole concatenation
        assert_eq!(idx.aggregate(), stats(&[1, 2, 3, 4, 5]));
        assert_eq!(idx.aggregate().count, 5);
        assert_eq!(idx.aggregate().min, Some(1));
        assert_eq!(idx.aggregate().max, Some(5));
    }

    #[test]
    fn each_chunk_has_an_inclusion_proof_under_the_root() {
        use crate::hash::verify_inclusion;
        let mut idx = ChunkIndex::new();
        for k in 0..6u8 {
            assert!(
                idx.push(digest(&[k, 7]), &[k as i64]),
                "test values fit i128"
            );
        }
        let root = idx.root();
        for i in 0..idx.len() {
            let proof = idx.chunk_proof(i).unwrap();
            assert!(
                verify_inclusion(&idx.entries[i].digest, &proof, &root),
                "chunk {i} must prove inclusion under the index root"
            );
        }
        assert!(idx.chunk_proof(idx.len()).is_none());
    }

    #[test]
    fn incremental_push_entry_matches_batch_index() {
        // ADR-0028 §5 streaming property: building the index entry-by-entry as fragments stream
        // (`push_entry` with each chunk's already-computed `{digest, stats}`) yields the SAME root +
        // aggregate + entries as building it from values in one go — so a >RAM streaming write can
        // accumulate the chunk-index without a second pass over the data.
        let chunks: [&[i64]; 3] = [&[1, 2, 3], &[10, 20], &[-5, 0, 7, 7]];
        let mut batch = ChunkIndex::new();
        let mut incr = ChunkIndex::new();
        for (i, c) in chunks.iter().enumerate() {
            let d = digest(&[i as u8]);
            assert!(batch.push(d.clone(), c), "test values fit i128"); // folds values now
            incr.push_entry(d, ChunkStats::from_values(c).expect("fits")); // stats arrived with the fragment
        }
        assert_eq!(incr.root(), batch.root());
        assert_eq!(incr.aggregate(), batch.aggregate());
        assert_eq!(incr.entries, batch.entries);
    }

    #[test]
    fn stat_pyramid_rolls_up_to_the_aggregate() {
        let mut idx = ChunkIndex::new();
        for k in 0..5u8 {
            assert!(
                idx.push(digest(&[k]), &[k as i64, k as i64 + 10]),
                "test values fit i128"
            );
        }
        let p = idx.stat_pyramid();
        // level 0 = per-chunk stats; levels halve (ceil) down to one summary node.
        assert_eq!(
            p.iter().map(|l| l.len()).collect::<Vec<_>>(),
            vec![5, 3, 2, 1]
        );
        assert_eq!(p[0][0], ChunkStats::from_values(&[0, 10]).expect("fits"));
        // the summit equals the flat aggregate (the monoid law: combine is associative).
        assert_eq!(p.last().unwrap()[0], idx.aggregate());
        // empty index → no levels.
        assert!(ChunkIndex::new().stat_pyramid().is_empty());
    }

    #[test]
    fn index_to_bytes_is_deterministic_and_roundtrips() {
        let mut idx = ChunkIndex::new();
        assert!(idx.push(digest(b"c0"), &[1, 2, 3]), "test values fit i128");
        assert!(idx.push(digest(b"c1"), &[-5, 9]), "test values fit i128");
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
        assert!(idx.push(digest(b"x"), &[7, 8, 9]), "test values fit i128");
        let json = serde_json::to_string(&idx).unwrap();
        let back: ChunkIndex = serde_json::from_str(&json).unwrap();
        assert_eq!(back.entries, idx.entries);
        assert_eq!(back.root(), idx.root());
    }
}
