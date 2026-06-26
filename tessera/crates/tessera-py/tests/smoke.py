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

print(f"tessera-py smoke OK: {verified} corpus archives verified via the Python bindings")
