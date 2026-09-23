# Tessera

**A substrate-agnostic, Rust-native FAIR data-product format** — `fd5` v2. One immutable,
content-addressed, self-describing product (manifest + shape-dispatched storage blocks), with a
single identity / provenance / integrity / versioning spine.

> ⚠️ **Pre-1.0 — the on-disk format is not yet frozen.** Tessera is in late-stage alpha (development on
> `dev`). The model, container, and CLI are real and tested, but the byte format may still change before
> the v0.1 freeze. **Keep your original data** — do not yet rely on a `.tsra` as the only copy of
> something irreplaceable.

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

**Why not just Parquet or HDF5?** They store bytes; Tessera adds the seal, provenance, versioning,
signing and cloud/FAIR distribution *around* them — see the capability comparison in
**[docs/book/src/why-tessera.md](docs/book/src/why-tessera.md)**.

## Install

Tessera's one native dependency is **libhdf5** (used only to *read* vendor acquisitions at ingest). How
you get it decides which install path fits. In order of least-effort-for-a-user first:

**1. Prebuilt binary (recommended)** — self-contained `tessera`, HDF5 bundled in, zero system deps.
Published to [GitHub Releases](https://github.com/vig-os/tessera/releases) by
[cargo-dist](https://opensource.axo.dev/cargo-dist/) for Linux & macOS (x86-64 + arm64):

```bash
# curl | sh installer (from a release) — or `cargo binstall tessera-cli`, or grab the tarball
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/vig-os/tessera/releases/latest/download/tessera-cli-installer.sh | sh
```

The first release (`0.1.0-alpha.1`) is staged but deliberately **held** — see `release-plz.toml`. Until
it is cut, build from source with one of the paths below.

**2. From source, self-contained** — no system HDF5 needed; builds a private copy from vendored source.
Needs **CMake + a C compiler** (and a few minutes). Works on any distro, including `lib64` ones
(Fedora / RHEL / SUSE / nix):

```bash
cargo install --git https://github.com/vig-os/tessera --features static-hdf5 tessera-cli
```

**3. From source, system HDF5 (fastest dev build)** — links a libhdf5 already on the box via
`pkg-config`. This is the default (no `static-hdf5` feature):

```bash
cd tessera && cargo build --release -p tessera-cli    # needs libhdf5 + pkg-config installed
```

**4. Nix (run, or install onto PATH)** — the flake exposes `tessera` as a package; nix supplies the
whole runtime closure (incl. libhdf5), so nothing is vendored and builds are reproducible:

```bash
nix run     github:vig-os/tessera -- inspect study.tsra   # run without installing
nix profile install github:vig-os/tessera                 # put `tessera` on PATH
```

**5. Nix dev shell (contributors)** — pins the whole toolchain + native deps (hdf5/zstd/…):

```bash
cp .envrc.example .envrc && direnv allow   # or: nix develop
cd tessera && cargo test
```

*Why bundled HDF5 is safe:* HDF5 is a read-only *input* format — Tessera re-encodes everything to
Zarr+pcodec / Vortex on write, so the libhdf5 build is never in a sealed `.tsra`'s byte-path and can
never affect a `content_hash`.

> **Consuming Tessera from another repo?** See [docs/CONSUMING.md](docs/CONSUMING.md) — git/flake refs while the crates.io/PyPI release stays held.

### Python

The `tessera` Python package (read / verify / write `.tsra`, returning NumPy arrays and
polars/pyarrow tables) is a pure [pyo3](https://pyo3.rs) `abi3` extension — one wheel serves CPython
≥ 3.9. Build the reproducible wheel with nix:

```bash
nix build github:vig-os/tessera#wheel      # → result/tessera-*-cp39-abi3-linux_<arch>.whl
pip install result/*.whl                   # needs a libstdc++ on the loader path
```

(The nix-built wheel is a `linux_<arch>` wheel, not yet `manylinux` — auditwheel/manylinux repair for
a PyPI upload is a tracked follow-up. It installs and imports today in any compatible-glibc env.)

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

# Read a table column → CSV: a preview, or the whole column, or a row range
tessera read corpus/files/listmode_events.tsra events -c e0 --limit 5    # preview
tessera read corpus/files/listmode_events.tsra events -c e0 --all > e0.csv
tessera read corpus/files/listmode_events.tsra events -c e0 --rows 0:100 # a slice

# Look at an array without decoding the whole volume
tessera stats   corpus/files/recon_int16.tsra volume              # shape · dtype · codec · min/max/mean
tessera slice   corpus/files/recon_int16.tsra volume --index "32,:,:"   # one plane → CSV
tessera project corpus/files/recon_int16.tsra volume --axis z --mode max  # MIP → CSV
# (Prefer NumPy/DataFrames? the `tessera` Python package returns np.ndarray / polars / pyarrow.)

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
