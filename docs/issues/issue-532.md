---
type: issue
state: open
created: 2026-09-29T15:38:04Z
updated: 2026-09-29T15:38:04Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/532
comments: 0
labels: effort:medium, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:01.298Z
---

# [Issue 532]: [perf(io): building the chunk index costs 5.2x the encode it accompanies](https://github.com/vig-os/tessera/issues/532)

## The number

Measured in the nix devShell, release build, on a 127,664,128-voxel `int16` volume
(487×512×512, 64³ chunks — the shape #347 is about):

| phase | time | vs encode |
|---|---|---|
| `array_block` (pcodec encode + digest) | 280 ms | 1.00× |
| `array_chunk_index` (build the `{hash, stats}` index) | **1459 ms** | **5.21×** |

**Building the chunk index costs 5.2× the actual encode.** This was found while measuring the
histogram's write cost for the ADR-0059 spike — the histogram itself adds ~290 ms (~20 % of the index
build), which is the small number here. The index fold is the one worth attention, and it matters a
lot more now that ADR-0059 makes the sidecar **default-on** for integer array blocks.

## Why it is slow (reading `array_chunk_index`, `crates/tessera-io/src/array.rs`)

Per chunk, it:

1. walks the chunk's voxels with an **odometer** over the multi-index, recomputing
   `flat = Σ v[ax]·stride[ax]` — a rank-length dot product **per voxel** — rather than walking the
   contiguous runs the C-order layout already gives it (the innermost axis is contiguous, so each row
   of a chunk is a slice);
2. materialises **two** per-chunk buffers, `bytes: Vec<u8>` (8 bytes per voxel, for the digest) and
   `chunk_vals: Vec<i64>` (another 8), i.e. ~16 bytes of scratch per voxel — 4 MiB per 64³ chunk, and
   it allocates them fresh each chunk;
3. folds `ChunkStats::from_values` as a **second pass** over `chunk_vals`, after the gather;
4. runs entirely **single-threaded**, while the encode beside it is parallel.

The data is also widened to `i64` first (`data.as_i64()` materialises a whole-array `Vec<i64>` — for
a 127.7 M-voxel `int16` array that is a **1 GB** allocation before any chunk is touched).

## Cheap wins, roughly in order of payoff

- **Don't materialise `as_i64()` for the whole array.** Fold from the native dtype per chunk and widen
  per value. Removes the 1 GB peak.
- **Walk contiguous runs, not voxels.** The innermost axis of a chunk is a contiguous slice; hash and
  fold it a run at a time. Drops the per-voxel dot product entirely.
- **Fold while gathering**, so stats cost no second pass.
- **Hash the native bytes** rather than building an `i64` LE buffer — but note this **changes the
  per-chunk digest**, so it is a format-visible change and must ride the ADR-0059 event if wanted.
- **Parallelise across chunks** (they are independent by construction). Must keep the result
  order-independent — the ADR-0059 determinism tests (identical `.cidx` bytes at worker counts
  1/4/16) already cover this.

## Care needed

The per-chunk `digest` is over the `i64`-LE gather buffer today, so anything that changes **what bytes
are hashed** moves the chunk digests, the index root, and therefore `content_hash`. Optimisations that
keep the same byte sequence (run-walking, fold-while-gathering, dropping `as_i64`, threading) are
**free of format impact**; changing the hashed representation is not, and should either ride ADR-0059's
single format event or be skipped.

## Refs

Found during the ADR-0059 / #522 spike. Becomes user-visible when #347 P1 turns the sidecar on by
default. Relates to #523 (the `sum_sq` overflow in the same fold) and ADR-0026 (bounded-memory write).

Refs: #522

