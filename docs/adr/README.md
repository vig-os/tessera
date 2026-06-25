# Tessera — Architecture Decision Records

Each ADR records one load-bearing decision: context, the decision, why, and consequences. They
correspond to the open-decision register in `tessera/docs/ROADMAP.md` (D1–D7). Status: Proposed /
Accepted / Superseded.

| ADR | Decision (register id) | Status |
|---|---|---|
| [0020](0020-canonical-encoding-and-identity.md) | **D4** canonical encoding (RFC 8785 JCS) + **D5** identity model (id / content_hash / manifest_hash) + manifest & BlockRef schema | **Accepted** |
| [0022](0022-versioning-and-container.md) | **D1** fd5 supersession (done) · versioning DAG · `.tsra` container spec | **Accepted** |
| 0002 | **D2** concurrency model — sync `core` / async `io` (tokio + `object_store`) + rayon encode pool; `spawn_blocking` boundary | Proposed (decide by P3) |
| 0003 | **D3** schema-id allocation — per-schema monotonic ids + `<plugin>:<id>` namespacing + reserved ranges | Proposed (decide by P1) |
| 0007 | **D7** encryption-at-rest — **non-goal**: delegate to storage-layer SSE / dm-crypt; no per-block envelope in the format | Proposed (decide by P1) |

D6 (bindings vs validation) is settled in the ROADMAP itself (P7/P8) and needs no separate ADR.
