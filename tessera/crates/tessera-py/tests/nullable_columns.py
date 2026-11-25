"""Python reads of nullable table columns (#804).

Run as: python3 nullable_columns.py <nullable_obs.tsra> <nullable_obs.expected.json>

The fixture is a partitioned table ``obs`` over the leaves ``obs/0000`` (5 rows) and ``obs/0001`` (6):
11 rows, so no leaf is a multiple of 8 and a ``ceil(rows/8)``-byte validity trailer left on the values
cannot pass by luck. ``i`` (u4) is not nullable; ``a`` (i4), ``e`` (f8, with a present NaN), ``t`` (u2,
declared nullable but holding no NULL) and ``z`` (f4, NULL on every row of the first leaf) are. Both
files are built by ``tests/nullable_fixture.rs``; the expected file is what the Rust reader decodes,
each column's values as little-endian hex plus its validity mask, and every read path here must
reproduce it bit for bit. The fixture's ``content_hash`` is pinned so a swapped file can't pass.

The contract (owner decision on #804): a nullable column reads as ``np.ma.MaskedArray`` with
``mask=True`` at each NULL through ``column()``, ``logical_column()`` and ``table_dict()``, whether or
not it holds a NULL; a non-nullable column is a plain ``np.ndarray``; ``table_arrow()`` carries a real
validity bitmap; ``table()`` (polars) carries native nulls, and a present NaN stays a NaN.
"""

import json
import math
import sys

import numpy as np
import polars as pl
import pyarrow as pa

import tessera

CONTENT_HASH = "blake3:4467468bd693813cb7bd288b3352e0f362f31ae76090c8377ed53369986b153e"

r = tessera.open(sys.argv[1])
r.verify()
with open(sys.argv[2]) as f:
    expected = json.load(f)
assert r.manifest()["content_hash"] == CONTENT_HASH, r.manifest()["content_hash"]
assert r.block_names() == ["obs/0000", "obs/0001"], r.block_names()
ROWS = expected["rows"]
assert ROWS == 11

WANT = {}
for c in expected["columns"]:
    values = np.frombuffer(bytes.fromhex(c["values_le_hex"]), "<" + c["dtype"])
    valid = None if c["validity"] is None else np.array(c["validity"], dtype=bool)
    WANT[c["name"]] = (values, valid)
assert list(WANT) == ["i", "a", "e", "t", "z"], list(WANT)
assert WANT["i"][1] is None and all(WANT[n][1] is not None for n in "aetz")


def check_ndarray(where: str, name: str, got) -> None:
    """``got`` is the Rust reader's column: values bit for bit, NULL exactly where it says."""
    values, valid = WANT[name]
    if valid is None:
        assert type(got) is np.ndarray, (
            f"{where} {name}: {type(got)} (not nullable: plain ndarray)"
        )
        data = got
    else:
        assert isinstance(got, np.ma.MaskedArray), (
            f"{where} {name}: {type(got)}, want MaskedArray"
        )
        mask = np.ma.getmaskarray(got)
        assert np.array_equal(mask, ~valid), f"{where} {name}: mask {mask} != {~valid}"
        data = got.data
    assert data.dtype == values.dtype, (
        f"{where} {name}: dtype {data.dtype} != {values.dtype}"
    )
    assert len(data) == ROWS, f"{where} {name}: {len(data)} rows"
    assert data.tobytes() == values.tobytes(), f"{where} {name}: {data!r} != {values!r}"


for name in WANT:
    check_ndarray("column", name, r.column("obs", name))
    check_ndarray("logical_column", name, r.logical_column("obs", name))
d = r.table_dict("obs")
assert list(d) == list(WANT), list(d)
for name, got in d.items():
    check_ndarray("table_dict", name, got)

# A column declared nullable is a MaskedArray even when it holds no NULL: the type follows the
# schema, not the data, so a caller's code does not change with the rows a file happens to hold.
t = r.column("obs", "t")
assert isinstance(t, np.ma.MaskedArray) and not np.ma.getmaskarray(t).any()
# A present NaN is not a NULL.
e = r.column("obs", "e")
assert np.isnan(e.data[2]) and not np.ma.getmaskarray(e)[2]
# Leaf 0 of `z` is all NULL, leaf 1 all present.
assert np.ma.getmaskarray(r.column("obs", "z")).tolist() == [True] * 5 + [False] * 6

