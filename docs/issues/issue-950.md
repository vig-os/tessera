---
type: issue
state: open
created: 2026-11-22T12:06:28Z
updated: 2026-11-22T12:06:28Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/950
comments: 0
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-23T10:16:29.199Z
---

# [Issue 950]: [feat(py): Builder.add_table cannot write a nullable column](https://github.com/vig-os/tessera/issues/950)

## Gap

The Python `Builder` cannot write a nullable table column. `Builder.add_table(name, columns, row_index=None)` takes
`columns` as `(name, code, le_bytes)` triples and builds every `Column` with `nullable: false`
(`tessera/crates/tessera-py/src/lib.rs`, `add_table`). A Python producer with NULLs has to fill them with a
sentinel, which the format forbids for floats (`NaN` is a measured value) and cannot express for integers.

The read side handles nullable columns since #804: `column()` / `table_dict()` return an `np.ma.MaskedArray`,
`table_arrow()` an Arrow array with a validity bitmap, `table()` polars nulls. The nullable test fixture is
therefore built from Rust (`tests/nullable_fixture.rs`), not from Python.

## Ask

1. `add_table` accepts a nullable column. Candidate shapes (decide in review): a 4th tuple member `validity`
   (`None`, or one byte per row like the native read side returns), or an `np.ma.MaskedArray` accepted by a
   pure-Python wrapper that splits it into values + validity before the FFI. Mirror the read side's
   `(le_bytes, numpy_code, validity)` so a read result can be written back unchanged.
2. The column is sealed with `nullable: true`; values under a NULL are canonicalised as on every other write path.
3. A round-trip test (masked array in, same mask out, Rust reader agrees), `api_drift.py` pins the accepted shape,
   and the book's bindings chapter / `write_example.py` show it.

Found while fixing #804 (out of scope there).

