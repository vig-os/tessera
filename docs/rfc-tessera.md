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
   *Any* dtype is supported (int8/16/32/64, uint8/16/32/64, float16/32/64) — **int16 is a
   recommendation for scanner-reconstructed CT/PET, not a constraint**; computed float
   products (SUV/parametric/TOFPET-lifetime/μ-maps) use float32/64.
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
| event table | size · projection · random `take` · vectorized | **Vortex** (decided, spikes S0–S11) | smallest + O(1) random take + ALP floats + filter pushdown |
| event table | external interop / lakehouse query | Parquet (derived export) | DuckDB/Spark; but Vortex→Arrow already gives this zero-copy |

- **ARRAY block backend → zarrs.** **TABLE block backend → Vortex** (see `tessera/docs/SPIKE-RESULTS.md`).
- Vortex (Rust + Arrow-native, FastLanes/ALP/FSST addressable encodings) won every measured axis vs Parquet and Lance: **smallest** (21% < Parquet on events, 23% on pure floats), **random `take` ~0.3–0.5 ms and flat to 100M rows** (Lance degrades to 0.9 s), **filter pushdown**, and **zero-copy into DuckDB**. It slots *under* the Tessera spine (encoding layer) — so the **Merkle tree stays integrity-only**, not a random-access index. Lance is not needed (it bundles a competing dataset/version layer and lost on size + scale).
- Cross-cutting: `zstd`/`blosc2` (MT compress), `blake3` (hash), `object_store` (cloud), `dicom-rs` (ingest), `pyo3` (bindings).

### 5.2 Table backends & the random-access axis

Parquet and Lance sit at opposite ends of one axis: Parquet wins interop + projection +
ecosystem; Lance wins random per-event `take` + versioning (bench: 16× on random row).
Tessera resolves this with **flavors, not runtime conversion**:

- The `TableSpec` declares the data + an optional `row_index` (random-access *intent*) and a
  future `encoding` field naming the chosen backend. **The flavor is picked at *write time*
  by the dominant access pattern**, recorded in the manifest:
  - scan / project / feed Spark·DuckDB → **Parquet** flavor (default).
  - random per-event fetch · versioning · ML sampling → **Lance** flavor.
- **Both decode into Arrow**, so downstream code is backend-agnostic and zero-copy either way.
  The cheap "adaptor" is the *in-memory Arrow* handoff — not an on-disk transcode.
- **A pq↔lance on-disk adaptor is a full rewrite** (different byte layouts encode different
  tradeoffs — Lance uses small/indexed blocks for O(1) take, Parquet uses big compressed
  pages). That is minutes for GB-scale tables → **never on the read path**. Use it once to
  *materialize* a flavor, not per query.
- **Parquet already has a partial answer**: Page Index (ColumnIndex+OffsetIndex) + Bloom
  filters give fast *predicate point-lookups on a sorted key*. Arbitrary positional `take`
  needs a sidecar `rowid → page-offset` index (the "addon") + range read — buildable, but it
  still pays per-page decompression, so it *approximates* Lance, not equals it.
- **Dual-hot tables** (genuinely hot on both axes) store **two blocks** in one product (a
  Parquet block for scans + a Lance block or sidecar index for take) — a deliberate *storage*
  cost, not a *latency* one. Rare; not the default.

Net: support the axis **backend-agnostically** (flavors chosen per workload), never transcode
on read, and let emerging both-axes formats (Vortex, Nimble) slot in as additional flavors.

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
