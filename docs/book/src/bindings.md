# Bindings: Python & WASM

Both bindings wrap `tessera-core` directly — no reimplementation, so they inherit the same identity,
seal, and verification logic.

> **Evidence form:** these are the "executed doctest / referenced check" chapters — the examples run under
> the bindings' own test gates (`cargo test` doctests + the hermetic Python/WASM flake checks), not as a
> CLI `trycmd`. See the [Introduction](./intro.md).

## Python (`pyo3`, abi3)

`import tessera` gives a `Reader` (open / manifest / blocks / `read_array` → NumPy / `read_array_subset`
for an ROI / `read_table` / `read_table_column` / `verify`) and a `Builder` (`add_array` / `add_table` /
`set_field` / `add_source` / `pack`), with a typed `TesseraError`.

```python
import tessera
r = tessera.Reader("study.tsra")
r.verify()                      # raises TesseraError on tamper
vol = r.read_array("volume")    # -> numpy.ndarray, native dtype
roi = r.read_array_subset("volume", [(0, 32), (0, 32), (0, 32)])   # ROI, only the chunks it needs
```

*Referenced check:* the `tessera-py-import` flake check does a full NumPy **write → read → verify**
round-trip over the corpus (numpy + polars + pyarrow).

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
