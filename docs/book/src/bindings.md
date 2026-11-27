# Bindings: Python & WASM

Both bindings wrap `tessera-core` directly — no reimplementation, so they inherit the same identity,
seal, and verification logic.

> **Evidence form:** these are the "executed doctest / referenced check" chapters — the examples run under
> the bindings' own test gates (`cargo test` doctests + the hermetic Python/WASM flake checks), not as a
> CLI `trycmd`. See the [Introduction](./intro.md).

## Python (`pyo3`, abi3)

`import tessera` gives `tessera.open()` → a `Reader` (manifest / blocks / `array` → NumPy / `array_roi`
for an ROI / `table` → polars / `table_arrow` / `table_dict` / `column` / `verify`) and a `Builder`
(`add_array` / `add_table` / `set_field` / `add_source`, whose `content_hash=` pins the parent's seal /
`with_producer` / `with_generation` / `pack`), with a typed `TesseraError`.

When using HDF5-backed reads (including through other Python HDF5 libraries), close all HDF5
handles before forking or starting a subprocess. Vendored libhdf5 opens file descriptors without
`O_CLOEXEC`: a child can inherit an open file and keep its exclusive lock after the parent closes it,
until the child closes the descriptor or exits. This is a general libhdf5 property, not a Tessera
reader guarantee; an inherited lock can make another HDF5 open fail with “unable to lock file”.

Tables are read by their **logical name** (ADR-0064). `table`, `table_arrow`, `table_dict` and `column` take a
name that is one block, a partitioned table (`pairs` over the leaves `pairs/0000`, `pairs/0001`, …, joined in
leaf order) or a legacy `events_NNNN` group, the same name `tessera read` takes; one leaf still reads by its full
name. `logical_column` is the same read as `column`, kept for existing callers. A partitioned table is
materialised whole by `table*`; `column` reads only that column's segments of each leaf. `read_block`, `array`
and `array_roi` take one block, and a name that is not one (a node such as `pairs`, or a name nothing answers
to) raises `NotABlockError`, a subclass of `TesseraError`. A name that is not a table, or a partitioned table
that is invalid (a gap, a foreign child, leaves out of order, ...), raises `TesseraError` naming the reason.
Schema mismatches also expose `kind`, `column`, `field`, `expected` and `found` attributes:

| `kind` | `column` | `field` | `expected` / `found` |
| --- | --- | --- | --- |
| `column_count` | `None` | `None` | Integer column counts |
| `column_name` | Zero-based index | `None` | Column name strings |
| `field` | Column name | `dtype`, `nullable`, `unit`, `scale` or `referencing` | Display strings |
| `unknown` (future mismatch kinds) | `None` | `None` | `None` |

Field values use quoted strings, plain numbers/booleans, `absent` for missing optional values and JSON
for complete referencing descriptors. These attributes apply to schema mismatches during reads and
`Builder.pack`; other errors retain their message-only interface.

```python
{{#include ../../../tessera/crates/tessera-py/tests/write_example.py:logical}}
```

