---
type: issue
state: open
created: 2026-11-11T01:15:12Z
updated: 2026-11-22T11:45:01Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/804
comments: 1
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-23T10:16:31.525Z
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


---

# [Comment #1]() by [gerchowl]()

_Posted on November 22, 2026 at 11:45 AM_

**Owner decision (2026-11-22): nullable columns return a numpy masked array.**
- `column()` and the ndarray read paths return `np.ma.MaskedArray`, with mask=True at each NULL.
- `table_arrow` returns pyarrow arrays that carry a real validity bitmap.
- polars frames use native nulls.
- Non-nullable columns still return a plain `np.ndarray`, unchanged.
- `api_drift.py` pins the new shape.

This now comes before #828: the table-v2 corpus must hold a nullable column (design note §2.5), and today tessera-py-import and tessera-wheel-import fail on it on both arches (draft PR #949).

