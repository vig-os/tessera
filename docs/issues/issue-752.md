---
type: issue
state: open
created: 2026-11-03T13:13:24Z
updated: 2026-11-03T13:13:24Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/752
comments: 0
labels: refactor, priority:medium, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-04T08:35:55.623Z
---

# [Issue 752]: [refactor(io): array dtype vocabulary and widening rule re-implemented beside core::DType (duplicate decode tables)](https://github.com/vig-os/tessera/issues/752)

Found by an independent SOLID/DRY review of dev 74fc409 (astra + Fable, cross-checked by a third verifier). Reported independently by both reviewers.

## Defect (DRY / OCP)
`tessera-core::dtype::DType` exists (`dtype.rs` ~44-70: `as_str`, `is_integer`, `byte_width`), but its only production consumer is `DType::is_supported` (`core/block/array.rs` ~335). io re-implements both the vocabulary and the storage widen/narrow rule:
- `tessera-io/src/array.rs`: `ArrayData::dtype()` (~142-153), `numpy_code()` (~237-248), `from_le_bytes` (~302-314).
- Two identical string-dispatch decode tables: whole-array `decode` (~614-626) and ROI `decode_subset` (~653-671). Each carries the int8→i16, float16→f32 and bool→u16 widening (`narrow`, `narrow_f16`, `narrow_bool`), mirrored by the encoder (~478-493).
- `tessera-cli/src/nav.rs` ~195: a hard-coded single-byte set.

## Cost
A storage-mapping change, or a new dtype (#419 complex), must be carried into six matches in io plus nav. The whole-array and ROI paths can drift, and then decode the same block differently.

## Fix shape
- `ArraySpec.dtype: DType` (serde as a string) and `ArrayData::dtype() -> DType`.
- One `DType::storage_widening()` owning narrow/widen.
- One decode dispatch, parameterised by `retrieve_all` / `retrieve_region`.

No ADR is needed (sealed strings unchanged).

