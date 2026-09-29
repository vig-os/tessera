# Ingest cookbook: writing your own data

The other chapters show you how to *read* a `.tsra`. This one is the other direction: you have data, and
you want a sealed, self-describing, verifiable product out of it.

Pick the row that matches what you have.

| you have | use | why |
| --- | --- | --- |
| a vendor file nothing parses yet (`.l64`, `.7z`, a console dump, a PDF) | [`ingest blob`](#1-preserve-anything-bit-faithfully) | bit-faithful preservation now, decode later, nothing lost |
| DICOM — one multi-frame file, or a slice series | `ingest dicom` / `ingest dicom-series` | normalised at the door: native dtype, LPS geometry, PS3.15-aware |
| a NIfTI volume (`.nii`, `.nii.gz`, either byte order) | `ingest nifti` | sform→qform geometry, mm-normalised, every volume of a 4-D series |
| GE/vendor HDF5 listmode | `ingest hdf-compound` | descriptor-driven compound→columnar, streams above 256 MiB |
| a headerless binary you know the shape of | `ingest raw` | you supply `--shape`/`--dtype`; element-count guarded |
| several of the above, as one study | [`ingest --spec`](#2-a-declarative-spec) | one reproducible file → a sealed collection |
| numpy arrays / columns already in memory | [the Python `Builder`](#2-writing-from-python) | no intermediate file at all |

Two things worth knowing before you start, because they are the mistakes that cost the most time:

- **`--name` and `--timestamp` are part of the product's identity.** Re-running an ingest with the same
  inputs and the same name/timestamp produces a byte-identical `.tsra`. Change the timestamp casually and
  you have made a different product.
- **Check the [product-schema reference](./schemas.md) for your product kind.** A *required* field that is
  absent is a hard failure; a *recommended* one is a `WARN` you will see on stderr and should not ignore.

## 1. Two recipes, end to end

The transcript below is executed against the real binary on every build — the blob route, then the
declarative spec, then verifying what came out. The minimal spec it runs is this, in full:

```toml
{{#include ../../../tessera/crates/tessera-cli/tests/cmd/cookbook.in/minimal-spec.toml}}
```

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/cookbook.trycmd}}

Three things in there are worth pulling out, because they surprise people:

- **Members are written under their content id**, not their human name. Identity is the hash; the filename
  is an artifact. `tessera collection ls` is how you go from a name back to a file.
- **`tessera verify` is for a `.tsra`; a collection has its own verb** — `tessera collection verify`, which
  checks the catalog's seal *and* that every member is present, intact, and the pinned version.
- **`[collection] study` names the collection, not its members.** Per-product schema metadata goes in
  `[product.metadata]`, which is why the spec above sets `study` twice.

A fuller, heavily-commented template for a real PET/CT study — vendor-raw blob plus normalised CT and PET
reconstructions, with `derived_from` edges between them — ships at
[`docs/examples/migrate-petct-study.toml`](https://github.com/vig-os/tessera/blob/dev/tessera/docs/examples/migrate-petct-study.toml).

## 2. Writing from Python

The bindings advertise read/verify/**write**, and this is the write half. Every line below is executed by
the `tessera-py-import` check on every build, so it cannot go stale.

An array product:

```python
{{#include ../../../tessera/crates/tessera-py/tests/write_example.py:array}}
```

Reading it straight back — verify first, then the array as a numpy array:

```python
{{#include ../../../tessera/crates/tessera-py/tests/write_example.py:roundtrip}}
```

A table product, including the `row_index` that buys O(1) row lookup:

```python
{{#include ../../../tessera/crates/tessera-py/tests/write_example.py:table}}
```

Two asymmetries that are deliberate rather than oversights: **`str` columns are table-only**, and
**`float16` is array-only** — the columnar toolchain (polars/arrow/duckdb) has no native half-float, so a
float16 *column* would silently upcast somewhere downstream.

## 3. Getting it back out

Once sealed, the read side is the rest of this book — but the two you will want immediately:

```console
tessera verify study.tsra      # the seal and every block digest
tessera schema study.tsra      # required fields + block roles, against its own embedded contract
tessera ls     study.tsra      # what is actually in there
```

Array data comes out as CSV, TSV, JSON, NumPy `.npy` or a PNG preview — see
[Arrays](./arrays.md#output-formats). Tables come out as CSV/TSV/NDJSON, or via SQL — see
[Tables & SQL](./tables-sql.md).

## Not yet: generic `table` / `array` ingest

The verbs above are all *vendor* decoders. A generic "here is a Parquet file / a `.npy` / an Arrow table,
make it a product" path is designed (ADR-0056) and in progress, but **is not on `dev` yet**, so it is
deliberately not documented here — a cookbook that lists flags you cannot run is worse than one that
admits the gap. Track it at
[#386](https://github.com/vig-os/tessera/issues/386). Until then, the Python `Builder` in §2 is the
supported way to turn in-memory arrays and columns into a product without an intermediate vendor file.
