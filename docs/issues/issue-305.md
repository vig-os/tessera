---
type: issue
state: closed
created: 2026-07-02T14:26:32Z
updated: 2026-09-28T17:54:55Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/305
comments: 2
labels: discussion, effort:large
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:51.331Z
---

# [Issue 305]: [discussion(ingest): GE listmode HDF5 monolith — model its 11 tables as BLF-derived products, not one file](https://github.com/vig-os/tessera/issues/305)

## Context — first-user DUPLET FAIR shakedown

Each DP01 session carries a ~120 GB GE Singles **HDF5** (`0__Singles/…/*.h5`) plus a 126 GB `.dat`
twin. `h5ls` shows **11 datasets** in one file:

```
/proc_data/events_2p   {158,366,867}      /proc_data/coin_2p   {1,237,008,905}
/proc_data/events_3p   {9,974,503}        /proc_data/coin_3p   {340,114,474}
/raw_data/singles      {7,902,762,532}    /lyso_map {544,54,4}
/raw_data/coin_counters, /raw_data/time_markers, /raw_data/table_positions, /proc_data/coin_size_dist
```

The `ingest ge-hdf5` CLI reads **only `events_2p`/`events_3p`** (2 of 11) and via a flat `--dataset`
name (these live under `/proc_data/`). So a plain `ge-hdf5` ingest **prunes 9 datasets** — violating
"full dumps / no pruning".

## Design direction (from discussion)

The `.h5` is a **pre-tsra artifact**: one old writer processed the raw **BLF listmode** once and
bucketed it into these 11 tables. In the tsra model there's little reason to keep the monolith —
each table should be its **own product (or multiple blocks / a sub-collection)**, with **`derived_from`
tracing back to the BLF** (the true raw source-of-record, preserved as a blob). The bucketing itself
is just a **DataFusion query** (multi-query sort) over the raw listmode.

## Ask (deferred — needs design)

- Decide table-per-`.tsra` vs multi-block vs sub-collection for the 11 datasets.
- Wire `derived_from → BLF` provenance; reconstruct tables via DataFusion rather than trusting the
  vendor monolith.
- The spec engine already has `format = "hdf-compound"` with a `dataset` field — confirm it resolves
  nested paths (`/proc_data/events_3p`) and handles the non-compound datasets (`lyso_map`, counters).

Deferred by decision; filed for follow-up. Found during: DUPLET first-user FAIR ingest (DP01).

---

## UPDATE — provenance root is NOT uniform across the cohort

The `derived_from → BLF` chain does **not** hold for every patient. Two distinct lineages exist:

- **~DP01–DP06** (exact boundary TBC): `BLF (singles) → coincidences → events`.
  The BLF listmode singles are the raw root; coinc and events are derived downstream.
- **DP07+**: the data arrives as an **already-extended coincidence stream from GE** →
  `GE extended-coinc → coinc (filtered) → events`.
  Here the root is the **GE coincidence stream, not the BLF singles** — so a blanket
  `derived_from = [BLF]` would misattribute provenance.

**Implication:** the ingest/derivation DAG must be **per-cohort (per-patient) configurable**, not a
fixed template. The spec/engine needs to express "this table's parent is X" where X differs by dataset
generation. Get the actual DP0x boundary + the two canonical DAGs from the data owner before wiring.

## Shared sub-datasets — split-blocks vs split-tsra (spike)

Several of the 11 datasets likely **share** structures (e.g. `time_markers`, `table_positions`,
`coin_counters`) referenced by multiple tables. Open question for the spike:

- **Split blocks**: one `.tsra` holds the related tables as multiple blocks, shared structures stored
  **once** (dedup by content hash) — tightest, but couples their lifecycle/versioning.
- **Split `.tsra` files**: one product per table, shared structures either duplicated (content-addressed
  dedup at the OCI/object layer still collapses them) or referenced — cleaner boundaries, looser coupling.
