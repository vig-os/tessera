---
type: issue
state: closed
created: 2026-06-30T17:49:44Z
updated: 2026-11-18T14:56:02Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/235
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-19T09:52:45.421Z
---

# [Issue 235]: [spike: migrate one PET/CT study → fine-grained --meta-tagged .tsra collection (recon-zarr + ROI + original-as-blob)](https://github.com/vig-os/tessera/issues/235)

Spike the canonical **migration shape** for a real DUPLET PET/CT study, structured so it's cache- and cohort-friendly (per the caching-arch discussion).

## Target shape (a *collection*, not a monolith)
One study → a Tessera **collection** of fine-grained products so block-level de-dup + prune-before-fetch work:
- `recon` (CT volume) → Zarr v3 + pcodec block — *normalized, hot, range-readable*
- `recon` (PET volume) likewise
- `roi` table (if available)
- the **original DICOM series / `.l64`** preserved as a **`blob`** product — *bit-faithful, cold, archival*
- provenance edges: each normalized product `derived_from` the blob it came from
- `--meta study=… modality=… cohort=…` on every product (satisfies the `blob` schema's recommended `study`; makes the collection cohort-pruneable)

## Deliverables (spike, no real PHI committed)
- A declarative **ingest spec (TOML)** expressing the above (dicom-series + blob members, derived_from edges, metadata).
- A worked **demo on synthetic/corpus stand-in data** (NOT real DUPLET — that's manual-bench-only, PHI) proving the collection seals, verifies, and is cohort-pruneable.
- A short **operator runbook**: how to point it at `~/Data/HDD/...` and emit to `Data/HDD/tsra` locally.
- Notes on blob-cold / normalized-hot tiering for the eventual S3/OCI push.

Builds on: the blob tier (#229/#231), `--meta` + recommended-field severity (#233), collections/derivation (ADR-0033/0035).
---

# [Comment #1]() by [gerchowl]()

_Posted on July 1, 2026 at 01:40 PM_

Delivered in #245 (merged to spike/tessera-core).

