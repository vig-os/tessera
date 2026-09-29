---
type: issue
state: open
created: 2026-07-09T07:51:27Z
updated: 2026-09-28T17:58:58Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/347
comments: 0
labels: feature, area:core, area:io
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:46.529Z
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

