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
    /// Samples skipped because they were NULL / masked (ADR-0059 S2). `count + masked` recovers the
    /// chunk's element count, which is otherwise **lost**: `ColumnData::as_i64` drops nulls (so a
    /// chunk's `min`/`max` describe the values actually present, which pruning needs), and without
    /// this field there is no way to tell a 100-row group with 40 nulls from a 60-row group.
    ///
    /// `None` means "a v1 index that never recorded it" — **absent, not zero**. Mixing `None` into a
    /// roll-up yields `None`, because an unknown addend makes the total unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub masked: Option<u64>,
    /// Samples that were NaN (ADR-0059 S2). Always `Some(0)` for the integer core — kept in the
    /// schema because float arrays are the deferred §8 extension and adding a field later is a
    /// second corpus event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nan: Option<u64>,
    /// Samples that were `+inf` (ADR-0059 S2). See [`Self::nan`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pos_inf: Option<u64>,
    /// Samples that were `-inf` (ADR-0059 S2). See [`Self::nan`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub neg_inf: Option<u64>,
}

/// Add two "known or unknown" counters. `None` is **unknown**, not zero, so it propagates: a total
/// that includes an unknown addend is itself unknown (ADR-0059's "absent, never zero" rule). `None`
/// on overflow, like every other accumulator here (M2).
fn add_opt(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    a?.checked_add(b?)
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
                // An integer sample is never NaN/±inf, and `from_values` only sees values that
                // survived null-filtering, so the caller supplies `masked` via `with_masked`.
                masked: Some(0),
                nan: Some(0),
                pos_inf: Some(0),
                neg_inf: Some(0),
            })
        })
    }

    /// Record how many samples were skipped as NULL/masked. Builders call this after folding,
    /// because [`Self::from_values`] only ever sees the values that survived null-filtering.
    #[must_use]
    pub fn with_masked(mut self, n: u64) -> Self {
        self.masked = Some(n);
        self
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
            masked: add_opt(self.masked, other.masked),
            nan: add_opt(self.nan, other.nan),
            pos_inf: add_opt(self.pos_inf, other.pos_inf),
            neg_inf: add_opt(self.neg_inf, other.neg_inf),
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
            // Known-and-zero, so `combine(identity(), x) == x` holds for the counters too.
            masked: Some(0),
            nan: Some(0),
            pos_inf: Some(0),
            neg_inf: Some(0),
        }
    }

    fn combine(&self, other: &Self) -> Self {
        // Delegates to `checked_combine` and PANICS on overflow rather than wrapping.
        //
        // Plain `+` wrapped silently in release, and this method feeds `aggregate`,
        // `stat_pyramid` and the live `MerkleStatsAccumulator` — so a wrapped total could reach a
        // reported statistic or a live integrity root with nothing to show for it. A panic is loud
        // and fail-closed; a wrong number under a content hash is neither (ADR-0059 M2, #523).
        //
        // Every seal path in-tree already goes through `from_values` / `checked_combine`, so this
        // is a guard for read paths and future callers, not an expected outcome. Callers that can
        // receive adversarial data should use `checked_combine` and handle `None`.
        self.checked_combine(other)
            .expect("ChunkStats::combine overflowed i128 — use checked_combine to handle this")
    }
}

/// How a [`Histogram`]'s bins are laid out. An enum rather than a bool so the deferred float story
/// (ADR-0059 §8: log-buckets with a relative-error guarantee) is additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HistKind {
    /// **Strictly equal-width** integer bins: every bin spans exactly `width` values, and bin `b`
    /// covers `[lo + b*width, lo + (b+1)*width - 1]`.
    ///
    /// The binned range is *widened* so `width` divides it evenly, so the last bin may extend past
    /// the observed `hi` — which is why the observed edges are stored separately from the bin
    /// geometry. A reader reconstructs every edge from `lo`, `width` and `bins` with integer
    /// arithmetic alone: no float math, and no formula to guess.
    EqualWidthInt,
}

/// The bin budget for [`HistKind::EqualWidthInt`] — **normative, not a default** (ADR-0059 §5,
/// owner decision 2026-10-05).
///
/// A reader recomputes `width`/`bins` from `lo`/`hi` with this exact value and rejects an index that
/// disagrees, so it is part of what the kind *means*. A different budget is a different layout for
/// the same declared `kind`, which would make two valid-looking indexes incomparable — so it
/// requires a NEW kind name instead.
pub const EQUAL_WIDTH_INT_BINS: usize = 4096;

