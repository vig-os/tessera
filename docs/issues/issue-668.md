---
type: issue
state: open
created: 2026-10-25T11:18:54Z
updated: 2026-10-26T12:00:26Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/668
comments: 0
labels: feature, priority:medium, effort:large, area:io
assignees: none
milestone: none
projects: none
parent: 654
children: none
synced: 2026-10-27T07:16:25.460Z
---

# [Issue 668]: [feat(query): multi-product and Collection scans, local then object store (ADR-0062 PR3)](https://github.com/vig-os/tessera/issues/668)

Phase 3 of #654, designed in ADR-0062 §6, §7 and §8 "PR3".

- **Scope:** multi-product and Collection scans: member resolution (ADR-0049 §4), `manifest_hash` pinning,
  sub-collection flattening with the cycle guard, (product, block, row-group run) units, the schema policy
  (identical, or explicit common columns), the opt-in product column, table/scan-node statistics. Then the
  object store: the per-store I/O thread, the ranged `VortexReadAt`, the VerifyBlocks spool, and the
  `RangeSource` seam that lets a consumer's DataFusion `ObjectStoreRegistry` (object_store 0.13) feed the reader.
- **Acceptance:**
  - Equality: a collection scan equals the `UNION ALL` of its products' single-product scans.
  - Refusal before any row: a member `manifest_hash` mismatch, an unresolvable member, and a schema mismatch.
  - Object store: against MinIO (extending the `minio-range-read` flake check), results equal the local scan;
    pruned blocks issue zero GETs, in a key-mode zip too; the spool stays within its bound; a store taken from a DataFusion
    `ObjectStoreRegistry` reads the same rows as one opened by `open_url`.
  - Bounded memory: the ksb run of ADR-0062 §3 over a many-product collection.
- **Depends on:** PR1. **Split if needed:** (a) local multi-product; (b) object store.



