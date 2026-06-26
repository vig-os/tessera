# ADR-0028 — The unified hierarchy: recursive MMR Merkle + multiscale `{hash, stats}` pyramid + derived sidecars + fused streaming

Status: **Proposed** (2026-06-26) · Tracks `#215` · **Supersedes** the flat-list `content_hash` of
ADR-0020 · **Absorbs** ADR-0027 (sub-block Merkle + chunk-index) · Rides ADR-0026 (streaming
compaction). A deliberate **pre-1.0 (v0.2)** identity revision.

## Context
Tessera currently has two *separate* hierarchies that should be one. The block-level integrity is a
**flat list** — `content_hash = blake3(concat(block_digests))`, one level, no tree — while ADR-0027
proposes a real authenticated *tree* inside blocks. And the stats (ADR-0027), the OME-Zarr-style
multiscale pyramid, and the streaming pipeline are all describing the *same* shape from different
angles. This ADR unifies them: **one recursive, content-addressed, multiscale hierarchy, built in a
single streaming pass, verifiable at every level.**

## Decision

### 1. One recursive Merkle construction — `product → block → chunk → sub-chunk`
Replace the flat-list root with a **single node-hash construction applied recursively** at every level.
A node's digest is the Merkle hash of its ordered children; leaves are `blake3(bytes)` of the finest
unit (a row-group / a 64³ cube). Inclusion proofs then work uniformly — prove a chunk, a block, or the
product with **one recursive verifier**. (At the block level, where a product has few children, this is
mostly uniformity + future-proofing; at the chunk level, with thousands of leaves, it is load-bearing.)

### 2. Append-friendly tree at the streaming level (MMR / history tree)
The leaves that land *during capture* (row-group fragments) use an **append-only Merkle Mountain Range
/ CT-style history tree**, not a balanced tree that would rebalance on append. This buys **consistency
proofs**: the *sealed* root is provably an append-only extension of the *live* root verified at
watermark T — the DAQ guarantee, "the archive is provably the capture I was watching." One MMR-shaped
construction is used throughout for uniformity; static levels are just the degenerate (no-more-appends)
case.

### 3. Each node carries `{ hash, rolled-up monoid stats }` — the multiscale pyramid
Every node is `{ merkle_hash, aggregate_stats }`, with stats as **mergeable monoids** (ADR-0027). The
same tree then serves **integrity** (the hash), **pruning** (descend skipping subtrees by `min/max`),
and **overview** (read coarse levels without touching data). "Multiscale" is the **one cross-substrate
concept**:
- **Arrays** → adopt **OME-Zarr multiscale** literally (level 0 canonical, levels 1+ downsampled) →
  direct interop with napari/viv/neuroglancer/OME tooling.
- **Tables** → the **aggregate pyramid** (per-row-group → super-chunk → block summaries) = the ADR-0027
  monoid tree.

So a reader asks for the product at **level L** uniformly across zarr and vortex; spatial downsample
(arrays) and value aggregation (tables) are the same hierarchy.

### 4. Derived-sidecar block class — accelerators that float on top of the identity
Pyramids, **projections** (MIP / average / MPR; group-bys, sort-indexes, materialized views), indexes,
thumbnails, format-views share one shape: **a pure function of the canonical data, costly to recompute,
worth caching.** Make that a first-class block class:
- **Pure function of canonical → regenerable** (a cache, never source-of-truth).
- **Content-addressed + recipe-stamped:** each sidecar records `from = <canonical digest>` + `recipe`
  ("MIP/z", "mean-pool 2³", "min/max per 2¹⁶") → verifiable (recompute + compare) or trustable
  (recipe + signature).
- **NOT in the canonical identity** — its own digest, a separate tier; regenerating a thumbnail must not
  change `content_hash`.
- **Optional / detachable** — ship lean (canonical only) or fat (with accelerators); the reader uses a
  sidecar if present, recomputes or does-without if absent.
- **Same two substrates** — a pyramid level is a zarr/vortex block, an index is vortex, a MIP is zarr.
  No third storage kind; just blocks **tagged `canonical | derived`** in the manifest.

The integrity leaves (hashes) are canonical (they *are* the identity); stats/pyramids/projections are
derived. The chunk-index straddles — leaf-hashes canonical, stat columns derived — so the manifest tag
keeps the canonical identity clean while accelerators float above it.

### 5. The fused streaming pass — encode + hash + tree + stats in one flow
The hierarchy is **built in the streaming pipeline**, not a separate pass:
```
producer ─▶ bounded ring ─▶ ENCODE POOL (parallel, per-chunk, independent) ─▶ ORDERED COMMITTER (serial)
                              { encode (pcodec/zstd | Vortex)                   { durable fragment
                                + blake3(encoded bytes, hot in cache)             + MMR-append the leaf
                                + min/max/count/sum over raw column }              + fold stats up the tree
                                                                                   + advance the live root }
```
Per-chunk work is independent → the pool (one touch while the chunk is resident in the ring, before the
allocator recycles the slot; blake3 + stats ≪ codec cost → ~free). The **order-dependent fold**
(MMR-append + stats roll-up) is the single serial committer (already ordered). The **live root advances
per row-group as it streams**, giving live integrity + live overview + crash-durable fragments in one
pass, and — because the fold is **push-ordered** — it is deterministic and reconciles exactly with the
sealed identity. Only the float `sum` needs a **canonical reduction** guard; blake3 / min / max / count
are exact.