/// A block-level value histogram with **strictly equal-width** bins fixed at write time
/// (ADR-0059 §5).
///
/// Counts are integers, so merging is associative **and commutative** — the roll-up is identical
/// under any worker count or merge order, by algebra rather than by a fixed fold order. That is why
/// a histogram is the structure this format can afford: an order-dependent sketch (t-digest) or a
/// randomised one (KLL) would make sealed bytes depend on how many workers happened to run.
///
/// Stored **once per block**, not per chunk: per-chunk histograms measured 4.6×–14.5× the sidecar
/// for a consumer nobody has, while one block-level histogram is ~1.13× (ADR-0059 §5). Pruning is
/// already served exactly by per-chunk `min`/`max`.
///
/// **Observed range vs bin geometry are separate.** `lo`/`hi` are the observed extremes; `width`
/// and `bins` are the geometry, and `lo + bins*width - 1` may exceed `hi` because the range is
/// widened to divide evenly. Conflating the two is what makes an edge unreconstructible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Histogram {
    pub kind: HistKind,
    /// Observed minimum — the lower edge of bin 0.
    pub lo: i64,
    /// Observed maximum. NOT necessarily the top edge of the last bin (see the type docs).
    pub hi: i64,
    /// Values per bin. Every bin has exactly this width.
    pub width: u64,
    /// Number of bins. Must equal `counts.len()`; stored explicitly as a cross-check on the counts
    /// vector, validated by the reader (ADR-0059 §11 step 3).
    pub bins: u32,
    pub counts: Vec<u64>,
}

impl Histogram {
    /// Lay out strictly equal-width bins covering the observed `[lo, hi]`, under
    /// [`HistKind::EqualWidthInt`]'s normative [`EQUAL_WIDTH_INT_BINS`] budget.
    ///
    /// ```text
    /// span  = hi - lo + 1                     (in i128 — a full i64 range is 2^64)
    /// width = ceil(span / EQUAL_WIDTH_INT_BINS)   (at least 1)
    /// bins  = ceil(span / width)                  (<= EQUAL_WIDTH_INT_BINS)
    /// ```
    ///
    /// `width` is computed first and `bins` second, so every bin is exactly `width` wide and only
    /// the COUNT of bins absorbs the remainder. The alternative — fixing `bins` and deriving a
    /// width — is what produces unequal bins.
    ///
    /// **The budget is not a parameter.** It is normative for this `kind`, which also makes the
    /// field widths provably safe: `width <= ceil(2^64 / 4096) = 2^52` fits `u64`, and
    /// `bins <= 4096` fits `u32`, for every possible `lo`/`hi`. A configurable budget could yield
    /// `width = 0` (budget above the span) or `bins` past `u32`, which earlier silently truncated.
    pub fn new(lo: i64, hi: i64) -> Self {
        let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        // i128 throughout: `hi - lo + 1` for the full i64 range is 2^64, and `lo + bins*width` can
        // likewise pass i64::MAX near the top of the range.
        let span = hi as i128 - lo as i128 + 1;
        // `i128::div_ceil` is unstable, and both operands are >= 1 here, so the closed form is exact.
        let ceil_div = |a: i128, b: i128| (a + b - 1) / b;
        let width = ceil_div(span, EQUAL_WIDTH_INT_BINS as i128).max(1);
        let bins = ceil_div(span, width);
        debug_assert!((1..=EQUAL_WIDTH_INT_BINS as i128).contains(&bins));
        debug_assert!((1..=(1i128 << 52)).contains(&width));
        Self {
            kind: HistKind::EqualWidthInt,
            lo,
            hi,
            // Safe by the bounds above, not by hope: width <= 2^52 and bins <= 4096.
            width: width as u64,
            bins: bins as u32,
            counts: vec![0; bins as usize],
        }
    }

    /// True when every bin holds exactly one value (`width == 1`) — then the histogram is a
    /// complete description of the distribution and quantiles taken from it are exact.
    ///
    /// **Derived, not stored.** A boolean that is a pure function of `width` would be a field that
    /// can disagree with itself, and one more thing a reader has to validate rather than compute.
    pub fn exact(&self) -> bool {
        self.width == 1
    }

    /// The bin a value falls in, saturating at the observed edges.
    pub fn bin_of(&self, v: i64) -> usize {
        let last = self.bins.saturating_sub(1) as usize;
        let w = (self.width as i128).max(1);
        let off = (v as i128).clamp(self.lo as i128, self.hi as i128) - self.lo as i128;
        ((off / w) as usize).min(last)
    }

