---
type: issue
state: open
created: 2026-11-11T01:15:12Z
updated: 2026-11-11T01:15:25Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/804
comments: 0
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:40.332Z
---

# [Issue 804]: [bug(py): a nullable table column reads back with its validity mask appended to the values](https://github.com/vig-os/tessera/issues/804)

## Defect (found by code reading during the #801 consumer gap analysis, not yet executed)

A nullable table column read from Python comes back with its validity mask appended to its values.

- `ColumnData::to_le_bytes` serialises `Nullable` as `values ‖ pack_validity(validity)`, i.e. a `ceil(rows/8)`-byte trailer (`tessera/crates/tessera-io/src/table.rs:320-324`).
- `read_table`, `read_table_column` and `read_logical_table_column` hand that buffer to Python together with `numpy_code()` only, which for `Nullable` is the inner code (`tessera/crates/tessera-py/src/lib.rs:135-200`, `table.rs:168`).
- `_ndarray(buf, code)` does `np.frombuffer(buf, "<" + code)` (`tessera/crates/tessera-py/python/tessera/__init__.py:43-69`). It never splits the trailer off and never builds a mask.

Expected effect: for a nullable `i4` column of `n` rows the buffer is `4n + ceil(n/8)` bytes. `frombuffer` raises when that isn't a multiple of 4. When it is, it silently returns extra elements built from mask bits. NULL slots come back as `0` with no mask in any case.

No Python test covers a nullable column (`tests/*.py`), and the Python `Builder` can't write one, so nothing exercises this today. The #799 consumers will hit it: gPET-sim `annih_*` and sim2listmode `pos_*`/`annih_*`/`first_*` are modelled as nullable in `docs/spikes/801-consumer-schemas/`.

## Ask
1. A failing test first: read a nullable column (for example a sealed fixture written from Rust) through `table_dict` / `column`.
2. Return a numpy masked array, or a `(values, mask)` pair, and a pyarrow array with a validity bitmap from `table_arrow`.
3. Extend `api_drift.py` to cover it.


