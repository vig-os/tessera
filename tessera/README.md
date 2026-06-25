# Tessera (spike)

A substrate-agnostic, Rust-native **FAIR data product** format — the evolution of
[fd5](../README.md) from "FAIR Data on HDF5" to a manifest + shape-dispatched storage blocks
(N-D chunked arrays via zarrs, columnar tables via arrow/parquet/lance), unified by one
identity / provenance / Merkle-hash / version spine.

> Status: **spike** on branch `spike/tessera-core`. The manifest/identity/hash/provenance
> spine and block descriptors are real; block *payload* I/O is stubbed behind the
> `array-zarr` / `table-arrow` features. Design + benchmark rationale: [`../docs/rfc-tessera.md`](../docs/rfc-tessera.md).

## Why

Real medical-imaging data has two shapes — dense N-D arrays (CT/PET volumes) and large event
tables (listmode coincidences) — and no single byte layout is optimal for both. Tessera does
not invent a new codec; it composes the proven engine per shape under one FAIR product model.
Evidence: fd5 issues #192 (recon native-dtype + cubic chunks), #193 (columnar listmode),
#194 (zstd).

## Layout

```
crates/tessera-core
  manifest.rs     manifest model + (de)serialisation + identity
  identity.rs     id = blake3 over identity inputs
  hash.rs         blake3 Merkle over block digests -> content_hash
  provenance.rs   sources/ DAG
  block/array.rs  N-D chunked array block (zarrs backend, feature-gated)
  block/table.rs  columnar table block (arrow backend, feature-gated)
  product.rs      ProductBuilder: add blocks -> seal()
```

## Build

```bash
cd tessera
cargo test            # core spine + descriptors (no backend deps)
cargo check --features full
```

## Next

1. Implement `ArrayBlock::write_zarr` over `zarrs` (sharded, cubic, zstd).
2. Implement `TableBlock::write_parquet` over `arrow`/`parquet` (+ optional row index).
3. Single sealed container (`.tess`) vs directory-of-shards (RFC §8).
4. `pyo3` bindings for Python parity with the fd5 package.
5. `dicom-rs` ingest: DICOM series → `recon` product.
