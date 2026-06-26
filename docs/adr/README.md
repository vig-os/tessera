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
| [0002](0002-concurrency-model.md) | **D2** concurrency — synchronous API; async kept dependency-internal (no tokio); object_store deferred | **Accepted** (as-built) |
| [0003](0003-schema-identification.md) | **D3** schema id — string product names, open-world (no numeric allocator) | **Accepted** (as-built) |
| 0007 | **D7** encryption-at-rest — **non-goal**: delegate to storage-layer SSE / dm-crypt; no per-block envelope in the format | Proposed (decide by P1) |

D6 (bindings vs validation) is settled in the ROADMAP itself (P7/P8) and needs no separate ADR.
