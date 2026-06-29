"""Hermetic smoke test for the `tessera` Python PACKAGE (#210 + the ergonomic layer).

Run as: python3 smoke.py <corpus_files_dir>

Exercises the ERGONOMIC surface over the committed conformance corpus — numpy ``ndarray`` for arrays
(+ ROI sub-cube), polars ``DataFrame`` / pyarrow ``Table`` / dict-of-arrays + projected column for
tables — plus the typed-error path and a numpy write→read roundtrip. Driven by the `tessera-py-import`
flake check (the assembled `tessera/` package — pure-Python `__init__.py` + `_native.so` — on PYTHONPATH).
"""

import pathlib
import sys
import tempfile

import numpy as np
import polars as pl
import pyarrow as pa

import tessera

corpus = pathlib.Path(sys.argv[1])
files = sorted(corpus.glob("*.tsra"))
assert files, f"no .tsra fixtures under {corpus}"

assert isinstance(tessera.__version__, str)
assert issubclass(tessera.TesseraError, Exception)

verified = 0
for f in files:
    r = tessera.open(f)
    m = r.manifest()  # a dict, parsed from the canonical JSON
    for key in ("id", "product", "name", "blocks"):
        assert key in m, f"{f.name}: manifest missing {key!r}"
    assert r.product == m["product"]
    assert r.id == m["id"]
    assert r.block_names() == [b["name"] for b in m["blocks"]]
    r.verify()
    tessera.verify(f)

    for b in m["blocks"]:
        if b["kind"] == "array":
            arr = r.array(b["name"])  # -> ndarray, already reshaped (native dtype, C-order)
            assert isinstance(arr, np.ndarray)
            assert list(arr.shape) == [int(d) for d in b["spec"]["shape"]]
            # ROI sub-cube equals the full array's corresponding slice (only intersecting chunks read)
            if all(d >= 2 for d in arr.shape):
                half = [max(1, d // 2) for d in arr.shape]
                sub = r.array_roi(b["name"], [0] * arr.ndim, half)
                assert np.array_equal(sub, arr[tuple(slice(0, h) for h in half)]), f"{f.name}: ROI"
        elif b["kind"] == "table":
            df = r.table(b["name"])  # -> polars DataFrame (via Arrow)
            assert isinstance(df, pl.DataFrame)
            at = r.table_arrow(b["name"])  # -> pyarrow Table
            assert isinstance(at, pa.Table)
            assert df.height == at.num_rows
            cols = r.table_dict(b["name"])  # -> {column: ndarray}
            first = next(iter(cols))
            proj = r.column(b["name"], first)  # projected single column
            assert isinstance(proj, np.ndarray)
            assert np.array_equal(proj, cols[first]), f"{f.name}: projection != full read"
            assert df.height == proj.shape[0]
    verified += 1

# typed-error path: a missing file raises TesseraError, not a bare exception
try:
    tessera.open(corpus / "does_not_exist.tsra")
    raise AssertionError("expected TesseraError for missing file")
except tessera.TesseraError:
    pass

# write path: build from numpy, pack, reopen through the wrapper, assert the ergonomic reads match
vol = np.arange(2 * 3 * 4, dtype="<i2").reshape(2, 3, 4)
idx = np.arange(5, dtype="<u4")
en = np.array([0.5, 1.5, 2.5, 3.5, 4.5], dtype="<f4")

with tempfile.TemporaryDirectory() as td:
    out = pathlib.Path(td) / "roundtrip.tsra"
    b = tessera.Builder("recon", "rt", "py write roundtrip", "2024-01-01T00:00:00Z")
    b.add_array("volume", "i2", list(vol.shape), vol.tobytes())
    b.add_table("events", [("idx", "u4", idx.tobytes()), ("en", "f4", en.tobytes())], "idx")
    b.set_field("modality", '{"_vocabulary": "DICOM", "_code": "PT"}')
    cid = b.pack(str(out))
    assert cid.startswith("blake3:")

    rr = tessera.open(out)
    rr.verify()
    assert np.array_equal(rr.array("volume"), vol), "array write→read mismatch"
    cols = rr.table_dict("events")
    assert np.array_equal(cols["idx"], idx) and np.array_equal(cols["en"], en)
    # polars + pyarrow views agree with the numpy view
    assert rr.table("events")["idx"].to_list() == idx.tolist()
    assert rr.table_arrow("events").column("en").to_pylist() == en.tolist()

print(
    f"tessera-py smoke OK: {verified} corpus archives via the ergonomic surface "
    "(numpy / polars / pyarrow) + numpy write→read roundtrip"
)