- **Sub-collection** (now viable — recursive collections landed, #293/PR #306): a listmode
  sub-collection binding per-table members, shared block referenced across members.

Spike needed to decide, driven by how tightly the tables are actually coupled (do consumers read them
together, or independently?).

---

## Granularity decision framework — driven by pruned-fetch + integrity

Grounded in the integrity model (`manifest.rs`, `registry.rs`, `cloud.rs`):

- **OCI push = one monolithic layer = whole `.tsra`, one sha256.** A *pruned/partial* fetch **cannot**
  verify that whole-blob digest → the OCI-layer integrity check dies on partial fetch.
- **blake3 Merkle + seal survives partial fetch**: `content_hash` = Merkle root over per-block digests;
  `manifest_hash` seals the manifest (incl. every `BlockRef` digest). Fetch `{manifest + one block}` →
  verify seal → verify that block's bytes vs its digest. The range-reader (`cloud.rs`) already does this
  over `s3://`/`http` — but it is **not** a generic `docker pull` (which is whole-blob only).

### Consequence: fetch boundary = artifact boundary

| Want | Storage granularity |
|---|---|
| Pull a subset (only events), leave singles/coinc behind, **OCI-native + integrity, any client** | **one `.tsra` per table** (artifact = unit of both prune and verify) |
| Prune within a single `.tsra` at block level | only via tessera's **range-reader** (block-blake3) over `s3://`/`http`; **not** via `docker pull` |
| Selective pull within one artifact, per-part sha256 | **multi-layer artifact** (map block→layer) — *not built today* (push is single-layer) |

### Recommended model

- **Product = `.tsra`** — each emitted table/file its own FAIR envelope (identity, seal, OCI artifact,
  signing, provenance, **OCI-native pruned fetch**).
- **Collection = the acquisition** — binds the acq's products (recursive → patient → cohort, #293/#306);
  a queryable binding, not a monolith.
- **Multi-block same-`.tsra`** reserved for genuinely-one products: always-read-together **and**
  version-together **and** never-fetched-separately (e.g. volume + per-slice rescale sidecar #300).
- **Tight analytical coupling does NOT force same-`.tsra`** — cross-object DataFusion / `LogicalTableView`
  (#297) spans artifacts and prunes across them. Same-`.tsra` only buys atomic seal + single-open locality.
- **Shared sub-datasets** (`time_markers`): prefer duplicate-into-each-self-contained-artifact
  (content-addressing dedups the identical blob at registry + cache — free at rest, keeps each artifact
  independently pruneable/verifiable) over hoist-and-reference (which adds a fetch dependency edge).

Rule of thumb: **prune boundary = artifact boundary = `.tsra`; "belongs to this acq" = collection.**

---

## Shared-table duplication cost — measured (no bench needed)

Per-dataset sizes in the DP01 Singles `.h5` (uncompressed == on-disk; the HDF5 is **not** compressed):

| dataset | rows | size | role |
|---|---|---|---|
| raw_data/singles | 7.9 B | **81 GB** | big |
| proc_data/coin_2p | 1.24 B | 18 GB | big |
| proc_data/coin_3p | 340 M | 7 GB | big |
| proc_data/events_2p | 158 M | 5 GB | big |
| proc_data/events_3p | 10 M | 447 MB | big |
| **raw_data/time_markers** | 416 k | **13.7 MB** | shared |
| **raw_data/coin_counters** | 408 k | **6.5 MB** | shared |
| lyso_map | — | 0.9 MB | shared |
| coin_size_dist | 60 | ~1 KB | shared |
| table_positions | 0 | 0 B | empty |

The shared/small tables total **~21 MB = 0.017%** of the 120 GB. **Duplicating them into each per-table
`.tsra` is free** — 4 orders of magnitude below the event/singles payloads, and content-addressing
dedups the identical blob at the registry/cache anyway. So the split-blocks-vs-split-files decision is
**not** constrained by shared-table cost → decide it purely on prune/verify granularity (framework
above). Also note: the HDF5 stores these **uncompressed**, so tessera (pcodec/Vortex) will shrink them
substantially on ingest.

---

# [Comment #1]() by [gerchowl]()

_Posted on July 3, 2026 at 09:49 AM_

## Correction — the DP01–06 raw root is the `.dat`, not the BLF

Verified against GEDDF (`MorePET/ge-discovery-data-framework` `hit-parser` reads **raw hits**) + the acquisition `.ini` (`acq_mode = singles`, DAQ config `acq.cfg.LYSO4x9_6_SIPMGEN1`):

```
DAQ → .dat (126 GB, raw hit stream / singles)
        → GEDDF hit-parser (+ gain/ctc/energy/crystalmap cals)
             → .h5: raw_data/singles → proc_data/coin_2p·3p → proc_data/events_2p·3p
```

So the `.h5` tables' raw root is the **`.dat`** (the custom DAQ singles acquisition). The **BLF**
(`951/952 GEMS_PET_LST`) is a **separate, standard GE clinical listmode** stream — NOT the source of
the `.h5`. ADR-0051 §2's "DP01–06 BLF-singles-rooted" should read **`.dat`(raw DAQ hits)-rooted**;
DP07+ is still the GE-extended-coincidence variant (confirm which file). The cohort-split shape stands;
only the identity of the root file changes.

**Acquisition provenance is a sidecar `.ini`** (`[DAQ]` section: exam, acq_mode, acq_mode_config, and
the cal files gain/ctc/energy/crystalmap + start/stop time) — this is exactly the instrument/acquisition
software+settings provenance that should be elevated into the product manifest per #324.

---

# [Comment #2]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: ADR-0051 landed (c431c8e 'docs(adr): ADR-0051 — GE listmode HDF5 decomposition (tables as products) (#305) (#320)'). Discussion resolved.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

