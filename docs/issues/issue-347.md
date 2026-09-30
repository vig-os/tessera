---
type: issue
state: open
created: 2026-07-09T07:51:27Z
updated: 2026-09-29T14:12:22Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/347
comments: 1
labels: feature, area:core, area:io
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:14.706Z
---

# [Issue 347]: [feat(explore/io): serve array stats + histogram from the chunk-index aggregate — no full decode](https://github.com/vig-os/tessera/issues/347)

## Motivation

Opening **Data mode** on a real reconstructed volume is slow because stats + histogram are computed **live, from a full decode, every time**. Concretely, `~/Data/HDD/data/tsra/DP01/recon-2-ct.tsra`:

- `volume`: `int16 [487, 512, 512]` = **127.7 M voxels ≈ 255 MB** decoded, chunked `64³` (~512 chunks).
- `tessera-tui`'s `load_array` (`crates/tessera-tui/src/data.rs`) does, on first entry: `read_block` (full pcodec decompress) → `array_stats` (full pass) → `histogram` (second full pass) → `region_to_f64(&data, None)` (**a ~1 GB `Vec<f64>` copy**) → `mip_plane` — *even when the image sub-view is never toggled*.

The stats a viewer needs (count/min/max/mean/std and a value histogram) are **exactly the monoid roll-ups** the format is already designed to precompute. This issue is about *not decoding 255 MB to draw a histogram*.

## What already exists (the plumbing is ~90% there)

- **`tessera_core::chunk_index::ChunkStats`** is a monoid over `i64` samples: `count / min / max / sum / sum_sq` → derived `mean() / variance() / std_dev()`, plus `overlaps(lo,hi)` range-pruning. Extensible by adding a monoid (ADR-0028 §3).
- **`ChunkIndex::aggregate()`** rolls every chunk's stats up to a single global `ChunkStats` — i.e. **exact global count/min/max/mean/variance with zero decode**, given the index.
- **`tessera_io::array::array_block_with_index` / `array_chunk_index`** already build the `{hash, stats}` index for an integer array and emit it as an additive `.cidx` companion block (`chunk_index_block`); `crates/tessera-io/src/multiblock.rs:260` already reads an index back and calls `.aggregate()`.

## The gaps

1. **Ingest doesn't attach the index.** The DP01 recon volumes carry *no* chunk-index block (manifest blocks = just `volume`; aux = `provenance.json`). The ingest/seal path used `array_block`, not `array_block_with_index`, even though `volume` is integer + chunked (eligible). So the aggregate isn't available to read.
2. **The explorer never consults it.** `load_array` always full-decodes for stats even when a `.cidx` is present — it should read `count/min/max/mean/variance` from `aggregate()` and skip the decode.
3. **Histogram isn't a monoid yet.** min/max/mean/count/variance are free from the aggregate; a *value histogram* needs either (a) a new fixed-bin count-vector monoid added to the `ChunkStats` factory — exact, but bin edges must be predetermined (two-level: read `aggregate()` for the min/max range → per-chunk histogram monoid over those edges), or (b) an approximate histogram assembled from per-chunk partial histograms / a coarse pyramid level.

## Proposed approach (phased)