A **nullable** column (one whose schema declares `nullable`) reads as an `np.ma.MaskedArray`, masked (`mask=True`)
at each NULL, from `column`, `logical_column` and `table_dict`. The type follows the schema, not the rows: a
nullable column that holds no NULL is still a masked array (with nothing masked), and a column that is not
nullable is always a plain `np.ndarray`. `table_arrow` gives a nullable column a real Arrow validity bitmap
(`null_count` counts the NULLs), and `table` hands polars native nulls. A NULL is not a NaN: a present NaN stays a
value in every form. Under a NULL the masked array's `.data` holds the dtype default (`0`), never what the
producer had there (the format does not keep it). Nullability covers the fixed-width numeric dtypes; a sealed
table cannot hold a nullable `str` or `b1` column. The native layer passes the values and the validity separately,
`(le_bytes, numpy_code, validity)` with `validity` `None` for a column that is not nullable, so nothing has to
split a mask off the values. The Python `Builder` cannot write a nullable column yet
([#950](https://github.com/vig-os/tessera/issues/950)).

```python
{{#include ../../../tessera/crates/tessera-py/tests/nullable_columns.py:nullable}}
```

*This snippet is `{{#include}}`d from `tessera/crates/tessera-py/tests/nullable_columns.py`, which the
`tessera-py-import` and `tessera-wheel-import` checks run on a committed fixture, holding every read above to the
Rust reader's values and masks.*

Reading:

```python
{{#include ../../../tessera/crates/tessera-py/tests/write_example.py:read}}
```

Writing — the half that used to be advertised but never shown:

```python
{{#include ../../../tessera/crates/tessera-py/tests/write_example.py:array}}
```

Provenance — who generated the product (`with_producer`), how (`with_generation`), and which sealed parent
it derives from (`add_source(..., content_hash=)`). See [Provenance, signing & trust](./provenance-signing.md)
for what each record means and how `tessera provenance` walks the chain:

```python
{{#include ../../../tessera/crates/tessera-py/tests/write_example.py:provenance}}
```

`with_generation` takes a `config` dict whose keys are `str` at every level and whose values are JSON
(`str`, 64-bit `int`, finite `float`, `bool`, `None`, `list`/`tuple`, `dict`). Anything else raises
`TypeError` naming the key, rather than being coerced, and that includes a `str` with a lone surrogate. As with
`json.dumps`, a container that contains itself raises `ValueError`. A config may nest at most 125 levels (dicts
and lists alike, counting `config` itself), the most the manifest reader parses back; deeper raises
`TesseraError` instead of sealing a product that can't be opened. Read back through `manifest()`, every level of
the config is key-sorted, the canonical order the seal hashes. Its `config_ref=` digest must name a config block
carried in the same file, which the Python `Builder` cannot add yet
([#575](https://github.com/vig-os/tessera/issues/575)), so from Python use `config`. A product's schema
can require a recipe (`requires_generation`, ADR-0058 §3), but no built-in schema does today, and the Python
`Builder` cannot embed a custom one.

The [ingest cookbook](./ingest-cookbook.md#2-writing-from-python) has the table and string-column cases,
and the dtype asymmetries worth knowing.

*Every snippet above is `{{#include}}`d from `tessera/crates/tessera-py/tests/write_example.py`, which the
`tessera-py-import` and `tessera-wheel-import` flake checks execute on every build — so a call that stopped
working could not reach this page.* (The previous version of this section documented
`tessera.Reader("study.tsra")` and `read_array`/`read_array_subset`/`read_table`/`read_table_column`, none
of which exist — exactly the drift that including from a tested file prevents.)

## WASM (`wasm-bindgen`)

The pure-Rust spine compiles to `wasm32` and **runs in Node/the browser** with zero C dependencies:
manifest seal, MMR `content_hash`, inclusion/consistency proofs, ed25519 **verify**, and referencing.

```js
import init, { verify_manifest, seal_verdict, verify_signature, product_id } from "./tessera_wasm.js";
await init();
// The two stored members of a .tsra (ADR-0066), manifest.json and manifest.seal, as their bytes:
// Uint8Arrays read straight from the container, never strings decoded from them.
verify_manifest(manifestJson, manifestSeal); // canonical check + seal + id + content_hash
seal_verdict(manifestJson, manifestSeal);    // "ok" | "tampered" | "refused", as corpus/seal-cases.json
```

The seal is over the stored bytes, so the bindings take the bytes and parse them as strict UTF-8 in
Rust. Decoding them first would verify a different document: a default `TextDecoder` drops a leading
byte-order mark and maps invalid UTF-8 to U+FFFD, both of which the native reader refuses.
A product without `manifest.seal` (pass `undefined`) is the pre-ADR-0066 alpha layout and is refused.

*Referenced checks:* `wasm-core` (the core compiles to wasm32) + `wasm-bindgen-smoke` (bindings build →
bindgen → **execute in Node**, reaching every verdict of `corpus/seal-cases.json`). Columnar payload reads at the RS↔TS boundary go through **Arrow-JS** — the
WASM core verifies + locates the bytes, Arrow-JS reads them — so the heavier Vortex/zarrs stacks aren't
needed in-browser.
