"""The **write** path, as a runnable example the book includes verbatim (#389).

Run as: python3 write_example.py

The audit's complaint was that the Python bindings advertise "read/verify/**write**" while no
documentation ever showed the write half. So this file is the documentation: `docs/book/src/bindings.md`
and the ingest cookbook `{{#include}}` the anchored regions below, and the `tessera-py-import` flake check
executes it — a snippet that stopped working could not reach the book.

Everything is asserted, not just printed: a doc example that runs but produces the wrong file is no better
than a stale snippet.
"""

import pathlib
import tempfile

import numpy as np

import tessera

out_dir = pathlib.Path(tempfile.mkdtemp())

# ---------------------------------------------------------------------------
# ANCHOR: array
# Write a 3-D volume: a `recon` product with one array block.
#
# `add_array` takes the numpy dtype CODE, the shape, and little-endian C-order
# bytes. The `newbyteorder("<")` is what makes the write explicit rather than
# host-dependent — a big-endian machine would otherwise seal different bytes.
volume = (np.arange(4 * 8 * 8, dtype="<i2") % 2048).reshape(4, 8, 8)

b = tessera.Builder(
    "recon",  # product kind — see the product-schema reference
    "demo-ct-01",  # your name for this product
    "a synthetic CT volume",
    "2024-01-01T00:00:00Z",  # ISO-8601; sealed, so keep it deterministic
)
b.add_array("volume", "i2", list(volume.shape), volume.tobytes())
# `recon` marks `modality` required, so set it (absent = a hard validation block).
b.set_field("modality", '{"_vocabulary": "DICOM", "_code": "CT"}')
b.add_source("ingested_from", "synthetic://demo")
content_id = b.pack(str(out_dir / "demo.tsra"))
# ANCHOR_END: array
# ---------------------------------------------------------------------------

assert isinstance(content_id, str) and content_id, "pack() returns the content id"

# ---------------------------------------------------------------------------
# ANCHOR: roundtrip
# Read it back: `verify` first (integrity), then the array as a numpy array.
r = tessera.open(out_dir / "demo.tsra")
r.verify()  # raises TesseraError if a hash disagrees
back = r.array("volume")  # -> np.ndarray, original dtype and shape
assert back.dtype == np.dtype("int16")
assert back.shape == (4, 8, 8)
# ANCHOR_END: roundtrip
# ---------------------------------------------------------------------------

assert np.array_equal(back, volume), "the write→read roundtrip must be exact"
assert r.product == "recon"

# ---------------------------------------------------------------------------
# ANCHOR: table
# Write a table: ordered columns of `(name, dtype_code, little-endian bytes)`.
# All columns must be the same length. `row_index` names the column to build the
# O(1)-take index over — pass the monotonic one if you have it.
ms = np.arange(1000, dtype="<u4")
energy = (350 + (ms % 160)).astype("<u2")

t = tessera.Builder(
    "listmode", "demo-lm-01", "synthetic events", "2024-01-01T00:00:00Z"
)
t.add_table(
    "events",
    [("ms", "u4", ms.tobytes()), ("energy_keV", "u2", energy.tobytes())],
    row_index="ms",
)
t.pack(str(out_dir / "events.tsra"))
# ANCHOR_END: table
# ---------------------------------------------------------------------------

ev = tessera.open(out_dir / "events.tsra")
ev.verify()
df = ev.table("events")  # polars DataFrame
assert df.shape == (1000, 2), df.shape
assert df["ms"].to_numpy().tolist() == ms.tolist()
assert df["energy_keV"].to_numpy().tolist() == energy.tolist()

# A string column is table-only (float16 is the mirror case: array-only).
s = tessera.Builder("blob", "demo-labels", "a string column", "2024-01-01T00:00:00Z")
labels = [b"lesion", b"liver", b"aorta"]
packed = b"".join(len(x).to_bytes(4, "little") + x for x in labels)
s.add_table("labels", [("name", "str", packed)])
s.pack(str(out_dir / "labels.tsra"))
# `table_dict` hands back numpy arrays, so compare as a list rather than with `==` (which would
# produce an array and raise "truth value … is ambiguous").
got = tessera.open(out_dir / "labels.tsra").table_dict("labels")["name"]
assert list(got) == ["lesion", "liver", "aorta"], list(got)

# The typed error, so a reader knows what a failure looks like.
try:
    tessera.open(out_dir / "does-not-exist.tsra")
except tessera.TesseraError:
    pass
else:  # pragma: no cover
    raise AssertionError("opening a missing file must raise TesseraError")

# ---------------------------------------------------------------------------
# ANCHOR: read
# The read surface, over a product you already have.
r = tessera.open(out_dir / "demo.tsra")
r.verify()  # raises TesseraError on tamper
r.product  # "recon"
r.block_names()  # ["volume"]
r.manifest()  # the full manifest as a dict

vol = r.array("volume")  # -> np.ndarray, native dtype and shape
roi = r.array_roi(
    "volume", origin=[0, 0, 0], shape=[2, 4, 4]
)  # only the chunks it needs

events = tessera.open(out_dir / "events.tsra")
events.table("events")  # -> polars DataFrame
events.table_arrow("events")  # -> pyarrow Table
events.table_dict("events")  # -> {column: np.ndarray}
events.column("events", "ms")  # one column, as np.ndarray
# ANCHOR_END: read
# ---------------------------------------------------------------------------

assert vol.shape == (4, 8, 8) and roi.shape == (2, 4, 4)
assert np.array_equal(roi, volume[0:2, 0:4, 0:4]), (
    "the ROI must match the same slice of the whole"
)

print("write_example: ok")
