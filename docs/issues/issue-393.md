---
type: issue
state: open
created: 2026-08-19T14:02:27Z
updated: 2026-09-28T17:59:23Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/393
comments: 0
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:41.079Z
---

# [Issue 393]: [feat(ingest): directory-shaped sources — layout detection → one array product vs a collection](https://github.com/vig-os/tessera/issues/393)

Split out of the #386 generic-ingest spike (ADR-0056 §7 scopes it out explicitly).

## Motivation

Every row of #386's source→primitive routing table assumes a **single file**. Real scientific data on
disk almost never is:

- A BIDS dataset — a tree of thousands of `.nii.gz` + JSON sidecars + `participants.tsv`.
- A DICOM study — a directory of `IM_0001` … `IM_1000`, often several series interleaved.
- Raw electrophysiology (SpikeGLX / Open Ephys) — `run.ap.bin` + `run.ap.meta`; the `.meta` **is** the
  header the `.bin` lacks, so ingesting either alone is meaningless.
- Microscopy acquisitions — `experiment/ch0/z0000.tif` … `experiment/ch3/z0500.tif` + a JSON.
- Training-loop dumps — `arr_000.npy` … `arr_999.npy`.
- OME-TIFF `BinData` split across sibling files with UUID cross-references.

Without this, a user's first real ingest is "write me a TOML spec" — the exact friction the AX
onboarding audit (#390) already scored 2/5 on write-your-own-data.

## Decision already taken (ADR-0056 §7) — this issue implements it

ADR-0056 pins the **semantics**; this issue owns the **detection**. The shape rule applies one level
up from files:

- Directory members that together form **one dense N-D grid** (a DICOM series, a z-stack, a numbered
  `.npy` sequence) → **one array product**, stacked into a single Zarr block. Generalises the
  existing `dicom-series` stacking.
- Directory members that are **independent acquisitions** → a **collection** of `.tsra`, one product
  per acquisition (ADR-0033/0049).

## Scope

- P0: `tessera ingest <DIR>` routes to product-vs-collection by the rule above; numbered-sequence
  detection (`name_%04d.ext`) for `.npy` / single-page TIFF / DICOM.
- P0: sidecar pairing — a data file plus a same-stem header/metadata file ingests as **one** product.
- P1: BIDS layout recognition (`dataset_description.json` → collection; entity-parsed names).
- P1: multi-series DICOM directory → collection of per-series array products.
- P2: OME-TIFF multi-file UUID/`TiffData` cross-references.

## Pitfalls

- Detection heuristics are a large design surface — it is why ADR-0056 scopes them out rather than
  guessing. Every heuristic must obey ADR-0056's rule that shape may be inferred but **semantics may
  never be auto-decided**; an ambiguous layout asks, it does not guess.
- Sort order for numbered sequences must be numeric, not lexicographic, and must be recorded in
  provenance — the stacking order is part of the sealed bytes.
- A partially-readable directory must fail loudly, never silently ingest a subset.
- Interaction with the `--spec` TOML engine: detection should **emit** a spec, not bypass it, so the
  declarative path stays the single source of truth (ADR-0035).

## Acceptance criteria

- [ ] A directory of numbered `.npy` files ingests to one array product whose values equal the
      stacked source.
- [ ] A directory of two DICOM series ingests to a collection of two array products.
- [ ] A `.bin` + `.meta` pair ingests as one product; the `.bin` alone errors with a message naming
      the missing sidecar.
- [ ] Detection emits a `--spec` TOML that reproduces the same result when run directly.
- [ ] Cross-arch determinism holds for a stacked directory ingest.

## References

- #386 generic ingest · ADR-0056 §7 (scope boundary + the shape rule)
- ADR-0033 / ADR-0049 (collections, recursion) · ADR-0035 (declarative spec engine)
- #390 AX onboarding audit (write-your-own-data gap) · #389 ingest cookbook