**Where the interior levels are built — the committer's MMR carry.** The pool builds *leaves only*
(per atomic chunk: `blake3` + the factory fields). The committer builds **every interior level**
incrementally as the MMR "carry": it keeps the **peaks** (roots of complete perfect subtrees, sizes
1,2,4,8… like the set bits of a counter); appending leaf *k* merges colliding peaks → each merge **is**
an interior node `{H(left,right), merge(stats)}`, so leaf *k* builds `popcount-carry(k)` =
(trailing-1-bits of *k*) interior nodes — amortized **O(1)**, worst-case O(log n). Trace for 4 leaves:
append L1 builds P01 (+1); append L3 builds P23 (+1) then P0123 (+2 = root). No seal-time pass ever
re-folds: sealing snapshots the peaks into the final root, and the interior `{hash,stats}` nodes are
**persisted as they finalize** into the chunk-index/sidecar block (persist-all = the "materialize the
pyramid" knob; keep-only-peaks = integrity root only). **The fold is NOT rayon'd** — it's nanoseconds
per leaf vs a millisecond encode, and order-dependent; one committer keeps up trivially (its only
possible bottleneck is durable fragment I/O, fixed by fsync-batching, not by parallelizing the fold).

**Table vs array.** This live **append-MMR** is the *table/DAQ* case (row-groups arrive in time order).
Arrays are usually batch-written, so their tree is the *same* `{hash,stats}` construction over the 64³
chunks in a **canonical order** (Morton/raster), built at write — the carries still apply if streamed
slice-by-slice. The **multiscale/pyramid + projection** generalization for arrays (next section) shares
the node structure; only the *liveness* is table-centric.

## Arrays — one fold abstraction for pyramids *and* projections (the factory, generalized)
The factory "field" is a **mergeable monoid** (`max`, `mean=sum/count`, `min`, `sum`, …). An array
derived view is that monoid **folded over a chosen *subset* of the chunk-grid axes, to a chosen depth**:
- fold all axes hierarchically (2× per level) → the **OME-Zarr multiscale pyramid** (3-D coarsening);
- fold **one axis fully** → a **projection** (`max` ⇒ **MIP**, `mean` ⇒ average projection; an oblique
  fold ⇒ **MPR**) → an (N−1)-D image;
- fold two axes → a 1-D profile; fold all axes → the scalar block stat (== the table's block stat).

So pyramids, MIP/MPR projections, profiles, and scalar stats are **one abstraction** parameterized by
`(monoid, axes, depth)` — and a projection **appears at every pyramid level** because it's the same
monoid folded along the projection axis at each level, so it falls out of the *same* per-chunk fold that
builds the pyramid. Tables are the degenerate 1-D case (the only axis is `rows`). `max`/`min`/MIP are
exact; `mean`/`sum` carry the float canonical-reduction guard. **Status: to be validated by the
array-fold spike (#215)** — does one monoid-fold-over-axes pass really yield a correct pyramid level
*and* a correct MIP, cheaply, on a real volume.

## Consequences
- **`content_hash` changes** (flat list → recursive root) → a deliberate **v0.2 identity revision** +
  golden regen. The independent reader is digest/Merkle-based, so it follows the SPEC's node-hash rule
  without decoding codecs.
- **SPEC additions:** the recursive node-hash + MMR/consistency rules; the `{hash, stats}` node + stat
  registry (from ADR-0027); the `canonical | derived` block tag + the sidecar `recipe` schema; OME-Zarr
  multiscale mapping for arrays.
- **`tessera-core`:** recursive + MMR Merkle, inclusion + consistency proofs, canonical/derived tagging.
- **`tessera-io`:** the fused encode+hash+tree+stats pipeline on the ADR-0026 accumulator; derived-
  sidecar emit + verify.
- **Conformance:** multi-level inclusion + consistency proof fixtures; a derived-sidecar fixture; the
  golden regen for the new root.
- **Backward-compat:** the flat→tree root is a **break** (v0.2); the derived-sidecar tier is **additive**
  (a product with no sidecars verifies exactly as a canonical-only product).

## Status note
Proposed — the determinism-critical identity revision. Sequence **after** the ADR-0026 accumulator
(its row-group fragments are the leaves the fused pass folds), and gate on the same cross-env
determinism the format carries (#198) + the golden regen. This is the unifying keystone; ADR-0027's
sub-block Merkle + chunk-index become the in-block levels of *this* tree.
