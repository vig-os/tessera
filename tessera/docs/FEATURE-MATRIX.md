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
| Block dispatch (schema→array/table) | ○ | tensor col→Zarr; scalar cols→Vortex | #19/#20 |
| Seal = hash-of-hashes | ✓ | µs seal, no 2nd data pass, valid partial root | design+tests |
| Error taxonomy | ✓ | typed `#[non_exhaustive]`, `Integrity{what,exp,act}`, never panic | `error.rs`, `verify()` |

## B. Codec & storage (decided, proven)
| Feature | Status | Gate | Evidence |
|---|:--:|---|---|
| Volume codec = **pcodec** | ✓ | smallest lossless; ≤ best competitor | CT 74.3 vs zstd 94.5 (−21%), PET −33% |
| Table backend = **Vortex** | ✓ | smallest + fastest random-take + pushdown | S0/S4/S7/S10/S11 |
| Cubic 64³ chunking | ✓ | isotropic reads; size/read sweet spot | chunk sweep |
| zstd fallback codec | ✓ | decades-stable alt at ≤+27% size | codec sweep |

## C. Correctness gates (REQUIRED — binary)
| Gate | Status | Pass condition | Evidence |
|---|:--:|---|---|
| **Bit-exact lossless** (arrays+tables) | ✓ | `bytes ==` incl NaN/±inf/−0.0/denormal/int-limits | **S13 PASS** |
| **Writer determinism** (same-ver) | ✓ | same input→byte-identical output | **S15 PASS** |
| Cross-version / cross-arch determinism | ○ | byte-identical across releases/arch | hedge: pin+vendor (S15 remain) |
| Pruning never lies | ○ | predicate-match chunk never skipped | TEST-PLAN |

## D. Performance SLA gates (regression floors — benched, 88-core box)
| Metric | Floor (don't regress) | Measured |
|---|---|---|
| Volume size (CT) | ≤ 0.80× zstd · ≤ 0.40× DICOM | 74.3 MB (0.79× / 0.29×) |
| Orthogonal/ROI read (cube 64³) | ≤ 0.05 s coronal · ≤ 0.02 s ROI | 0.039 / 0.008 s |
| Full-volume load vs DICOM | ≥ 4× faster | 5.2× |
| Table random take ×500 (Vortex) | ≤ 30 ms | 23 ms |
| Table projection (Vortex) | ≤ 15 ms | 7.6 ms |
| Encode (pcodec, /core) | ≥ 60 MB/s | 71 MB/s |
| Encode (parallel) | ≥ 3 GB/s | 4.3 GB/s |
| Seal/hash (blake3) | ≥ 4 GB/s | 4.1 GB/s |
| DuckDB-over-Vortex query | works, zero-copy | S7 ✓ (0.035 s) |

## E. Durability & write engine
| Feature | Status | Gate | Evidence |
|---|:--:|---|---|
| No-encode-on-hot-path rule | ✓ | never stream DAQ into a single sealed file | S17 (footer=total-loss) |
| Fragment-append + atomic commit | ○ | crash-tolerant to last committed fragment | S17 |
| Hash-on-write incremental Merkle | ○ | valid root at every commit watermark | S17 |
| Crash recovery (replay to watermark) | ○ | resume from registry C; ignore >C | S17 |
| Unified Source/WriteSession surface | ○ | push/from/seal/recover; schema-dispatch | S17 |

## F. Integrity, provenance & FAIR
| Feature | Status | Gate | Evidence |
|---|:--:|---|---|
| blake3 Merkle integrity | ✓ | any byte change → root change; tamper-evident | `tamper_*` tests |
| `sources[]` DAG (typed roles) | ◑ | typed role + content_hash on each edge | `provenance::Source` (resolve() pending) |
| Source-rooted signing + chain verify | ○ | sign-at-source; walk DAG to scanner-signed root | S16 |
| WORM (Object-Lock) | ○ | overwrite/delete refused in retention | S16 |
| Units / descriptions / `_vocabulary`·`_code`·`default`·`extra/`·`study`·axes | ✓ | FAIR I1/I2 + AI-readable; fail-strict on missing required | `schema::FieldSpec`/`Coded`, `metadata`/`extra`/`study`, ArraySpec axes/unit/fill |
| Versioned product-schema registry (embedded, 9 schemas) | ✓ | additive evolution, stable ids, offline-valid, domain-agnostic | `SchemaRegistry::builtin`, `validate` tests |
| RO-Crate / DataCite export | ○ | conformant JSON-LD / DataCite | #19 |

## G. Layout, distribution, ingest, read
| Feature | Status | Gate | Evidence |
|---|:--:|---|---|
| Sealed `.tsra` (zip64, range-readable) | ○ | manifest discoverable; cloud range-read | #22 |
| OCI artifact / exploded prefix | ○ | push/pull; range-read; CoW versioning | #22 |
| Ingest: DICOM | ○ | lossless tags, PS3.15 verify, rescale/units | S9 |
| Ingest: GE-HDF5 · Siemens · raw · NIfTI | ○ | decode→re-encode open; lossless | S9 (GE transform benched) |
| Reader API (open/range/block-handle) | ○ | partial-product semantics; range backend | #21 |
| Conformance corpus + SPEC.md | ○ | golden roots; CI gate; 2nd-impl passes | #21 |
| Bindings (pyo3 → C-ABI → WASM) | ○ | Python parity; zero-copy Arrow | — |

## H. Release gates (definition of shippable)
**Shippable = all four green on the supported matrix: ① conformance corpus · ② bit-exact roundtrip · ③ perf-SLA (§D) · ④ writer-determinism.**

| Milestone | Done-when |
|---|---|
| **v0.1 — format frozen** | core + io (write+read), conformance corpus, §C+§D gates green, zip layout, CLI (pack/unpack/verify/inspect) |
| **v0.2 — DICOM ingest** | `tessera-ingest::dicom` lossless + PS3.15 verify + golden DICOM corpus |
| **v0.3 — vendor raw + integrity** | GE-HDF5/Siemens/raw plugins; minimal cosign signing; WORM on MinIO |
| **v0.5 — Python + ops** | pyo3 parity; reference podman-compose stack; perf-SLA CI gates; migration CLI |
| **v1.0 — spec stabilized** | 2nd independent reader passes conformance; spec frozen; 12 mo zero-breaking |

## How to use as the baseline
- **Regression:** re-run the benches; any §D row below its floor = fail the build.
- **Correctness:** §C is binary and required every release (S13/S15 harness).
- **Progress:** flip ○→◑→✓ as gates pass; the matrix is the single source of "where are we."
- **Conformance:** §G corpus is the cross-impl + cross-version gate (catches the S15 archival risk).
