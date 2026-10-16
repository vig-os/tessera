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
import init, { verify_manifest, verify_signature, product_id } from "./tessera_wasm.js";
await init();
verify_manifest(bytes);         // seal check, in-browser
```

*Referenced checks:* `wasm-core` (the core compiles to wasm32) + `wasm-bindgen-smoke` (bindings build →
bindgen → **execute in Node**). Columnar payload reads at the RS↔TS boundary go through **Arrow-JS** — the
WASM core verifies + locates the bytes, Arrow-JS reads them — so the heavier Vortex/zarrs stacks aren't
needed in-browser.
