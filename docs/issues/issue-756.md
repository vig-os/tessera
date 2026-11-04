---
type: issue
state: open
created: 2026-11-03T13:13:43Z
updated: 2026-11-03T13:13:43Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/756
comments: 0
labels: refactor, priority:medium, area:core
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-04T08:35:53.499Z
---

# [Issue 756]: [refactor(core): one ColumnDType owner for the table dtype vocabulary (7 hand-matched copies, unvalidated at seal)](https://github.com/vig-os/tessera/issues/756)

Found by an independent SOLID/DRY review of dev 74fc409 (astra + Fable, cross-checked by a third verifier). Reported independently by both reviewers.

## Defect (DRY, format detail)
The table column dtype vocabulary is a string set matched by hand at many sites, and core never validates it:
- `tessera-io/src/table.rs`: `ColumnData` → code (~156-167), `from_le_bytes` (~375-388), `dtype_size` (~547-556), the `validate_dtype` allowlist (~564-566), `empty_column_with_capacity` (~678-690). Its error message (~692) lists `i1…f8` and has already drifted: it omits `b1`/`str`.
- `tessera-ingest/src/canonical.rs`: `ACCEPTED_DTYPES` (~572-573; its doc claims "exactly one definition"), and `empty_column` (~581-595), a copy of the io one.
- `tessera-ingest/src/ge_hdf5.rs` ~83-92: a third empty-column match, without `b1`/`str`.
- `tessera-cli/src/sql.rs` ~100-111: `numpy_to_arrow`, a fourth closed match.
- `tessera-py/python/tessera/__init__.py` ~47-62: decodes the codes by hand.
- `tessera-core/src/block/table.rs` ~14-15: `pub dtype: String`. `seal` does not validate it, so core will seal a column dtype that no reader accepts.

Related, the same ownership problem: the zero-under-null rule is two full dtype matches, `ingest/canonical.rs` `normalise_null_slots` (~289-320) and `io/table.rs` `zero_masked` (~344-370, private). They differ on short masks.

## Failure
A dtype added or renamed in io (e.g. complex, #419) is silently refused by csv/ge-hdf5/sql, mis-decoded in Python, and accepted by core's seal.

## Fix shape
- A `tessera_core::block::table::ColumnDType` enum (`FromStr` / `as_code` / `byte_width` / `is_fixed_width`), validated on construction and deserialisation like `BlockName`. io, ingest and cli match on the enum, and Python gets its code→numpy table from `_native`.
- One owner for the null-slot rule, used by both ingest and io.

The sealed strings are unchanged, so no ADR is needed. This changes the public API of `Column.dtype`.

