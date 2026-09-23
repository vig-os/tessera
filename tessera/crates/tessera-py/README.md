# tessera-py — Python bindings for Tessera

`import tessera` — read, verify, decode, and write sealed `.tsra` FAIR data products from Python.
Built as an **abi3** extension module (pyo3): a single `tessera.so` works on any CPython ≥ 3.9, with
no libpython linkage.

## Read + verify + decode

```python
import tessera, numpy as np

r = tessera.open("study.tsra")          # opens + verifies the manifest seal
print(r.product, r.id)                   # "recon", "blake3:…"
r.verify()                               # full pass: seal + every block digest

# decode an array block straight to numpy (little-endian, C-order)
buf, shape, code = r.read_array("volume")
vol = np.frombuffer(buf, "<" + code).reshape(shape)

# or a 3-D ROI sub-cube — only the intersecting 64³ chunks are read (7–26× faster)
buf, shape, code = r.read_array_subset("volume", origin=[0, 0, 0], shape=[64, 64, 64])
roi = np.frombuffer(buf, "<" + code).reshape(shape)

# decode a table block, column by column
cols = {name: np.frombuffer(buf, "<" + code)
        for name, buf, code in r.read_table("events")}
```

Anything wrong — bad magic, broken seal, digest mismatch, unknown block — raises `tessera.TesseraError`.

## Write

```python
import tessera, numpy as np

vol = np.arange(64**3, dtype="<i2").reshape(64, 64, 64)
b = tessera.Builder("recon", "DP06-ct", "demo volume", "2024-01-01T00:00:00Z")
b.add_array("volume", "i2", list(vol.shape), vol.tobytes())
b.add_table("events",
            [("idx", "u4", np.arange(5, dtype="<u4").tobytes()),
             ("en",  "f4", np.array([511.0]*5, dtype="<f4").tobytes())],
            row_index="idx")
b.set_field("modality", '{"_vocabulary": "DICOM", "_code": "PT"}')
b.add_source("ingested_from", "raw.dcm")
content_id = b.pack("out.tsra")          # seals (3 hashes) + writes; returns the content id
```

Dtype codes are the fd5 numpy codes without a byte-order prefix: `i2/i4/i8`, `u2/u4/u8`, `f4/f8`
(tables also allow `i1/u1`). Buffers are little-endian, C-order — pass
`arr.astype(arr.dtype.newbyteorder("<")).tobytes()` if your array isn't already `<`.

## Building

The crate is part of the Tessera Nix workspace; `nix flake check` builds the `.so` and runs the
`tessera-py-import` check (a numpy round-trip over the conformance corpus). For a local wheel-less
build: `cargo build -p tessera-py --release` then `cp target/release/libtessera.so tessera.so`.