    /// The inclusive value range bin `b` covers, in `i128` because the top edge can pass `i64::MAX`.
    pub fn bin_range(&self, b: usize) -> Option<(i128, i128)> {
        if b >= self.bins as usize {
            return None;
        }
        let w = self.width as i128;
        let start = self.lo as i128 + b as i128 * w;
        Some((start, start + w - 1))
    }

    /// Count one sample.
    pub fn add(&mut self, v: i64) {
        let b = self.bin_of(v);
        self.counts[b] = self.counts[b].saturating_add(1);
    }

    /// Merge another histogram over the **same** geometry. `None` if anything about the layout
    /// differs — combining mismatched bins would silently fabricate a distribution.
    pub fn combine(&self, other: &Self) -> Option<Self> {
        if self.kind != other.kind
            || self.lo != other.lo
            || self.hi != other.hi
            || self.width != other.width
            || self.bins != other.bins
            || self.counts.len() != other.counts.len()
        {
            return None;
        }
        let mut out = self.clone();
        for (o, b) in out.counts.iter_mut().zip(&other.counts) {
            *o = o.checked_add(*b)?;
        }
        Some(out)
    }

    /// Total samples counted.
    pub fn total(&self) -> u64 {
        self.counts.iter().copied().fold(0u64, u64::saturating_add)
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
    /// The block-level value histogram (ADR-0059 §3), when one was built. `None` = a v1 index, or a
    /// writer that opted out — **absent, not an all-zero histogram**. Those must never be
    /// confusable: all-zero means "measured, nothing there", absent means "not measured".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub histogram: Option<Histogram>,
}

impl ChunkIndex {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            histogram: None,
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
    ///
    /// # Panics
    ///
    /// Panics if the roll-up overflows `i128` — it folds with [`Monoid::combine`], which fails
    /// closed loudly rather than wrapping (ADR-0059 M2). Three `i64::MAX` chunks are enough. Use
    /// [`Self::checked_aggregate`] on data you did not produce, and handle `None`.
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
    ///
    /// # Panics
    ///
    /// Panics if any level's roll-up overflows `i128` — it folds with [`Monoid::combine`] (see
    /// [`Self::aggregate`]). There is no checked variant yet; build the pyramid only over stats you
    /// produced, or pre-check with [`Self::checked_aggregate`], whose success implies every level
    /// below it also fits.
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

    /// Parse an index from its deterministic bytes.
    ///
    /// **This is a deserializer and validates nothing semantic.** `lo > hi`, an empty `counts`, a
    /// `counts` length that disagrees with `bins`, a `width` of 0, and a `width`/`bins` pair that
    /// does not match the ADR-0059 §5 layout recomputed from `lo`/`hi` are all accepted as written.
    /// A reader that will *report* these values must validate them — ADR-0059 §5; the reader-side
    /// validation lands in C5. Deserializing is not vouching.
    ///
    /// (There is no `exact` on the wire to mistrust: it is derived from `width == 1`.)
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
    ///
    /// # Panics
    ///
    /// Panics if the roll-up overflows `i128`, for the same reason as [`ChunkIndex::aggregate`]:
    /// it folds with [`Monoid::combine`], which fails closed loudly rather than wrapping. This one
    /// sits on the streaming write path, so a caller folding adversarial 64-bit values should check
    /// with [`ChunkStats::checked_combine`] before pushing them.
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

    /// The S2 counters obey the monoid laws, and `None` means UNKNOWN — it propagates (ADR-0059).
    #[test]
    fn s2_counters_are_monoidal_and_unknown_propagates() {
        let a = stats(&[1, 2]).with_masked(3);
        let b = stats(&[5]).with_masked(4);

        // identity
        assert_eq!(ChunkStats::identity().combine(&a), a);
        assert_eq!(a.combine(&ChunkStats::identity()), a);
        // associativity
        let c = stats(&[9]).with_masked(1);
        assert_eq!(a.combine(&b).combine(&c), a.combine(&b.combine(&c)));
        // commutativity of the counters (integer addition — the property that makes merge order,
        // and therefore worker count, irrelevant to the sealed bytes)
        assert_eq!(a.combine(&b).masked, b.combine(&a).masked);
        assert_eq!(a.combine(&b).masked, Some(7));

        // `None` is UNKNOWN, not zero: a total containing an unknown addend is unknown.
        let unknown = ChunkStats {
            masked: None,
            ..stats(&[7])
        };
        assert_eq!(a.combine(&unknown).masked, None, "unknown must propagate");
        assert_eq!(unknown.combine(&a).masked, None);
        // and it does not contaminate the other fields
        assert_eq!(a.combine(&unknown).count, 3);
    }

