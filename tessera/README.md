# Tessera — workspace

The Rust workspace for **Tessera** (`fd5` v2), a substrate-agnostic FAIR data-product format.
For the project overview, quickstart, and the model, see the [root README](../README.md).

> ⚠️ **Pre-1.0** on branch `spike/tessera-core`. The manifest/identity/hash/provenance spine, the
> array (Zarr+pcodec) & table (Vortex) payload codecs, the `.tsra` container, the streaming write
> engine, the content-addressed versioning repository, ingest, and the CLI are all implemented and
> gated. The on-disk format is not yet frozen.

## Crates

```
crates/tessera-core     format spine — manifest · identity · hash (blake3 MMR) · provenance ·
                        schema registry · array/table block descriptors (no I/O)
crates/tessera-io       the engine — array (Zarr v3 + pcodec) & table (Vortex) codecs · the .tsra
                        container (STORED zip64, range-readable) · streaming write engine ·
                        content-addressed versioning repository (repo.rs) · cloud range-reads
crates/tessera-ingest   vendor decoders (DICOM · GE-HDF5 · NIfTI · raw) + declarative ingest engine
crates/tessera-cli      the `tessera` binary (inspect/verify/tree/ls/read/ingest/sign/bench +
                        versioning: init/import/commit/log/diff/publish/seal)
crates/tessera-py       Python bindings (pyo3, abi3) — `import tessera`
crates/tessera-wasm     wasm32 bindings
```

## Build & test

```bash
# inside the Nix dev shell (direnv allow / nix develop at the repo root)
cd tessera
cargo test                 # or: cargo nextest run
cargo build --release -p tessera-cli
```

The hermetic gate (`nix flake check`, run from the repo root) is the source of truth: workspace
clippy `--all-features -D warnings`, nextest, doctests, fmt, the wasm build, MinIO range-read, OCI
round-trip, and the guardrails gates. Conformance fixtures live in `corpus/`.

## Docs

- `docs/rfc-tessera.md` — design + benchmark rationale.
- `docs/FEATURE-MATRIX.md` — status + passing gates + perf SLA floors.
- `docs/SPEC.md` — the `.tsra` byte format · `docs/adr/` (repo root) — decision records.