- **P1 — ingest: attach the chunk-index sidecar by default for integer array blocks** (route the recon/array seal through `array_block_with_index`). Deterministic, additive, back-compat (older products simply lack it).
- **P2 — explore stats from the aggregate.** When a data block has a sibling `.cidx`, `array_stats` reads `count/min/max/mean/variance` from `ChunkIndex::aggregate()` — **no `read_block`**. Fall back to the current full-decode path when absent. (Exposed once in `tessera-explore` so CLI/TUI/serve/MCP all get it.)
- **P3 — histogram monoid.** Add a fixed-bin histogram to the `ChunkStats` factory (bin edges from the aggregate's min/max) so an **exact** value histogram rolls up per-chunk and the explorer reads it decode-free; or ship the approximate-from-pyramid variant first and label it as approximate.

Separately, the eager `region_to_f64` + MIP in `load_array` should be **lazy** (only when the image is toggled) — a cheap, independent win that halves post-decode work and cuts peak memory ~4× regardless of this issue. (Tracked as the "cheap fix"; can land first.)

## Scope / non-goals

- **Integer arrays only** for the exact path (the ADR-0028 §3 integer core); float arrays reduce canonically (ADR-0024) or defer to the approximate path.
- Preserve **byte-determinism** of the sidecar (same input → same `.cidx` bytes).
- Exact histogram is inherently **two-level** (range then bin) — document the ordering; don't pretend a single monoid pass fixes bins a priori.
- Regenerate the conformance corpus / DP01 fixtures (or rely on the absent-index fallback) so nothing regresses.

## Acceptance

- Opening Data mode on the 255 MB `recon-2-ct` volume shows stats + histogram **without a full block decode** when the `.cidx` is present; the numbers match the full-decode path within bin resolution.
- The write path attaches a deterministic `.cidx` for integer array blocks; a product without one still works (fallback).
- Cross-checked stats (min/max/mean/std) are bit-identical to the full-decode aggregate.

## Refs

Refs: #286 (the .tsra explorer). Relates to #215 (unified hierarchy: MMR + multiscale {hash,stats} pyramid + fused streaming), #214 (sub-block Merkle + chunk-index block), #221 (ADR-0026–0032 overhead spike). Builds on the ADR-0028 §3 extensible-stats factory.

---

# [Comment #1]() by [gerchowl]()

_Posted on September 29, 2026 at 02:12 PM_

## Correction: the acceptance criterion has the exactness backwards

> Cross-checked stats (min/max/mean/std) are bit-identical to the full-decode aggregate.

This cannot hold, and the direction matters: **the index is the more accurate side, not the reference.**

`nav.rs::array_stats` accumulates `sum` and `sumsq` in `f64`. `ChunkStats` accumulates in `i128`
(`sum: i128`, `sum_sq: i128`). On a 127.7 M-voxel `int16` volume — i.e. `recon-2-ct`, the volume this
issue is about — `sum_sq` reaches ~1.18e17, well past the `f64` exact-integer limit 2^53 = 9.007e15.

Measured on a deterministic 127,700,224-value `int16` stream at realistic magnitudes (values cycling
28000..32767), comparing the two accumulation strategies directly:

```
sum_sq  exact (i128)  118129291954102976
sum_sq  f64           118129291905977936
sum_sq  abs error              48,125,040

std     exact (i128)  1376.4030171166635
std     f64           1376.4028802165080
std     abs error              1.369e-4
```

`count`, `min`, `max` and `mean` still agree here (`sum` ≈ 3.88e12 stays under 2^53); it is `sum_sq`,
and therefore `variance`/`std`, that diverges. The error is data-dependent — it grows with both voxel
count and value magnitude — so it is not a fixed tolerance one can hard-code.

Consequences for this issue:

1. **The equivalence test must use an exact integer reference**, assert the index matches it exactly,
   and allow the existing `f64` full-decode path a tolerance. Asserting "index == full decode" bit-for-bit
   would be asserting that the correct answer equals the approximate one.
2. **`array_stats` should accumulate integer dtypes in integer arithmetic.** That is a real (small)
   correctness fix independent of the chunk-index work, and it makes the two paths agree bit-for-bit for
   integer arrays, which is what the criterion was reaching for.

Suggested replacement wording:

> Cross-checked stats (count/min/max/mean/std) from the index are **exact** — equal to an exact-integer
> reference over the full array. The `f64` full-decode path agrees within a tolerance that grows with
> voxel count and magnitude; where they differ, the index is correct.

## Two other notes from reading the code

- **`tessera-explore` / `crates/tessera-tui/src/data.rs` do not exist on `dev`** — that is #286's
  explorer, not yet landed. The only consumer of array stats today is `tessera-cli`'s
  `nav::stats` (`tessera stats FILE BLOCK`), text-only. P2 therefore lands there, and adds
  `tessera stats --json` as the surface for the `exact` / `method` fields.
- **P3 (the histogram) is split out to #522**, because it is a `.cidx` schema change while P2's read
  path is not, and because it is ordered against P1: it is free today (no corpus product carries a
  `.cidx`) and becomes a second corpus event once P1 attaches one by default.

P1 (attach `.cidx` by default) moves `content_hash` for every integer array product
(`content_hash = merkle_root(&digests)` over every block ref, `product.rs:177`) and is held for an
owner decision. P2 needs no format change and is in progress.


