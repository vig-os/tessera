"""Python table reads on logical table names (ADR-0064 PR C2, #717).

Run as: python3 logical_tables.py <partitioned_pairs.tsra>

The fixture is a partitioned table ``pairs`` over the leaves ``pairs/0000`` (3 rows), ``pairs/0001`` (3)
and ``pairs/0002`` (2); column ``i`` is the global row index 0..8 and ``x`` is ``i / 4``. It is built by
``tests/partitioned_fixture.rs`` (which keeps the committed bytes equal to its recipe), and its
``content_hash`` is pinned here so a swapped file can't pass. Every table read must return all eight rows
in leaf order, and ``read_block("pairs")`` must raise ``NotABlockError``.
"""

import sys

import numpy as np
import polars as pl
import pyarrow as pa

import tessera

CONTENT_HASH = "blake3:b07c54420a0b9010c35a45cbdf22550dcfa316114e5ca8da2b18be85ac47f8d6"
LEAVES = ["pairs/0000", "pairs/0001", "pairs/0002"]
WANT_I = np.arange(8, dtype="<u8")
WANT_X = WANT_I / 4.0

r = tessera.open(sys.argv[1])
r.verify()
m = r.manifest()
assert m["content_hash"] == CONTENT_HASH, m["content_hash"]
assert r.block_names() == LEAVES, r.block_names()
assert [b["spec"]["partition"] for b in m["blocks"]] == [
    {"index": 0, "last": False},
    {"index": 1, "last": False},
    {"index": 2, "last": True},
]

# Every table read resolves the logical name: all rows, in leaf order.
assert np.array_equal(r.column("pairs", "i"), WANT_I)
assert np.array_equal(r.column("pairs", "x"), WANT_X)
assert np.array_equal(r.logical_column("pairs", "i"), WANT_I)
d = r.table_dict("pairs")
assert list(d) == ["i", "x"]
assert np.array_equal(d["i"], WANT_I) and np.array_equal(d["x"], WANT_X)
at = r.table_arrow("pairs")
assert isinstance(at, pa.Table) and at.num_rows == 8
assert at.column("i").to_pylist() == WANT_I.tolist()
df = r.table("pairs")
assert isinstance(df, pl.DataFrame)
assert df["i"].to_list() == WANT_I.tolist() and df["x"].to_list() == WANT_X.tolist()

# The native surface the wrappers sit on resolves the same way.
cols = r._r.read_table("pairs")
assert [n for n, _, _ in cols] == ["i", "x"]
assert np.array_equal(np.frombuffer(cols[0][1], "<u8"), WANT_I)
buf, code = r._r.read_table_column("pairs", "x")
assert code == "f8" and np.array_equal(np.frombuffer(buf, "<f8"), WANT_X)

# A leaf is still one table, read by its full name.
assert np.array_equal(r.column("pairs/0001", "i"), WANT_I[3:6])
assert r.table("pairs/0002").height == 2

# `read_block` takes one block: the node is the typed "not a block" error, a leaf reads.
assert issubclass(tessera.NotABlockError, tessera.TesseraError)
for what, read in (
    ("read_block", r.read_block),
    ("array", r.array),
    ("array_roi", lambda n: r.array_roi(n, [0], [1])),
):
    try:
        read("pairs")
        raise AssertionError(f"{what}('pairs') must raise NotABlockError")
    except tessera.NotABlockError as e:
        assert "'pairs' is not a block" in str(e) and "3 block(s)" in str(e), e
try:
    r.read_block("nope")
    raise AssertionError("read_block('nope') must raise NotABlockError")
except tessera.NotABlockError as e:
    assert "no block named 'nope'" in str(e), e
assert len(r.read_block("pairs/0000")) > 0

# A name that is not a table stays a TesseraError, never an empty or partial table.
try:
    r.table("nope")
    raise AssertionError("table('nope') must raise TesseraError")
except tessera.TesseraError as e:
    assert "logical table 'nope'" in str(e), e

print("logical_tables: ok")
