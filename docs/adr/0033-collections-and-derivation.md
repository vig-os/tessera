# ADR-0033 — Collections, studies & the raw→derived boundary

Status: **Proposed** (2026-06-28) · Relates ADR-0022 (versioning DAG + `.tsra` container), ADR-0025
(ingest / provenance), ADR-0028 (stats/pyramid). Tracks #223; enables the cohort-scale reads in #225.

## Context
A real vendor acquisition is **multiple related tables**, not one. A GE listmode `.h5` carries
`singles` (~10⁸ raw photons), `coin_2p`/`coin_3p` (coincidences derived from singles),
`events_2p`/`events_3p` (validated/processed coincidences), and downstream a reconstructed volume — a
**raw → derived → derived DAG**. Today ingest maps *one* dataset → *one* single-block product; the
only cross-product grouping is the `study` label + provenance `sources` edges. A `.tsra` is already a
FAIR collection of **blocks**, but there is no collection of **products**, and no explicit place for
the derivation hierarchy that FAIR/WORM/distribution all want to act on.

## Decision
1. **Flat physical products.** Each `.tsra` stays ONE content-addressed product = **raw** *or* **one
   derived stage**. No monolithic nested container. (Per-product WORM retention, range-read /
   partial-fetch, dedup/regen, and OCI's own index primitive all want flat products.)
2. **Logical collections nest by reference.** A *collection* references its member products — and
   sub-collections, recursively — by `id`/`manifest_hash` plus the provenance DAG. Nesting lives at the
   logical (descriptor) layer, never by embedding one container in another. The existing `study` label
   and `sources` edges are the primitives.
3. **One logical collection, three projections** (not opinionated — all three are emitted from the same
   descriptor): **RO-Crate** `ro-crate-metadata.json` (FAIR discovery; already the discovery-export
   target), **OCI image index** (native manifest-of-manifests referencing N `.tsra` artifacts), and an
   **S3/MinIO prefix** (a prefix of independently range-readable `.tsra` objects).
4. **The raw→derived boundary drives WORM** (ties to #209): **raw → Compliance-mode** (immutable
   forever); **derived → Governance-mode** (regenerable, looser retention).

## Why
- The provenance/derivation relationship — not "how many files" — is the real axis; modeling it makes
  re-derivation cheap (one derived product re-seals; raw untouched) and retention honest.
- **Enables cohort-scale reads (#225):** a collection/catalog enumerates "all images" to fan out over,
  and the per-product stats (ADR-0028) let a query prune-before-fetch — answer "which products match?"
  from the tiny header without reading any data block, then fetch only survivors over the cloud
  range-read backend.

## Consequences
- A **collection descriptor** type + emit/read for the three projections (RO-Crate / OCI-index /
  S3-prefix).
- Ingest of a multi-dataset acquisition produces **raw + derived products + a study collection**, each
  with `derived_from` edges (the generic reader from #222 feeds the per-table products).
- A collection's identity is content-addressed over its members' addresses (a content-addressed
  catalog), so the collection itself is verifiable and versionable on the ADR-0022 DAG.
- Reconciles with ADR-0022 (a collection is a node/edge set on the versioning DAG) and ADR-0025
  (each member keeps its `ingested_from`/`derived_from` provenance).