    /// Bins are STRICTLY equal width, and every edge is reconstructible from stored integers.
    ///
    /// Re-derives the ADR-0059 §5 layout independently of `Histogram::new`, then checks the
    /// property the owner's decision turns on: EVERY bin is the same width. The previous
    /// near-equal rule put `v = 4096` of `[0, 12288]` in bin 1365 with widths differing by one;
    /// equal widths remove that entirely at the cost of a top bin that may reach past `hi`.
    #[test]
    fn bins_are_strictly_equal_width_and_edges_reconstruct() {
        // HARDCODED expectations, not a re-implementation of the formula — a helper that copies
        // `new()`'s arithmetic would agree with a wrong `new()`. These were worked out by hand from
        // ADR-0059 §5 with the normative 4096-bin budget.
        for (lo, hi, want_width, want_bins) in [
            (0i64, 0i64, 1u64, 1u32), // span 1    -> one bin of one value
            (0, 4095, 1, 4096),       // span 4096 -> exactly the budget, width 1
            (0, 4096, 2, 2049),       // span 4097 -> width 2, and only 2049 bins
            (0, 12288, 4, 3073),      // span 12289 -> width 4
        ] {
            let g = Histogram::new(lo, hi);
            assert_eq!(
                (g.width, g.bins),
                (want_width, want_bins),
                "span {} must lay out as width {want_width} x {want_bins} bins",
                hi as i128 - lo as i128 + 1
            );
            assert_eq!(g.counts.len(), g.bins as usize);
        }

        // The worked example from the review, in detail.
        let h = Histogram::new(0, 12288);
        assert_eq!(
            (h.width, h.bins),
            (4, 3073),
            "width 4 -> 3073 bins, not 4096"
        );
        assert!(!h.exact(), "width 4 is not one-per-value");

        // EVERY bin is the same width. This is the whole point of the decision.
        let mut widths = std::collections::BTreeSet::new();
        for b in 0..h.bins as usize {
            let (s, e) = h.bin_range(b).expect("in range");
            widths.insert(e - s + 1);
        }
        assert_eq!(widths.len(), 1, "exactly one bin width: {widths:?}");
        assert_eq!(*widths.iter().next().unwrap(), h.width as i128);

        // Edges reconstruct from lo/width/bins with integer arithmetic alone.
        for b in 0..h.bins as usize {
            let (s, e) = h.bin_range(b).unwrap();
            assert_eq!(s, h.lo as i128 + b as i128 * h.width as i128);
            assert_eq!(e, s + h.width as i128 - 1);
            // every value in the bin maps back to it
            for v in [s, (s + e) / 2, e] {
                if v <= h.hi as i128 {
                    assert_eq!(h.bin_of(v as i64), b, "v={v} belongs to bin {b}");
                }
            }
        }

        // The binned range is WIDENED, so the last bin may pass the observed max — which is why
        // `hi` is stored separately from the geometry.
        let (_, last_end) = h.bin_range(h.bins as usize - 1).unwrap();
        assert!(
            last_end >= h.hi as i128,
            "the top bin must cover hi, and may extend past it"
        );
        assert_eq!(last_end, 12291, "widened from 12288 to divide by 4");

        // One bin per value when the span allows: width 1, and bin_of collapses to v - lo.
        let e = Histogram::new(-1024, 3071);
        assert_eq!((e.width, e.bins), (1, 4096));
        assert!(e.exact());
        for v in [-1024i64, -1023, 0, 3070, 3071] {
            assert_eq!(e.bin_of(v), (v + 1024) as usize);
        }
        // Out-of-range clamps rather than panicking or wrapping.
        assert_eq!(e.bin_of(-9999), 0);
        assert_eq!(e.bin_of(9999), 4095);

        // A degenerate single-value range is one bin of width 1.
        let one = Histogram::new(7, 7);
        assert_eq!((one.width, one.bins), (1, 1));
        assert!(one.exact());
        assert_eq!(one.bin_of(7), 0);
    }

