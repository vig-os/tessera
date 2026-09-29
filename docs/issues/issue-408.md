---
type: issue
state: open
created: 2026-08-21T09:28:18Z
updated: 2026-09-28T17:59:06Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/408
comments: 0
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:38.241Z
---

# [Issue 408]: [Three presentation-not-values paths can move `content_hash` with no information change (H10–H12)](https://github.com/vig-os/tessera/issues/408)

## Motivation

ADR-0056 §6a (#403, PR #405) rests on **Finding 1**: because ingest is a logical re-encode,
`content_hash = f(extracted logical values, Tessera's own encoder)`, so a decoder change that extracts
identical logical values cannot move `content_hash`. That finding is why the decoder identity does not
need to be sealed — and it is load-bearing for the whole decision.

The CI-enforcement lens tested the finding rather than accepting it, and found **three concrete paths
by which a decoder change could move `content_hash` with no change in information**. None of them is
enumerated in the §5 H1–H9 hazard table:

1. **Nullable wrapping.** `ColumnData::Nullable{values, validity}` is a distinct variant and
   `to_le_bytes` appends a packed validity trailer (`tessera-io/src/table.rs:259-263`). A decoder that
   starts wrapping an all-valid column in `Nullable` moves `content_hash` while carrying identical
   information. H5 covers *values under nulls*, not the *wrapper*.
2. **Physical column order.** `TableSpec.columns` is a `Vec`, and its order determines the Vortex
   `StructArray` field order and therefore the bytes. A decoder that reorders columns (or a source
   whose schema iteration order changes) moves the hash with no semantic change.
3. **Dtype width, and row-group leakage.** `Int32` vs `Int64` for the same values are different
   `ColumnData` variants. Separately, decoder-provided row-group boundaries leaking upstream can change
   block layout in multi-block ingest at `BLOCK_ROWS` boundaries.

So Finding 1 currently holds **by assumption at the canonicalisation boundary**, not by construction —
"assumed complete" rather than "gated complete". Every one of these is also a live cross-*producer*
determinism risk in its own right, independent of #403: ADR-0056 §5 already promises that the same
logical table written by pyarrow, polars and DuckDB seals to **one** `content_hash`, and paths 1 and 2
are exactly how that promise breaks.

## Decision / proposed approach

Treat these as first-class hazards and close them at the §2 `arrow → primitive` boundary:

- **H10 — nullable wrapping.** Normalise at the boundary: a column whose validity mask is all-true is
  carried as the plain variant, never as `Nullable`. Nullability becomes a property of the *data*, not
  of what the decoder happened to hand over.
- **H11 — column order.** Fix a canonical physical order at ingest, independent of decoder iteration
  order. Source order is the obvious candidate but is not obviously stable across producers; the
  alternative is a declared deterministic ordering. **This needs a decision, not a default** — it is
  visible in `read`/`select` output, so it is an ergonomics choice as well as a determinism one.
- **H12 — dtype width.** The §2 type map already fixes Arrow→Tessera widths; make it explicit that the
  *declared source width* governs, never a decoder's runtime narrowing, and that block boundaries are
  Tessera's (`BLOCK_ROWS`), never the source's row groups.

Then add them to the §5 hazard table with their rules, and give each a corpus fixture per ADR-0057 §5's
one-fixture-per-hazard floor.

## What already exists

- `docs/adr/0056-…` §5 — the H1–H9 table these extend, and §5's three-producer hash-equality fixture
  (`ingest_parquet_producers`), which is the test that would catch H10/H11 today if it existed
- `docs/adr/0056-…` §6a "Two conditions this decision depends on" — condition 2 is this issue
- `tessera/crates/tessera-io/src/table.rs:259-263` — the validity trailer
- `tessera/crates/tessera-io/src/table.rs` — `TableSpec.columns` as a `Vec`
- ADR-0057 §5 — the anti-vacuity expected-count guard the new fixtures must register with

## Scope

**P0**
- [ ] Decide the canonical column-order rule (source order vs declared deterministic order)
- [ ] Normalise all-valid → non-`Nullable` at the §2 boundary
- [ ] Pin dtype width to the declared source type; forbid row-group boundaries reaching block layout
- [ ] Add H10–H12 to the §5 hazard table

**P1**
- [ ] One corpus fixture per new hazard, registered in the expected-count guard
- [ ] Extend `ingest_parquet_producers` to assert the three-writer hash equality actually exercises
      nullable-wrapping and column-order divergence (today it may pass without touching either)

## Pitfalls

- **Column order is user-visible.** A canonical reordering that is right for determinism may surprise
  someone reading columns back in source order. Decide deliberately and document it in the §2 table.
- **Do not "fix" this by hashing a normalised form while storing a non-normalised one** — the digest
  must be over the bytes actually stored, or the seal stops meaning what it says.
- These are *encoder-boundary* rules; they must hold for **every** ingest path, not just the generic
  one, or vendor and generic ingest of the same logical table diverge.
- The all-valid→plain normalisation interacts with `Column.nullable` (`block/table.rs:41-44`): a column
  *declared* nullable whose data happens to be all-valid must not silently change its declared type.

## Acceptance criteria

- [ ] H10–H12 in the §5 table with rules, each with a corpus fixture
- [ ] Three-producer fixture demonstrably exercises nullable-wrapping and column-order divergence
- [ ] ADR-0056 §6a condition 2 discharged, so Finding 1 is gated-complete rather than assumed-complete

## References

- ADR-0056 §5 (H1–H9), §6a Finding 1 + condition 2; ADR-0057 §5 (Gate A/B, anti-vacuity)
- #403 / PR #405 (where these were found), #386 (ingest impl — this blocks its determinism claim)

Refs: #403
