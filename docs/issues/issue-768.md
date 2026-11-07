---
type: issue
state: open
created: 2026-11-06T14:41:55Z
updated: 2026-11-06T14:42:24Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/768
comments: 1
labels: feature, area:core, area:io, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-07T08:18:16.650Z
---

# [Issue 768]: [feat(ingest): wide FixedSizeList (N > 8) has no carrier — store declared vector columns natively as a Vortex fixed_size_list](https://github.com/vig-os/tessera/issues/768)

## Motivation

`FixedSizeList<scalar, N>` with **N > 8** is rejected on table ingest. Its only alternatives are dropping the
column or ingesting the file as an opaque blob (`arrow_table.rs` error hint). That leaves no lossless home for
legitimately wide per-row vectors:

- embedding vectors (384/768/1024 wide), needed for similarity search
- per-event waveforms / sample windows (list-mode, digitizer traces)
- per-row spectra or histograms (energy spectra, fixed-bin distributions)
- fixed-size descriptors or feature vectors from analysis pipelines

None of these is a flattened volume. They are a **row-aligned vector column**: every row has exactly `N` values,
and they are read together with that row's other columns.

## What already exists

- **The guard is deliberate and right for its case.** ADR-0056 §2 (table row `FixedSizeList<scalar, N≤8>`, line 115)
  and `MAX_FIXED_LIST_WIDTH = 8` (`tessera/crates/tessera-ingest/src/arrow_table.rs:106-113`): a width-3 list is a
  coordinate, but a width-262144 list is a flattened 64³ volume whose home is the array primitive, and minting
  262144 columns would perpetuate that misuse.
- **Vortex can carry the column natively.** The pinned fork (`608d8a1`) has a `fixed_size_list` array
  (`vortex-array/src/arrays/fixed_size_list`): one column, one contiguous child buffer, no expansion.
- **ADR-0056 §11** already handles fixed-size lists through the shared logical→physical `gather` lane, so the
  nullity/validity rules for list slots are worked out.

## The gap

The ingest decision has two outcomes, *expand into N columns* or *reject*. A third one is missing:
**store natively as one Vortex `fixed_size_list` column**. Expansion doesn't scale past a handful of components,
and the array primitive isn't row-aligned with the table, so it loses filter pushdown, joins and streaming
(ADR-0062). It also excludes `int8`/`float16` (pcodec, ADR-0023).

## Proposal

- Add a **declared vector column kind**: the schema (or an ingest flag/spec key) marks a field as
  `vector{dim, dtype}`. Declared fields of any width are stored natively as a Vortex `fixed_size_list` column,
  with a transform record such as `fixed_list_native{n}`.
- **Keep the guard for undeclared fields.** An undeclared `N > 8` list is still rejected, and the error suggests
  either the array primitive (volume) or declaring the field as a vector. So the ADR-0056 protection against
  silently ingesting flattened volumes stays intact, and the misuse can't happen by accident.
- `N ≤ 8` keeps expanding by default (SQL-friendly coordinates). Whether a declared narrow field should also stay
  native is an open question.
- Encode/decode, the DataFusion provider (ADR-0062) and the Python/wasm bindings expose the column as Arrow
  `FixedSizeList` (or the canonical `arrow.fixed_shape_tensor` extension).

## Open questions

1. **Determinism.** Does `fixed_size_list` produce byte-identical output through the fork's writer and default
   compressor (the ALP-exclusion lesson from ADR-0024)? Probe this first: it decides whether the column can be
   sealed as-is.
2. **dtype coverage.** `f32`/`f64`/integers clearly; `f16` (currently widened by `f16_widen`) and `i8` + scale
   (cf. #310)?
3. **Chunk statistics** (ADR-0059): per-component min/max, or none for vector columns?
4. Should the same mechanism later cover `FixedSizeBinary` (hashes, UUIDs), which is rejected today for a related
   reason?

Refs: ADR-0023, ADR-0024, ADR-0056 §2/§11, ADR-0059, ADR-0062. Blocks the strict-Vortex vector-search issue (filed
alongside).

---

# [Comment #1]() by [gerchowl]()

_Posted on November 6, 2026 at 02:42 PM_

Blocks #769 (strict-Vortex vector search).

