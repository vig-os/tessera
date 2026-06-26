# Tessera — Architecture Decision Records

Each ADR records one load-bearing decision: context, the decision, why, and consequences. They
correspond to the open-decision register in `tessera/docs/ROADMAP.md` (D1–D7). Status: Proposed /
Accepted / Superseded.

| ADR | Decision (register id) | Status |
|---|---|---|
| [0020](0020-canonical-encoding-and-identity.md) | **D4** canonical encoding (RFC 8785 JCS) + **D5** identity model (id / content_hash / manifest_hash) + manifest & BlockRef schema | **Accepted** |
| [0022](0022-versioning-and-container.md) | **D1** fd5 supersession (done) · versioning DAG · `.tsra` container spec | **Accepted** |
| [0023](0023-array-block-payload.md) | Array block payload — Zarr v3 + pcodec, serialized as one deterministic blob (P3/S5) | **Accepted** |
| [0024](0024-table-block-payload.md) | Table block payload — a single deterministic Vortex file (P3/S5) | **Accepted** |
| [0025](0025-ingest-model.md) | Ingest model — normalise at the door, lossless native dtype, provenance-rooted (P5) | **Accepted** |
| [0026](0026-streaming-table-ingest.md) | Streaming chunked table writes for >RAM ingest — one always-chunked deterministic encoder (#208) | **Proposed** |
| [0027](0027-sub-block-merkle-chunk-index.md) | Sub-block Merkle + content-addressed chunk-index block — per-chunk confirmation + pruning (#214) | **Superseded by 0028** (design shipped as 0028 §3) |
| [0028](0028-unified-hierarchy.md) | The unified hierarchy — recursive MMR Merkle + multiscale `{hash,stats}` pyramid + derived sidecars + fused streaming (#215); supersedes ADR-0020 flat root | **Accepted** |
| [0029](0029-data-model-composition.md) | Data model — composition over inheritance · N-D blocks · multi-dimensional acquisitions (A vs B) · ROI representation · substrate-by-nature (#216) | **Accepted** |
| [0030](0030-spatial-referencing.md) | Spatial referencing — one voxel→world affine + named frame; spacing & OME-Zarr per-level transforms derived; registration = provenance edge; LPS canonical (#217) | **Accepted** |
| [0031](0031-sparse-representation.md) | Sparse data — COO table for scatter (reuses Merkle/stats/pushdown) · dense-chunked + stat-prune for block sparsity · threshold measured · materialize-don't-densify (#218) | **Accepted** (as-built) |
| [0032](0032-referenced-coordinates-and-quantities.md) | Referenced coordinates & quantities — one `(transform, unit, frame)` descriptor unifying space/time/intensity/parametric; UCUM units; store-don't-compute; generalises ADR-0030 (#219, #220) | **Accepted** |
| [0002](0002-concurrency-model.md) | **D2** concurrency — synchronous API; async kept dependency-internal (no tokio); object_store deferred | **Accepted** (as-built) |
| [0003](0003-schema-identification.md) | **D3** schema id — string product names, open-world (no numeric allocator) | **Accepted** (as-built) |
| 0007 | **D7** encryption-at-rest — **non-goal**: delegate to storage-layer SSE / dm-crypt; no per-block envelope in the format | Proposed (decide by P1) |

D6 (bindings vs validation) is settled in the ROADMAP itself (P7/P8) and needs no separate ADR.
