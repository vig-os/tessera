# RFC: Tessera — a substrate-agnostic FAIR data product format

> Status: **Draft / spike** (`spike/tessera-core`)
> Supersedes the substrate assumption of fd5 ("FAIR Data on **HDF5**"). Tessera keeps fd5's
> *model* (one self-describing, immutable, hashed FAIR **data product**) and generalises the
> *substrate* from a single HDF5 file to a manifest + shape-dispatched storage blocks.

## 1. Motivation

fd5 is "FAIR Data on HDF5". Real medical-imaging data (DUPLET-Patients: CT/PET volumes,
listmode coincidence events, lifetime spectra, ROIs, calibrations) has **two fundamentally
different shapes** — dense N-D arrays and large event tables — and no single byte layout is
optimal for both. Benchmarks on real data (see §7) show the axes are in tension:

- compression ⟂ random-access (Lance stores volumes raw to get O(1) row `take`)
- chunked-array slicing ⟂ columnar projection (you cannot be Zarr *and* Parquet at once)

**Thesis:** the "best of all worlds" is not a novel byte codec — it is a thin Rust-native
*composition* layer: one FAIR identity/provenance/hash/version **spine** over storage
**blocks** that dispatch by data shape to the proven engine for that shape.

## 2. Design principles (inherited from fd5, generalised)

1. One immutable, content-hashed **data product** per artifact.
2. Self-describing: embedded schema, `description`, units (`@units`/`@unitSI`).
3. Provenance as a DAG (`sources/`), not a string.
4. Store at the source's **native precision** (CT/PET = int16 + rescale, not float32).
5. **Substrate-agnostic**: the product model does not assume HDF5.
6. FAIR-first; AI-retrievable; offline-self-contained.

## 3. Product anatomy

```
PRODUCT  =  manifest  +  N typed blocks
              │                  │
   identity / Merkle hash /   shape-dispatched:
   provenance DAG / schema /   ├── ARRAY block  (chunked, sharded, zstd, cubic)
   units / version chain       └── TABLE block  (columnar, per-col codec, row-index)
```

- **Manifest** (the novel IP): identity, schema, units, provenance, and a list of block refs;
  Merkle-hashed for immutability + integrity. Generalises fd5's root attrs to span arrays,
  tables, *and* a version chain.
- **Array block**: N-D chunked storage with sharding (Zarr v3 semantics). Volumes, sinograms,
  μ-maps. Default: native dtype, cubic chunks, zstd.
- **Table block**: columnar with per-column codecs + optional secondary index. Listmode
  events, spectra, ROIs. Projection + (via index) fast random per-event `take`.
- **Version chain**: append-only manifest chain (audit-trail; cf. fd5 issues #167–170)
  reconciled with content-addressing.

## 4. Block dispatch rules

| Product / dataset | Block | Backend (engine) |
|---|---|---|
| CT / PET / μ-map / parametric volume | array | zarrs (chunked, sharded) |
| sinogram / michelogram | array | zarrs |
| listmode events / coincidences | table | arrow/parquet (+ index) or lance |
| lifetime / energy spectra (histograms) | table or small array | arrow / zarrs |
| ROI definitions, calibration tables | table | arrow |

## 5. Storage & encoding defaults (from benchmarks)

- Arrays: **native integer dtype** + `rescale_slope`/`rescale_intercept`; **cubic chunks**
  (e.g. 64³); **zstd** codec; sharding for cloud range-reads.
- Tables: **columnar** (never row-major compound); per-column codec; optional row index.
- Compression: zstd (multithreaded via rayon over chunks/shards, or blosc2).
- Hash: **blake3** Merkle tree (faster than SHA-256, Merkle-friendly).

### 5.1 Engine selection (decided from benchmarks)

| Shape | Access pattern | Engine | Bench evidence |
|---|---|---|---|
| dense volume (CT/PET/sinogram) | full-load · orthogonal · ROI | **zarrs** (sharded, cubic, zstd, native int) | cubic 18–24× orthogonal; sharded 5 vs 513 files |
| dense volume | web tiles | OME-Zarr (derived export) | viewers only, not archival |
| event table | projection · vectorized per-event | **Parquet/Arrow** (default) | fastest projection + MT + lakehouse ecosystem |
| event table | projection **+** random per-event `take` | **Lance** (option) | random `take` 16× vs columnar HDF5 |
| event table | smallest, single-file-in-product | HDF5-columnar | 84 vs 99 MB |

- **ARRAY block backend → zarrs.** **TABLE block backend → Parquet/Arrow default, Lance option.**
- Lance is *not* a Parquet superset (separate format/ecosystem); it adds random-row `take` + versioning. Use Parquet for interop, Lance for random-access/versioned event data.
- Cross-cutting: `zstd`/`blosc2` (MT compress), `blake3` (hash), `object_store` (cloud), `dicom-rs` (ingest), `pyo3` (bindings).

## 6. Rust crate layout (this spike)

```
tessera/crates/tessera-core
  manifest.rs     manifest model + (de)serialisation
  identity.rs     id = algo-prefixed blake3 over identity inputs
  hash.rs         blake3 Merkle over blocks -> content_hash
  provenance.rs   sources/ DAG
  block/array.rs  N-D chunked array block (zarrs backend, feature-gated)
  block/table.rs  columnar table block (arrow backend, feature-gated)
  product.rs      Product = manifest + blocks; create()/seal()
```

Reuse, do not reinvent: `zarrs`, `arrow`/`parquet`, `lance`, `zstd` (`multithread`),
`blosc2`, `blake3`, `object_store`, `dicom-rs` (ingest), `pyo3` (Python parity).

## 7. Evidence (real DUPLET data; cold cache via `posix_fadvise`)

| Finding | Implication for Tessera |
|---|---|
| int16 vs float32: 2.6× smaller, lossless | native-dtype arrays |
| cubic vs slice chunks: 18–24× faster orthogonal | cubic chunk default |
| zstd-9 vs gzip-4: 5% smaller **and** ~2× faster | zstd default |
| compound HDF5 can't project (0.62 ≈ full read) | columnar tables |
| Lance `take` 16× faster random row | optional row index |
| Zarr sharded: 5 files vs 513 unsharded | shard array blocks |
| plain HDF5 zstd filter single-threaded on write | rayon/blosc2 parallel compress |

Full benchmark tables and method: fd5 issues #192 (recon), #193 (listmode), #194 (codec).

## 8. Open questions

- Single sealed file (sharded container / `.tess`) vs directory-of-shards — must preserve
  the "offline, self-contained, inspectable" property.
- Reconcile content-addressing (immutability) with append-only versioning.
- Multi-language readers + a conformance suite (a format with one reader is a liability).
- Supersession vs sibling of fd5 (decides repo: rename vs new repo).

## 9. Non-goals

- A novel byte-level codec/container that out-competes zarrs/arrow/lance on their home turf.
- Parsing vendor formats (DICOM, GE `.dat`/`.BLF`) — that is ingest, a separate package.
- Interactive web tile streaming (export to sharded OME-Zarr as a derived render layer).
