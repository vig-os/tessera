# Bindings: Python & WASM

Both bindings wrap `tessera-core` directly — no reimplementation, so they inherit the same identity,
seal, and verification logic.

> **Evidence form:** these are the "executed doctest / referenced check" chapters — the examples run under
> the bindings' own test gates (`cargo test` doctests + the hermetic Python/WASM flake checks), not as a
> CLI `trycmd`. See the [Introduction](./intro.md).

## Python (`pyo3`, abi3)

`import tessera` gives `tessera.open()` → a `Reader` (manifest / blocks / `array` → NumPy / `array_roi`
for an ROI / `table` → polars / `table_arrow` / `table_dict` / `column` / `verify`) and a `Builder`
(`add_array` / `add_table` / `set_field` / `add_source` / `pack`), with a typed `TesseraError`.

Reading:

```python
{{#include ../../../tessera/crates/tessera-py/tests/write_example.py:read}}
```

Writing — the half that used to be advertised but never shown:

```python
{{#include ../../../tessera/crates/tessera-py/tests/write_example.py:array}}
```

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
import init, { verify_manifest, verify_signature, product_id } from "./tessera_wasm.js";
await init();
verify_manifest(bytes);         // seal check, in-browser
```

*Referenced checks:* `wasm-core` (the core compiles to wasm32) + `wasm-bindgen-smoke` (bindings build →
bindgen → **execute in Node**). Columnar payload reads at the RS↔TS boundary go through **Arrow-JS** — the
WASM core verifies + locates the bytes, Arrow-JS reads them — so the heavier Vortex/zarrs stacks aren't
needed in-browser.
