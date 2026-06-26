# Tessera — feature matrix & passing gates (ALOCA)

The single tracked baseline: every capability, its **gate** (measurable pass condition), and the
**evidence** (spike/bench/test). Future development is benched/compared against this — perf rows
are *regression floors* (don't go below), correctness rows are *required* (binary).

**Status:** ✓ proven · ◑ partial · ○ todo · ⊘ blocked.  Spike refs: S0–S17, P0 tasks #19–22.

## A. Format core
| Feature | Status | Gate (pass condition) | Evidence |
|---|:--:|---|---|
| Manifest spine (build/seal) | ✓ | seals, Merkle root set, blocks==refs | 22 tests green |
| Identity `id` (stable) | ✓ | same inputs→same id; rename≠new-content | `id_stable_and_distinct` |
| `id` vs `content_hash` + `id_inputs` | ✓ | id=blake3(JCS(id_inputs)) logical; content_hash=Merkle; reconciled | **ADR-0020**, `id_*` tests |
| Canonical manifest encoding (JCS) | ✓ | re-serialize → identical hash (RFC 8785, `serde_jcs`) | `canonical::*`, `seal_round_trips_*` |
| `manifest_hash` seal (whole-manifest tamper-evident) | ✓ | blake3 over JCS(manifest); covers meta+sources+digests | `tampering_*`, `verify()` |
| Block dispatch (schema→array/table) | ✓ | array→Zarr v3+pcodec · table→Vortex — both real, bit-exact, deterministic | `tessera-io::{array,table}`, ADR-0023/0024 |
| Seal = hash-of-hashes | ✓ | µs seal, no 2nd data pass, valid partial root | design+tests |
| Error taxonomy | ✓ | typed `#[non_exhaustive]`, `Integrity{what,exp,act}`, never panic | `error.rs`, `verify()` |

## B. Codec & storage (decided, proven)
| Feature | Status | Gate | Evidence |
|---|:--:|---|---|
| Volume codec = **pcodec** | ✓ | smallest lossless; ≤ best competitor | CT 74.3 vs zstd 94.5 (−21%), PET −33% |
| Table backend = **Vortex** | ✓ | smallest + fastest random-take + pushdown; real Rust codec | S0/S4/S7/S10/S11 + `tessera-io::table` |
| Cubic 64³ chunking | ✓ | isotropic reads; size/read sweet spot | chunk sweep |
| zstd fallback codec | ✓ | decades-stable alt at ≤+27% size | codec sweep |

## C. Correctness gates (REQUIRED — binary)
| Gate | Status | Pass condition | Evidence |
|---|:--:|---|---|
| **Bit-exact lossless** (arrays+tables) | ✓ | `bytes ==` incl NaN/±inf/−0.0/denormal/int-limits | **S13** + Rust `array::tests` (pcodec) + `table::tests` (Vortex, 10 dtypes) |
| **Writer determinism** (same-ver) | ✓ | same input→byte-identical output (manifest + .tsra) | **S15** + `corpus_packs_deterministically` |
| Cross-version / cross-arch determinism | ◑ | golden hashes locked in `corpus.json`; drift fails CI | conformance gate (multi-release CI pending, S15 remain) |
| Pruning never lies | ○ | predicate-match chunk never skipped | TEST-PLAN |

## D. Performance SLA gates (regression floors — benched, 88-core box)
Rust benches: `cargo bench -p tessera-io` (`benches/codec.rs`). Wall-clock floors are machine-dependent
(measured below, **not** CI-gated); the machine-independent compression-ratio floor **is** gated
(`array::tests::pcodec_compresses_smooth_int16_volume`). Python-spike numbers retained where the bench
needs real CT/DICOM. Cross-substrate comparison vs the bare backends (`.tsra` overhead, size ladder,
ROI/slice speedups): `cargo run -p tessera-io --example bench_compare --release` → SPIKE-RESULTS.md #143.

**`.tsra` container tax vs the bare codec** (same 128³ int16, 8× 64³ chunks, same run — `tsra_vs_bare`):
size = bare codec bytes + ~320–600 B zip + ~1 KB FAIR manifest (≪1% at scale); **write +0.7%** (zip
STORE + blake3 seal); **read +11%** (the seal + per-block digest *verification* vanilla skips — the
integrity it buys); **3-D ROI 32³-of-128³ ≈ 2.3× faster than full decode** (cubic chunks; only the
intersecting chunk is read). A timed cross-*ecosystem* run (vs zarr-python/vortex-python on DUPLET
data) is the remaining dedicated harness (#143).

| Metric | Floor (don't regress) | Measured |
|---|---|---|
| Volume size (CT) | ≤ 0.80× zstd · ≤ 0.40× DICOM | 74.3 MB (0.79× / 0.29×) — spike |
| Orthogonal/ROI read (cube 64³) | ≤ 0.05 s coronal · ≤ 0.02 s ROI | 0.039 / 0.008 s — spike |
| Full-volume load vs DICOM | ≥ 4× faster | 5.2× — spike |
| Table random take ×500 (Vortex) | ≤ 30 ms | 23 ms — spike |
| Table projection (Vortex) | ≤ 15 ms | 7.6 ms — spike |
| Encode (pcodec, /core) | ≥ 60 MB/s | **Rust 113 MiB/s** (int16 64³, gated bench) |
| Decode (pcodec, /core) | — | **Rust 1.02 GiB/s** |
| Seal/hash (blake3) | ≥ 4 GB/s | **Rust 6.0 GiB/s** (8 MiB) |
| Table encode/decode (Vortex) | — | **Rust 214 MiB/s / 4.4 GiB/s** (100k×3 cols) |
| pcodec ratio, smooth int16 | > 4× (gated) | **107×** (gradient); 2.0× table |
| DuckDB-over-Vortex query | works, zero-copy | S7 ✓ (0.035 s) — spike |

## E. Durability & write engine
| Feature | Status | Gate | Evidence |
|---|:--:|---|---|
| No-encode-on-hot-path rule | ✓ | never stream DAQ into a single sealed file | S17 (footer=total-loss) |
| Fragment-append + atomic commit | ✓ | crash-tolerant to last committed fragment | `WriteSession` (fsync fragment→journal commit), `write::tests` |
| Hash-on-write incremental Merkle | ◑ | valid root at every commit watermark | `hash::MerkleAccumulator` (root==batch at each watermark); engine wiring pending |
| Crash recovery (replay to watermark) | ✓ | resume from registry C; ignore >C | `WriteSession::recover` (drops torn tail), `write::tests` |
| Unified Source/WriteSession surface | ◑ | push/from/seal/recover; schema-dispatch | `WriteSession` create/append/recover/seal; bounded-ring + rayon pool pending |

## F. Integrity, provenance & FAIR
| Feature | Status | Gate | Evidence |
|---|:--:|---|---|
| blake3 Merkle integrity | ✓ | any byte change → root change; tamper-evident | `tamper_*` tests |
| `sources[]` DAG (typed roles) | ✓ | typed role + content_hash on each edge; resolve + walk | `provenance::{Source,Resolver,verify_chain}` |
| Source-rooted chain verify | ✓ | walk DAG, each edge's hash == parent seal; cycles rejected | `provenance::verify_chain`, 2 tests |
| Source-rooted **signing** | ○ | sign-at-source (cosign); needs signing keys | S16 (external key → P6) |
| WORM (Object-Lock) | ○ | overwrite/delete refused in retention | S16 |
| Units / descriptions / `_vocabulary`·`_code`·`default`·`extra/`·`study`·axes | ✓ | FAIR I1/I2 + AI-readable; fail-strict on missing required | `schema::FieldSpec`/`Coded`, `metadata`/`extra`/`study`, ArraySpec axes/unit/fill |
| Versioned product-schema registry (embedded, 9 schemas) | ✓ | additive evolution, stable ids, offline-valid, domain-agnostic | `SchemaRegistry::builtin`, `validate` tests |
| RO-Crate / DataCite export | ◑ | RO-Crate 1.1 JSON-LD + DataCite `dois` from the manifest; CLI `tessera export ro-crate\|datacite` | `export::{ro_crate,datacite}`, 4 tests (InvenioRDM/validation pending) |

## G. Layout, distribution, ingest, read
| Feature | Status | Gate | Evidence |
|---|:--:|---|---|
| Sealed `.tsra` (zip64, range-readable) | ✓ | STORED zip64, mimetype-first magic, central-dir index; 1-block read ≪ whole archive | `container::pack`, `range::CountingReader` (S6 proof) |
| OCI artifact / exploded prefix | ○ | push/pull; range-read; CoW versioning | #22 (P6) |
| Ingest: DICOM | ◑ | lossless int16 + rescale/units/modality + provenance + 3-D series-stack + PS3.15 de-id; CLI `tessera ingest dicom [--deidentify]`; golden corpus + JPEG transfer-syntaxes pending | `tessera-ingest::dicom`, ADR-0025 |
| Ingest: GE-HDF5 (listmode) | ◑ | lossless 2p+3p compound→columnar transpose (row-major #193 fix) + `ingested_from` provenance; CLI `tessera ingest ge-hdf5 --dataset`; chunked-stream for 7 GB files pending | `tessera-ingest::ge_hdf5` (#208) |
| Ingest: Siemens · raw · NIfTI | ○ | decode→re-encode open; lossless | S9 |
| Reader API (open/verify/block read) | ✓ | magic+seal verify on open; per-block read verified vs digest; partial-product; generic Read+Seek | `tessera-io::Reader`, container tests |
| Conformance corpus + SPEC.md | ✓ | 6 golden fixtures + `.tsra` test vectors locked in CI; `docs/SPEC.md` | `corpus/corpus.json`, `corpus/files/`, `tests/conformance.rs` |
| **Independent reader passes corpus** (v1.0 gate) | ✓ | a 2nd impl, from SPEC.md alone, reproduces all 6 goldens | `corpus/reference_reader/` (pure-Python, 6/6 first try) |
| Bindings (pyo3 → C-ABI → WASM) | ◑ | Python read+verify+decode+**write** shipped (abi3 `import tessera`: `Reader` open/manifest/blocks/read_block/read_array→numpy/read_table→numpy/verify + `Builder` add_array/add_table/set_field/add_source/pack + typed `TesseraError`); hermetic check does a full numpy write→read→verify round-trip over the corpus. Arrow zero-copy + WASM pending | `tessera-py` (#210) |

## H. Release gates (definition of shippable)
**Shippable = all four green on the supported matrix: ① conformance corpus · ② bit-exact roundtrip · ③ perf-SLA (§D) · ④ writer-determinism.**

| Milestone | Done-when |
|---|---|
| **v0.1 — format frozen** | core + io (write+read), conformance corpus, §C+§D gates green, zip layout, CLI (pack/unpack/verify/inspect/schema/ingest) |
| **v0.2 — DICOM ingest** | `tessera-ingest::dicom` lossless + PS3.15 verify + golden DICOM corpus |
| **v0.3 — vendor raw + integrity** | GE-HDF5/Siemens/raw plugins; minimal cosign signing; WORM on MinIO |
| **v0.5 — Python + ops** | pyo3 parity; reference podman-compose stack; perf-SLA CI gates; migration CLI |
| **v1.0 — spec stabilized** | 2nd independent reader passes conformance; spec frozen; 12 mo zero-breaking |

## How to use as the baseline
- **Regression:** re-run the benches; any §D row below its floor = fail the build.
- **Correctness:** §C is binary and required every release (S13/S15 harness).
- **Progress:** flip ○→◑→✓ as gates pass; the matrix is the single source of "where are we."
- **Conformance:** §G corpus is the cross-impl + cross-version gate (catches the S15 archival risk).
