# ADR-0051 — GE listmode HDF5 decomposition: tables as products, not a monolith

**Status:** Proposed (2026-07-02) — decided during the DUPLET first-user FAIR ingest (#305). Relates
ADR-0033 (collections & raw→derived boundary), ADR-0049 (recursive collections), ADR-0035
(declarative ingest), ADR-0025 (ingest model). Consumes the GEDDF column dictionary (#307) and the
opt-in quantization transform (#310). Fetch-granularity/integrity analysis relates ADR-0034 (read
path) and the OCI/S3 transport (#291/#292).

## Context

A DUPLET acquisition's GE Discovery MI Gen2 "singles" file is a ~120 GB **HDF5 monolith** holding
**11 datasets** produced by a pre-Tessera writer that processed the raw **BLF listmode** once and
bucketed it into tables:

| dataset | rows | size | role |
|---|---|---|---|
| `raw_data/singles` | 7.9 B | 81 GB | raw detections |
| `proc_data/coin_2p` / `coin_3p` | 1.24 B / 340 M | 18 GB / 7 GB | raw coincidences |
| `proc_data/events_2p` / `events_3p` | 158 M / 10 M | 5 GB / 447 MB | reconstructed events (en/vtx/lt) |
| `raw_data/time_markers` · `coin_counters` · `lyso_map` · `coin_size_dist` · `table_positions` | ≤ 416 k | **~21 MB total** | shared index / small |

`tessera ingest ge-hdf5` reads **only** `events_2p`/`events_3p` — 2 of 11 — so a naive ingest prunes
9 datasets, violating "no pruning / full dumps". The monolith is an artifact of the old writer, not a
Tessera-native shape.

## Decision

### 1. One product per table (`.tsra`), NOT one product per file

Each of the 11 datasets becomes its **own** `.tsra` product. Rationale is fetch-granularity +
integrity, measured against the OCI/S3 transport:

- OCI push maps a `.tsra` to **one monolithic layer = one sha256**. A **pruned/partial** fetch cannot
  verify a whole-blob digest → the OCI-layer integrity check dies on partial fetch.
- Tessera's **blake3 Merkle + seal** survives partial fetch (per-block digest under `content_hash`,
  sealed by `manifest_hash`), and the range-reader verifies fetched blocks — **but that is Tessera's
  reader over `s3://`/`http`, not a generic `docker pull`** (whole-blob only).
- Therefore the **prune boundary = the artifact boundary**. To "pull only `events`, leave `singles`
  and `coin` behind, OCI-natively and with integrity," the unit of fetch must be the unit of artifact
  → **one `.tsra` per table**.

Reserve **multi-block-in-one-`.tsra`** for things that are genuinely one product: always read
together **and** versioned together **and** never fetched separately (e.g. an image volume + its
per-slice rescale sidecar, ADR-0300/#300). The 11 tables fail that test — consumers routinely want a
subset.

**Tight analytical coupling does NOT force co-location:** cross-object DataFusion / `LogicalTableView`
already span `.tsra` and prune across them, so `events ⋈ coin` joins across two artifacts. Same-`.tsra`
buys only atomic sealing + single-open locality — not joint-analysis capability.

### 2. The acquisition is a Collection; the derivation DAG is cohort-dependent

An acquisition's tables are bound by a **Collection** (ADR-0033), recursively up to patient → cohort
(ADR-0049). The `derived_from` provenance root is **not uniform** across the cohort:

- **~DP01–DP06** (exact boundary from the data owner): `BLF (singles) → coincidences → events` — the
  BLF listmode is the raw root, preserved as a `blob` (cold tier).
- **DP07+**: the data arrives as an **already-extended coincidence stream from GE** →
  `GE extended-coinc → coin (filtered) → events` — the **GE coincidence stream is the root, not the
  BLF singles**.

A blanket `derived_from = [BLF]` would misattribute DP07+ provenance. The ingest DAG must therefore be
**per-cohort configurable** (declared in the ADR-0035 spec), not a fixed template. The bucketing
itself is a DataFusion sort over the raw listmode — reconstructable, not trusted-because-vendor.

### 3. Shared sub-datasets: duplicate freely (measured)

`time_markers` (14 MB), `coin_counters` (6.5 MB), `lyso_map` (0.9 MB), `coin_size_dist` (~1 KB) total
**~21 MB = 0.017 %** of the 120 GB — four orders of magnitude below the event/singles payloads. So the
split-blocks-vs-split-files decision is **not** constrained by shared-table cost:

- **Duplicate** the shared tables into each self-contained per-table `.tsra` (keeps every artifact
  independently pruneable + verifiable). Content-addressing **dedups the identical blob** at the OCI
  registry + local cache anyway — free at rest.
- Preferred over hoist-and-reference, which adds a fetch-dependency edge (a pruned `events` fetch would
  also have to pull the `time_markers` artifact).

The HDF5 is stored **uncompressed** (120 GB uncomp == on-disk), so pcodec/Vortex shrink these
substantially on ingest.

### 4. Columns are annotated + optionally quantized at ingest

Table columns carry the fd5 I1/I2 triad (short_name/description/**unit**) from the domain-owner **GEDDF
dictionary** (#307), and the reconstructed float columns (`en` keV, `vtx` mm, `lt` ns) are optionally
requantized to int16 at the physical resolution (#310) — opt-in, default byte-identical.

## Consequences

- A regulator/auditor reaches any table's raw source via its `derived_from` chain to the preserved
  BLF (or GE-coinc) blob. Cohort-correct because the DAG is per-patient.
- A cohort/cluster read pulls only the tables it needs (e.g. `events_3p` for positronium-lifetime
  analysis) without fetching the 81 GB `singles` — OCI-natively, integrity intact per artifact.
- The old `ge-hdf5` "events-only" path is **superseded** for full-fidelity ingest by a spec that
  declares every table as a product with its cohort-correct `derived_from`.

## Alternatives considered (and why they lost)

- **Blob the whole `.h5` verbatim (one cold artifact).** Preserves every byte but is un-queryable and
  un-pruneable — you fetch 120 GB to read 10 M events. Keep the BLF blob as the *raw* cold tier;
  the tables are hot products.
- **One `.tsra`, 11 blocks.** Loses OCI-native subset pull (prune only via Tessera's range-reader over
  a raw object, not `docker pull`) and couples all tables' lifecycle/versioning.
- **Multi-layer single OCI artifact (block→layer).** Would give per-table selective-layer pull + per
  layer sha256 within one artifact — best-of-both, but Tessera push is single-layer today (#291). A
  possible future refinement, not the v1.

## References

- #305 (this ADR), #307 (column dictionary), #310 (quantization), #304 (collection assembly / OCI-query).
- ADR-0033 (collections), ADR-0049 (recursive collections), ADR-0035 (declarative ingest), ADR-0034
  (read path), #291/#292 (OCI/S3 transport).
- Domain source: `MorePET/ge-discovery-data-framework` (`docs/DescriptionCSV.md`, `lib/GEHDF5`),
  GE `petCoincLinkEvents.h`.