    /// The full i64 range must not overflow: `span` is 2^64 and `lo + bins*width` passes i64::MAX.
    #[test]
    fn the_full_i64_range_lays_out_without_overflow() {
        let h = Histogram::new(i64::MIN, i64::MAX);
        // span = 2^64, which i64 cannot hold at all — the layout must be done in i128.
        let span = i64::MAX as i128 - i64::MIN as i128 + 1;
        assert_eq!(span, 1i128 << 64);
        assert_eq!(h.width as i128, span / 4096, "2^52 per bin");
        assert_eq!(h.bins, 4096);
        assert!(!h.exact());

        // The extremes land in the first and last bins, and nothing panics.
        assert_eq!(h.bin_of(i64::MIN), 0);
        assert_eq!(h.bin_of(i64::MAX), 4095);
        assert_eq!(
            h.bin_of(0),
            2048,
            "zero sits at the midpoint of a symmetric range"
        );

        // Here the top edge lands exactly ON i64::MAX.
        let (_, last_end) = h.bin_range(4095).unwrap();
        assert_eq!(last_end, i64::MAX as i128);

        // And here it genuinely EXCEEDS it — which is the reason `bin_range` returns i128 at all.
        // A span of 12289 at the very top: width 4, 3073 bins, so the widened top bin ends three
        // values past i64::MAX and simply could not be expressed as an i64.
        let top = Histogram::new(i64::MAX - 12288, i64::MAX);
        assert_eq!((top.width, top.bins), (4, 3073));
        let (_, over) = top.bin_range(3072).unwrap();
        assert_eq!(
            over,
            i64::MAX as i128 + 3,
            "the top bin must be allowed to pass i64::MAX"
        );
        assert!(over > i64::MAX as i128);
        // Lookups at the real extreme still work and stay in range.
        assert_eq!(top.bin_of(i64::MAX), 3072);
        assert_eq!(top.bin_of(i64::MAX - 12288), 0);

        // And counting at both extremes is safe.
        let mut h = h;
        h.add(i64::MIN);
        h.add(i64::MAX);
        assert_eq!((h.counts[0], h.counts[4095]), (1, 1));
        assert_eq!(h.total(), 2);
    }

    /// `Monoid::combine` must FAIL LOUDLY on overflow, never wrap (ADR-0059 review nit).
    ///
    /// It feeds `aggregate`, `stat_pyramid` and the live accumulator, so a wrapped total could
    /// reach a reported statistic or a live integrity root. A panic is fail-closed; wrapping is not.
    #[test]
    #[should_panic(expected = "use checked_combine")]
    fn combine_panics_rather_than_wrapping() {
        let big = ChunkStats {
            count: 1,
            min: Some(i64::MAX),
            max: Some(i64::MAX),
            sum: i64::MAX as i128,
            sum_sq: (i64::MAX as i128) * (i64::MAX as i128),
            ..ChunkStats::identity()
        };
        let two = big.combine(&big); // fits
        let _ = two.combine(&big); // must panic, not wrap
    }

    /// A histogram merges only over identical edges, and refuses otherwise.
    #[test]
    fn histogram_merges_only_over_identical_edges() {
        let mut a = Histogram::new(0, 9);
        let mut b = Histogram::new(0, 9);
        for v in [0, 5, 9, 5] {
            a.add(v);
        }
        for v in [5, 5] {
            b.add(v);
        }
        let m = a.combine(&b).expect("same edges merge");
        assert_eq!(m.total(), 6);
        assert_eq!(m.counts[5], 4);
        // commutative, so merge order cannot change the bytes
        assert_eq!(a.combine(&b), b.combine(&a));

        // One bin per value -> exact. Under the normative 4096-bin budget any span up to 4096 is
        // exact, so the coarse case needs a span past it.
        assert!(Histogram::new(0, 9).exact());
        assert!(
            Histogram::new(0, 4095).exact(),
            "span 4096 is still width 1"
        );
        assert!(!Histogram::new(0, 8191).exact(), "span 8192 needs width 2");

        // Mismatched geometry is refused rather than silently fabricating a distribution. A span
        // past the budget gives a different `width`, so the layouts genuinely differ.
        assert_eq!(
            a.combine(&Histogram::new(0, 8191)),
            None,
            "different width/bins"
        );
        assert_eq!(a.combine(&Histogram::new(1, 10)), None, "different edges");

        // out-of-range values saturate into the end bins rather than panicking
        let mut h = Histogram::new(0, 9);
        h.add(-100);
        h.add(1000);
        assert_eq!((h.counts[0], h.counts[9]), (1, 1));
    }

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
            ..ChunkStats::identity()
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
            ..ChunkStats::identity()
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
