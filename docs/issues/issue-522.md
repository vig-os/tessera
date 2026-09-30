---
type: issue
state: open
created: 2026-09-29T14:11:58Z
updated: 2026-09-29T14:11:58Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/522
comments: 0
labels: feature, area:core
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:05.402Z
---

# [Issue 522]: [feat(core): histogram monoid in the chunk-index — exact decode-free histograms, and the .cidx schema decision it forces](https://github.com/vig-os/tessera/issues/522)

## Motivation

#347 serves `count/min/max/mean/variance/std` from `ChunkIndex::aggregate()` with **no decode**, because
those are monoids the chunk-index already stores. A **value histogram** is the one statistic a viewer
needs that the index cannot answer today — there is no histogram monoid in `ChunkStats`.

This issue is that monoid, and the format decision it forces. It is split out of #347 because it is a
`.cidx` **schema change**, whereas #347's read path is not.

## The ordering constraint (why this is time-sensitive)

Adding a histogram field to `tessera_core::chunk_index::ChunkStats` changes its serialized bytes →
the `.cidx` payload → that block's digest → the `content_hash` of **any product carrying an index**
(`content_hash = merkle_root(&digests)` over every block ref, `product.rs:177`).

Right now that costs **nothing**: no product in `tessera/corpus/` carries a `.cidx` at all, and the
sidecar is opt-in by construction (`stream.rs:56-58`, `chunk_index.rs:8-10`). The moment the
"attach `.cidx` by default for integer arrays" decision lands (#347 P1, deferred to the owner), every
integer array product carries one and this becomes a **second corpus event**.

**So decide this together with #347 P1, and land this one first if both are wanted.** Two separate
regenerations is the avoidable outcome.

## The design constraint the phrase "two-level" hides

An **exact** histogram that costs no decode at read time requires the **bin edges to be fixed at write
time**. The tempting "read `aggregate()` for min/max, then bin" is two passes over the *data*, which is
precisely the full decode #347 exists to avoid. The bins must already be in the `.cidx`.

That makes the edges a schema decision, not an implementation detail:

- **How many bins?** A fixed count (256? 512?) keeps `ChunkStats` a fixed size and the roll-up trivial.
- **Over what range?** Per-chunk observed min/max make chunk histograms non-combinable (different edges
  per chunk — not a monoid). Combinable options: a dtype-wide domain (e.g. all of `int16`, which is
  exact and needs no prepass but wastes bins on sparse CT), or a **writer prepass** over the whole array
  to fix global edges before chunk folding (exact, combinable, costs one extra pass at write only).
- **Where recorded?** The edges must be in the `.cidx` block `spec` (alongside `recipe`/`root`), and the
  `recipe` bumped `chunk_index@1` → `@2`, so a reader knows which schema it has.
- **Back-compat.** `sum_sq` precedent: `#[serde(default)]` lets an old `.cidx` deserialize. A missing
  histogram must read as "absent", never as "all-zero bins", or a reader will silently serve a wrong
  histogram as exact.

## Alternative that needs no schema change

Approximate the histogram from the **stat pyramid** (`ChunkIndex::stat_pyramid()`) or a coarse pyramid
level, and label it `exact: false` with `method`. #347 already ships the `exact`/`method` contract, so
this slots in without a format event. Strictly weaker, but it is available today and honest about it.

## Acceptance

- A histogram monoid whose `combine` is associative and whose roll-up over all chunks equals the
  histogram of a full decode **exactly** (property test, not a spot check).
- Bin edges recorded in the `.cidx` spec; `recipe` bumped; an index without them reads as absent.
- Byte-determinism of the `.cidx` preserved (same input → same bytes).
- Corpus regenerated in the same PR if and only if the owner has approved the format event.
- `tessera stats --json` gains the histogram with `exact: true` (and the pyramid variant labelled
  `exact: false` if that lands instead).

## Refs

Split out of #347 (which serves the non-histogram stats decode-free and ships the `exact`/`method`
output contract). Builds on the ADR-0028 §3 extensible-stats factory; the `.cidx` block is ADR-0027 /
#214. Owner decision required before any sealed-layout change.

Refs: #347

