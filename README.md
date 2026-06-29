# Tessera

**A substrate-agnostic, Rust-native FAIR data-product format** — `fd5` v2. One immutable,
content-addressed, self-describing product (manifest + shape-dispatched storage blocks), with a
single identity / provenance / integrity / versioning spine.

> ⚠️ **Pre-1.0 — the on-disk format is not yet frozen.** Tessera is in late-stage alpha on the
> `spike/tessera-core` branch. The model, container, and CLI are real and tested, but the byte format
> may still change before the v0.1 freeze. **Keep your original data** — do not yet rely on a `.tsra`
> as the only copy of something irreplaceable.

## Why

Real scientific & medical-imaging data has two shapes — dense N-D arrays (CT/PET volumes) and large
event tables (PET listmode coincidences) — and no single byte layout is optimal for both. Tessera
does not invent a codec; it **composes the proven engine per shape** under one FAIR product model:

- **Arrays** → Zarr v3 + [`pcodec`](https://github.com/mwlon/pcodec), 64³ cubic chunks (lossless;
  −21% CT / −33% PET vs zstd). Sharded ROI reads decode only the chunks they touch.
- **Tables** → [Vortex](https://github.com/spiraldb/vortex) — smallest on disk, O(1) random-take,
  filter-pushdown, zero-copy to Arrow/DuckDB. Column projection reads only the columns you ask for.
- **Identity & integrity** — blake3 hash-on-write, a Merkle-Mountain-Range `content_hash`, and a
  `manifest_hash` seal that transitively commits to every block digest + all metadata.

## Install / build

The repository is Nix-managed. The reliable path is the dev shell:

```bash
direnv allow          # or: nix develop   — loads the pinned toolchain + native deps (hdf5/zstd/…)
cd tessera && cargo test
cargo build --release -p tessera-cli   # the `tessera` binary
```

(Building outside the Nix shell needs HDF5 headers + libs on `HDF5_DIR`; see `tessera/CLAUDE.md`.)

## Quickstart

A conformance corpus ships in `tessera/corpus/files/`. Every command that opens a `.tsra` verifies
its magic + manifest seal; `verify` additionally re-checks every block digest.

```bash
# Inspect & verify a product
tessera inspect corpus/files/recon_int16.tsra
tessera verify  corpus/files/recon_int16.tsra

# Navigate the structure like a zarr hierarchy
tessera tree corpus/files/listmode_events.tsra      # root status · meta · blocks+columns · sources
tessera ls   corpus/files/listmode_events.tsra events
tessera read corpus/files/listmode_events.tsra events -c e0 --limit 5   # cross-block column → CSV

# Ingest a vendor acquisition (normalise at the door), or a declarative multi-product spec
tessera ingest ge-hdf5 LIST.h5 out.tsra --name DP06-lm --timestamp 2024-01-01T00:00:00Z
tessera ingest --spec docs/examples/ingest-ge-listmode.toml --out ./study

# Read over the wire (range-read from S3 — only the bytes you need), with the `cloud` feature
tessera inspect s3://bucket/key.tsra
```

### Versioning & audit (copy-on-write, git-shaped)

A small edit (a metadata correction, attaching a derived block) should **not** copy the data, but
must stay audit-trailed. Tessera versions products in a **content-addressed repository** — a metadata
edit writes exactly one new object (the manifest); unchanged blocks are shared by digest.

```bash
tessera init repo
tessera import repo corpus/files/recon_int16.tsra        # prints the lineage id
tessera commit repo <id> --set tracer=FLT                # new version, data NOT recopied
tessera commit repo <id> --add-block roi=roi.tsra:roi    # attach an already-encoded block
tessera log    repo <id>                                 # version history
tessera diff   repo <tip>                                # what changed + lineage verdict

tessera publish repo <tip> out.tsra        # history-free standalone (git archive) — for DOI/handover
tessera seal    repo <tip> out.tsra        # history-preserving bundle  (git bundle) — for archival
```

The verbs map onto git muscle memory because the models are isomorphic: block ≈ blob,
manifest ≈ commit, `manifest_hash` ≈ sha, `supersedes` ≈ parent. `id` is the stable lineage handle;
`manifest_hash` is the version (cite `id@manifest_hash`).

## The model

| | |
|---|---|
| **`id`** | `blake3(JCS({product, name, timestamp}))` — the stable lineage handle (same across versions). |
| **`content_hash`** | MMR Merkle root over the ordered block digests — the data fingerprint. |
| **`manifest_hash`** | `blake3(JCS(manifest))` — *the seal*; commits to id-inputs, sources, every block digest, all metadata. |
| **Container** | a single sealed **STORED zip64 `.tsra`** (range-readable); opt-in exploded prefix / OCI artifact for cloud. |
| **Schema** | open-world product-schema registry (`recon`/`listmode`/…); engine is schema-driven, schemas are data. |

## Crates

- `tessera-core` — format spine: manifest, identity, hashing, provenance, schema, block descriptors (no I/O).
- `tessera-io` — the engine: array (Zarr+pcodec) & table (Vortex) codecs, the `.tsra` container, the
  streaming write engine, the content-addressed versioning repository, cloud range-reads.
- `tessera-ingest` — vendor decoders (DICOM, GE-HDF5, NIfTI, raw) + the declarative ingest engine.
- `tessera-cli` — the `tessera` command-line tool.
- `tessera-py` — Python bindings (pyo3, abi3): `import tessera`.
- `tessera-wasm` — `wasm32` bindings.

## Documentation

- `docs/rfc-tessera.md` — the design (decisions, fd5 conventions, impl-readiness).
- `docs/adr/` — Architecture Decision Records (identity, container, versioning/audit, ingest, …).
- `tessera/docs/FEATURE-MATRIX.md` — status + passing gates + perf SLA floors.
- `tessera/docs/SPEC.md` — the `.tsra` byte format; `tessera/corpus/` — the conformance corpus.

## License & provenance

Tessera is `fd5` v2 (repo history kept). See the founding white-paper for the FAIR-data vision.