# pyarrow: a real validity bitmap, the null count right, the values under it the Rust reader's.
at = r.table_arrow("obs")
assert isinstance(at, pa.Table) and at.num_rows == ROWS
assert at.column_names == list(WANT)
for name, (values, valid) in WANT.items():
    arr = at.column(name).combine_chunks()
    nulls = 0 if valid is None else int((~valid).sum())
    assert arr.null_count == nulls, (
        f"table_arrow {name}: null_count {arr.null_count} != {nulls}"
    )
    assert arr.is_null().to_pylist() == (
        [False] * ROWS if valid is None else (~valid).tolist()
    )
    assert arr.type == pa.from_numpy_dtype(values.dtype), (
        f"table_arrow {name}: {arr.type}"
    )
    present = [bool(v) for v in (np.ones(ROWS, bool) if valid is None else valid)]
    got = (
        arr.to_numpy(zero_copy_only=False)
        if valid is None
        else arr.fill_null(0).to_numpy()
    )
    assert np.array_equal(
        got[present], values[present], equal_nan=values.dtype.kind == "f"
    )
assert math.isnan(at.column("e")[2].as_py())  # a present NaN, not a NULL

# polars: native nulls, NaN stays a value.
df = r.table("obs")
assert isinstance(df, pl.DataFrame) and df.height == ROWS
for name, (values, valid) in WANT.items():
    s = df[name]
    want_nulls = [False] * ROWS if valid is None else (~valid).tolist()
    assert s.is_null().to_list() == want_nulls, (
        f"table {name}: nulls {s.is_null().to_list()}"
    )
    assert s.null_count() == sum(want_nulls)
assert df["e"].is_nan().to_list()[2] is True
assert df["a"].to_list() == [None if not v else int(x) for x, v in zip(*WANT["a"])]

# The book's example (docs/book/src/bindings.md includes this region).
path = sys.argv[1]
# ANCHOR: nullable
r = tessera.open(path)
a = r.column("obs", "a")  # np.ma.MaskedArray: `a` is declared nullable
assert a.mask.tolist() == [
    False,
    True,
    False,
    False,
    True,
    False,
    False,
    True,
    False,
    False,
    True,
]
assert a.compressed().tolist() == [-7, -1, 2, 8, 11, 17, 20]  # the present values
assert type(r.column("obs", "i")) is np.ndarray  # `i` is not nullable: a plain ndarray
assert r.table_arrow("obs").column("a").null_count == 4  # an Arrow validity bitmap
assert r.table("obs")["a"].null_count() == 4  # polars nulls
e = r.column("obs", "e")
assert np.isnan(e[2]) and not e.mask[2]  # a present NaN is a value, not a NULL
# ANCHOR_END: nullable

# The native layer hands values and validity over SEPARATELY: one plain buffer of exactly `rows`
# elements, and a mask (`None` when the column is not nullable). Nothing to split off by arithmetic.
for name, buf, code, validity in r._r.read_table("obs"):
    values, valid = WANT[name]
    assert code == values.dtype.str[1:] and len(buf) == values.nbytes, (
        name,
        code,
        len(buf),
    )
    assert (validity is None) == (valid is None), (name, validity)
    if validity is not None:
        assert np.frombuffer(validity, np.bool_).tolist() == valid.tolist(), name
for name, (values, valid) in WANT.items():
    buf, code, validity = r._r.read_table_column("obs", name)
    assert buf == values.tobytes() and (validity is None) == (valid is None), name
    buf, code, validity = r._r.read_logical_table_column("obs", name)
    assert buf == values.tobytes() and (validity is None) == (valid is None), name

# One leaf reads by its full name, its mask that leaf's rows.
assert (
    np.ma.getmaskarray(r.column("obs/0001", "a")).tolist()
    == (~WANT["a"][1][5:]).tolist()
)

print("nullable_columns: ok")
