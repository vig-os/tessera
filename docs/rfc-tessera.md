# RFC: Tessera — a substrate-agnostic FAIR data product format

> Status: **Draft / spike** (`spike/tessera-core`)
> Supersedes the substrate assumption of fd5 ("FAIR Data on **HDF5**"). Tessera keeps fd5's
> *model* (one self-describing, immutable, hashed FAIR **data product**) and generalises the
> *substrate* from a single HDF5 file to a manifest + shape-dispatched storage blocks.

## 0. Capstone decisions (settled by spikes S0–S15, on real DUPLET data)

The spike campaign converged on a clear, evidence-backed architecture. Headlines:

1. **Codec is universal; the container is chosen by *access pattern*.** A single codec —
   **`pcodec`** (lossless, FastLanes/ALP lineage) — is the compression winner for *both*
   shapes: it beat zstd by **21% on int16 CT (74.3 vs 94.5 MB)** and **33% on float PET
   (0.6 vs 0.9 MB)**, beat Vortex's own encodings on tables (73.3 vs 77.7 MB), and beat the
   lossy float codecs (zfp/bitround don't pay off on sparse medical data). **pcodec is shared
   plumbing, not the differentiator.** Keep **zstd** as the decades-stable fallback (~25%
   larger) for archival risk-aversion (pcodec is young — see §8 maturity note).

2. **Volumes → Zarr/OME-Zarr chunk grid · 64³ cubic chunks · pcodec.** Cubic chunks give
   **axis-independent (isotropic)** orthogonal access; 64³ is the size sweet spot (smallest-
   within-4% + fastest balanced reads + best ROI; 32³ smaller-but-slower, 256³ bigger+slower).
   "Container vs HDF5" is irrelevant — **Zarr ≡ HDF5** byte-for-byte at equal codec+chunking.

3. **Tables → Vortex engine** — kept for the **engine**, not compression. pcodec *matches*
   Vortex on table size, but Vortex wins **access**: random `take` **7–42× faster** than
   Zarr+pcodec, 3× projection, plus filter pushdown + zero-copy Arrow→DuckDB. A table format
   is about *how you access* (project/filter/random/join) → an engine problem Zarr isn't.

4. **The split, stated once:** *bulk-decode-local-N-D-region* (volumes) → **Zarr** (opaque
   cubic chunk + pcodec; Vortex's addressability wasted); *project · filter · random-access ·
   join* (tables) → **Vortex** (addressable encodings + scanner + Arrow). "Flat/Morton Vortex
   for volumes" was tested and loses (1.7–2× bigger; un-chunked = un-adaptive); "Zarr-on-Vortex"
   reduces to "chunk grid + a good codec" = **Zarr+pcodec** (already available).

5. **Layout (resolves §8):** a product is a **content-addressed collection** with one identity
   (Merkle root). Its **canonical serialization is a single sealed `.tessera` file** — a
   **STORED zip64** (no recompression; central directory = internal index → **cloud range-reads
   into the one file**, à la Zarr ZipStore). This preserves fd5's **inseparability** (one file,
   can't orphan a block by accident) *and* partial reads. An **exploded S3 prefix** is an
   opt-in working form only for concurrent-writer ingest / copy-on-write versioning. `pack`/
   `open` preserve the Merkle root → all forms are the same product.

6. **Three manifests, three layers** (don't conflate): **Tessera manifest** = *what it is*
   (id/schema/units/provenance/Merkle root, Layer 1); **OCI manifest** = *how it ships*
   (blobs+digests; registry distribution via `zot`/`registry:2` on MinIO, or an OCI-layout of
   objects ≈ the exploded prefix); **`ro-crate-metadata.json`** = *what it means* (Schema.org
   JSON-LD discovery, Layer 2, derived export).

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
   provenance DAG / schema /   ├── ARRAY block  (Zarr grid, 64³ cubic, pcodec)
   units / version chain       └── TABLE block  (Vortex engine, addressable, row-index)
```

- **Manifest** (the novel IP): identity, schema, units, provenance, and a list of block refs;
  Merkle-hashed for immutability + integrity. Generalises fd5's root attrs to span arrays,
  tables, *and* a version chain.
- **Array block**: N-D chunked, sharded Zarr/OME-Zarr grid. Volumes, sinograms, μ-maps.
  Native dtype, **cubic 64³ chunks**, **pcodec** codec, multiscale pyramid (OME-NGFF → viewers).
- **Table block**: **Vortex** columnar engine — addressable encodings + filter pushdown +
  optional row index; decode zero-copy to Arrow. Listmode events, spectra, ROIs, calibration.
- **Version chain**: append-only manifest chain (audit-trail; cf. fd5 issues #167–170)
  reconciled with content-addressing.

## 4. Block dispatch rules

| Product / dataset | Block | Backend (engine) |
|---|---|---|
| CT / PET / μ-map / parametric volume | array | **zarrs** (cubic 64³, pcodec, sharded) |
| sinogram / michelogram | array | **zarrs** |
| listmode events / coincidences | table | **Vortex** (addressable, filter pushdown) |
| lifetime / energy spectra (histograms) | table or small array | Vortex / zarrs |
| ROI definitions, calibration tables | table | **Vortex** |

## 5. Storage & encoding defaults (from benchmarks)

- Arrays: **native integer dtype** + `rescale_slope`/`rescale_intercept`; **cubic 64³ chunks**
  (isotropic access); **`pcodec`** codec (lossless; zstd fallback for archival risk-aversion);
  sharding for cloud range-reads.
- Tables: **columnar** (never row-major compound) via the **Vortex** engine; addressable
  encodings + filter pushdown + optional row index; decode zero-copy to Arrow.
- Compression: **pcodec** (lossless winner on both shapes); **zstd** stable fallback;
  blosc2/rayon where MT compress matters.
- Hash: **blake3** Merkle tree (faster than SHA-256, Merkle-friendly).

### 5.3 Codec selection — `pcodec` (with `zstd` fallback)

Wide Zarr codec sweep (lossless unless noted), real DUPLET data:

| | CT int16 | PET float32 | notes |
|---|---:|---:|---|
| zstd-3 / gzip | 94.5 / 96.6 MB | 0.9 MB | the stable baseline |
| zstd-9 / delta+zstd | 90.6 / 88.6 MB | — | best zstd-family |
| blosc-lz4 | 159.8 MB | — | speed-not-size |
| **pcodec** | **74.3 MB** | **0.6 MB** | **lossless winner, both shapes** |
| bitround+zstd | — | 0.7 MB (lossy) | *worse* than lossless pcodec |
| zfp fixed-accuracy | — | 3.7 MB (lossy) | far worse on sparse PET |

`pcodec` wins losslessly by 16–21% (CT) and 33% (PET); lossy codecs don't pay off on sparse
medical data and carry quantitative/regulatory risk. The codec is a per-block field in the
manifest, so swapping pcodec↔zstd needs no format change.

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

## 8. Container & layout (resolved) + remaining open questions

**Resolved (see §0.5–0.6):** the canonical serialization is a **single sealed `.tessera`
file** — a **STORED zip64** holding `manifest.json` + `*.zarr/` array blocks + `*.vortex`
table blocks. Its central directory is the internal index → **cloud range-reads into the one
file** (Zarr `ZipStore` proves the pattern), so the single file is *both* inseparable (fd5's
promise) *and* partial-readable. An **exploded S3 prefix** (≈ an OCI image-layout of
content-addressed objects) is an opt-in working form for concurrent-writer ingest and CoW
versioning. **Distribution** via OCI artifacts (`zot`/`registry:2` backed by MinIO, or a bare
OCI-layout in a bucket). **Discovery** via a derived `ro-crate-metadata.json`. `tar` is
cold-archive only (no random index). All forms share one **Merkle root** = identity.

Accidental separation is prevented by: (a) the share op produces one file; (b) the reader
validates the Merkle root and refuses orphaned/partial blocks; (c) each block self-references
its product id.

**Still open:**
- Reconcile content-addressing (immutability) with append-only versioning (the CoW manifest
  chain on the exploded prefix; cf. fd5 audit-trail #167–170).
- Maturity hedge: `pcodec` and `vortex` are young; pin spec versions + ship vendored readers
  in the Tessera crate so archives stay readable for decades (spike **S15**). zstd is the
  conservative fallback codec.
- Multi-language readers + a conformance suite (a format with one reader is a liability).
- Supersession vs sibling of fd5 (decides repo: rename vs new repo).
- Bit-exact roundtrip of `pcodec`/Vortex on int16 + float32 (spike **S13**) before clinical use.

## 10. Durability, integrity & audit (spikes S16–S17)

### Durability & DAQ ingest
Vortex/columnar files are **footer-at-end**: a writer that dies before close → the *whole file
is unreadable* (tested: truncation at even 99.9% = 0 rows recovered). Write ~48 MB/s / 1.3 M
rows/s — not streaming-optimised. Therefore:
- **Never stream a DAQ into a single Vortex/`.tessera`** — a crash loses the whole run.
- **Ingest = fragment-append**: finalise small fragments (footer written) per time-window / N
  events, each committed by an **atomic manifest pointer update** → crash-tolerant **up to the
  last committed fragment** (bound loss by the fragment interval). Or **raw WAL** (the vendor
  `.dat`/`.BLF` append log) → **offline convert** to Tessera (what the DUPLET pipeline does).
- **Parallel write** = N writers → N fragments → one manifest (not concurrent writers to one
  file). The sealed single `.tessera` is the **archival** form only.

### Integrity & tamper-evidence
**Tamper-*proof* is impossible for data you hold** (you can always rehash). Design for
**tamper-evident + signed + externally-anchored**:
1. **In-product integrity** — native **Merkle root** (self-verifying offline).
2. **History** — append-only manifest/hash chain (audit-trail #167–170).
3. **Source-rooted signing** (defends against *injection*): sign the acquisition Merkle root
   **at the scanner**, with a **hardware-rooted key (TPM/HSM)**, leveraging the existing
   **DICOM PS3.15 Digital Signatures** standard. Trust must originate at the source, or you
   only prove a downstream signer touched it.
4. **Chain of custody** — every transform (raw→events→recon) is a signed Tessera product whose
   `sources` references the parent hash. Verify = walk the DAG to the scanner-signed root,
   checking signature + signer identity + content hash + timestamp at each hop.
5. **Anchoring** — **private Sigstore (Fulcio + Rekor) on hospital infra**, or X.509 + an
   internal **RFC-3161 timestamp authority**. **Not the *public* Sigstore** — it leaks
   operational metadata + (GDPR) signer identity; only hashes/signatures are logged, never PHI,
   but hospitals won't egress. Private instance = same guarantees, internal trust boundary.
6. **Immutability** — **WORM** (S3 Object Lock, compliance mode).

**Regulatory fit:** FDA 21 CFR Part 11 (attributable e-signatures + audit + integrity), DICOM
PS3.15 (signatures + TLS), GDPR/HIPAA (no PHI in logs; internal infra; device/role identities).
**Limit:** proves *genuine, unaltered device output* — not *medical* correctness; a fully
compromised scanner is the root-of-trust boundary (mitigate: device attestation, physical
security, audit).

### Distribution / RDM (corrected — DataLad is not the backbone)
Cloud-native stack: **OCI artifact** (`zot`/`registry:2` on **MinIO**) for addressing +
distribution; **cosign (private Sigstore)** for sign + tamper-evident anchor; **S3 Object Lock**
for WORM; **InvenioRDM** (Zenodo's engine) for DOI · versioning · FAIR landing pages · access
control, fed by the derived **RO-Crate/DataCite** exports. **DataLad (git + git-annex)** is an
*optional* bridge for git-native workflows — not the audit backbone (it duplicates OCI digests,
cosign/Rekor, and InvenioRDM versioning with a git-centric paradigm).

## 9. Non-goals

- A novel byte-level codec/container that out-competes zarrs/arrow/lance on their home turf.
- Parsing vendor formats (DICOM, GE `.dat`/`.BLF`) — that is ingest, a separate package.
- Interactive web tile streaming (export to sharded OME-Zarr as a derived render layer).
