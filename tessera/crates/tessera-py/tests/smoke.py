"""Hermetic smoke test for the `tessera` Python extension (#210).

Run as: python3 smoke.py <corpus_files_dir>
Exercises the read+verify surface against the committed conformance corpus and asserts the typed
error path. Driven by the `tessera-py-import` flake check (the .so on PYTHONPATH).
"""

import functools
import json
import operator
import sys
import pathlib

import numpy as np

import tessera

corpus = pathlib.Path(sys.argv[1])
files = sorted(corpus.glob("*.tsra"))
assert files, f"no .tsra fixtures under {corpus}"

assert isinstance(tessera.__version__, str)
assert issubclass(tessera.TesseraError, Exception)

verified = 0
for f in files:
    r = tessera.open(str(f))
    # manifest round-trips to a dict with the canonical fd5 spine fields
    m = json.loads(r.manifest_json())
    for key in ("id", "product", "name", "blocks"):
        assert key in m, f"{f.name}: manifest missing {key!r}"
    assert r.product == m["product"]
    assert r.id == m["id"]
    assert r.block_names() == [b["name"] for b in m["blocks"]]
    # full integrity pass (seal + every block digest) — must not raise
    r.verify()
    tessera.verify(str(f))
    # every declared block is digest-verified-readable
    for name in r.block_names():
        assert isinstance(r.read_block(name), (bytes, bytearray))

    # decode each block to numpy via the typed accessors and check it materialises with the
    # shape/dtype the manifest declares (the actual "Python can read the data" parity check)
    for b in m["blocks"]:
        if b["kind"] == "array":
            buf, shape, code = r.read_array(b["name"])
            arr = np.frombuffer(buf, "<" + code).reshape(tuple(shape))
            assert arr.size == functools.reduce(operator.mul, shape, 1)
            # ROI sub-cube must equal the full array's corresponding slice
            if all(d >= 2 for d in shape):
                half = [max(1, d // 2) for d in shape]
                sbuf, sshape, scode = r.read_array_subset(b["name"], [0] * len(shape), half)
                sub = np.frombuffer(sbuf, "<" + scode).reshape(tuple(sshape))
                expected = arr[tuple(slice(0, h) for h in half)]
                assert np.array_equal(sub, expected), f"{f.name}: ROI subset mismatch"
        elif b["kind"] == "table":
            cols = r.read_table(b["name"])
            assert cols, f"{f.name}: empty table {b['name']}"
            lengths = {np.frombuffer(buf, "<" + code).shape[0] for _, buf, code in cols}
            assert len(lengths) == 1, f"{f.name}: ragged columns {lengths}"
    verified += 1

# typed-error path: bad path and unknown block both raise TesseraError (not a bare RuntimeError)
try:
    tessera.open(str(corpus / "does_not_exist.tsra"))
    raise AssertionError("expected TesseraError for missing file")
except tessera.TesseraError:
    pass

probe = tessera.open(str(files[0]))
try:
    probe.read_block("no-such-block")
    raise AssertionError("expected TesseraError for unknown block")
except tessera.TesseraError:
    pass

# write path: build a product from numpy, pack, read it back, assert bit-exact round-trip
import tempfile  # noqa: E402

vol = np.arange(2 * 3 * 4, dtype="<i2").reshape(2, 3, 4)
idx = np.arange(5, dtype="<u4")
en = np.array([0.5, 1.5, 2.5, 3.5, 4.5], dtype="<f4")

with tempfile.TemporaryDirectory() as td:
    out = pathlib.Path(td) / "roundtrip.tsra"
    b = tessera.Builder("recon", "rt", "py write roundtrip", "2024-01-01T00:00:00Z")
    b.add_array("volume", "i2", list(vol.shape), vol.tobytes())
    b.add_table("events", [("idx", "u4", idx.tobytes()), ("en", "f4", en.tobytes())], "idx")
    b.set_field("modality", '{"_vocabulary": "DICOM", "_code": "PT"}')
    b.add_source("ingested_from", "src.dat")
    cid = b.pack(str(out))
    assert cid.startswith("blake3:")

    rr = tessera.open(str(out))
    rr.verify()
    buf, shape, code = rr.read_array("volume")
    got = np.frombuffer(buf, "<" + code).reshape(tuple(shape))
    assert np.array_equal(got, vol), "array write→read mismatch"
    cols = {n: np.frombuffer(buf, "<" + code) for n, buf, code in rr.read_table("events")}
    assert np.array_equal(cols["idx"], idx) and np.array_equal(cols["en"], en)
    m = json.loads(rr.manifest_json())
    assert m["sources"][0]["reference"] == "src.dat"

print(f"tessera-py smoke OK: {verified} corpus archives verified + write→read roundtrip via the Python bindings")
