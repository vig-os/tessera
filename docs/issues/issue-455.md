---
type: issue
state: open
created: 2026-09-28T20:04:23Z
updated: 2026-09-29T06:53:09Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/455
comments: 0
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:31.338Z
---

# [Issue 455]: [land the `IngestOutput` seam so vendor backends stop reaching the seal directly (ADR-0057 §8)](https://github.com/vig-os/tessera/issues/455)

ADR-0057 §8 mandates a single output type every ingest backend returns, and a rule that backends may not seal:

```rust
pub enum IngestOutput {
    Table { columns: Vec<(Column, ColumnData)>, spec: TableSpec },
    Array { spec: ArraySpec, data: ArrayData },
    Blob  { bytes: Vec<u8>, media_type: Option<String> },
}
```

Today `dicom.rs` / `ge_hdf5.rs` / `nifti.rs` / `raw.rs` each reach the seal their own way. #386's generic lanes go through a shared `canonical::to_table_product` seam, which is the same idea at table scope — so the pattern now exists in-tree and the vendor bodies have a migration target.

ADR-0057 §8's acceptance test is deliberately strict and is the valuable part: **`ge_hdf5.rs` and `nifti.rs` must be rewritten on top of the public waist functions with their existing golden hashes unchanged.** If Tessera's own decoders cannot sit on the waist, no third party can either — the dogfood *is* the audit.

Also in scope (ADR-0057 §9A): making `tessera-io`'s `seal_table` / `seal_array` / `seal_blob` a stated, unstable-until-1.0 public surface, rather than an internal boundary described as an extension point.
