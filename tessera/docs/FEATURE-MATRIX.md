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
| `id` vs `content_hash` + `id_inputs` | ✓ | id=blake3(JCS(id_inputs)) logical; **content_hash = recursive MMR root** (domain-separated leaves/nodes; supersedes the flat root, ADR-0028 §1–2; cross-validated by the SPEC-only Python reader); reconciled | **ADR-0020**+**0028**, `id_*`, `hash::tests::{root_is_a_recursive_tree_not_a_flat_concat,watermark_root_is_consistent_at_each_step}` |
| Spatial referencing (`world_frame`: affine + LPS + named space) | ◑ | optional voxel→world 3×4 affine on `ArraySpec` (§1); spacing **derived** from columns (§2); per-level transform **derivation** `at_level` + **OME-Zarr `multiscales` export** `ome_zarr_multiscales` (§3 — v0.4 datasets/coordinateTransformations, orientation-lossy per the OME-Zarr schema); rigid registration = `transform` product + new frame + provenance edge (§5; the `deformation_field` schema is registered — **deformable-warp encode pipeline still pending**); LPS canonical (§6); additive | **ADR-0030** `world_frame_spacing_is_derived_from_affine_columns`, `array_spec_world_frame_is_additive_and_optional`, `world_frame_at_level_derives_per_level_transform`, `ome_zarr_multiscales_export_derives_per_level_transforms`, `schema::tests::registration_is_a_transform_product_with_new_frame_and_provenance` |
| Referenced coordinates & quantities (one `(transform,unit,frame)` descriptor) | ✓ | unified `Referenced` + closed `Transform` taxonomy {identity, affine_1d, affine_nd, lookup}; bridges from rescale (`value_referencing`) + `world_frame` (`spatial_referencing`); time/tick instances (`time_regular`/`time_irregular`/`time_ticks`); §3 `_vocabulary` escape; per-axis `axis_referencing` reachable through `manifest_hash`; pinned `CANONICAL_FRAMES`+`CANONICAL_UNITS` (frame `:<detail>` suffixes); wall-clock-vs-elapsed split; PET decay-correction ref; SPEC §5c (**ADR-0032 Accepted, as-built — 4 fresh-context re-audits**) | **ADR-0032** `referencing::tests::*` (taxonomy/bridges/time/tick/vocab/frame+unit-pinning/time-split), `value_and_spatial_referencing_derive_the_unified_descriptor`, `per_axis_referencing_is_additive_optional_and_reachable`, `axis_referencing_survives_manifest_seal_and_is_in_manifest_hash`, `dynamic_pet_carries_optional_decay_correction_reference` |
| Canonical manifest encoding (JCS) | ✓ | re-serialize → identical hash (RFC 8785, `serde_jcs`) | `canonical::*`, `seal_round_trips_*` |
| `manifest_hash` seal (whole-manifest tamper-evident) | ✓ | blake3 over JCS(manifest); covers meta+sources+digests | `tampering_*`, `verify()` |
| Block dispatch (schema→array/table) | ✓ | array→Zarr v3+pcodec · table→Vortex — both real, bit-exact, deterministic | `tessera-io::{array,table}`, ADR-0023/0024 |
| Seal = hash-of-hashes | ✓ | µs seal, no 2nd data pass, valid partial root | design+tests |
| Error taxonomy | ✓ | typed `#[non_exhaustive]`, `Integrity{what,exp,act}`, never panic | `error.rs`, `verify()` |

## B. Codec & storage (decided, proven)
| Feature | Status | Gate | Evidence |
|---|:--:|---|---|
| Volume codec = **pcodec** (default) | ✓ | smallest lossless on real CT/PET; ≤ best competitor | CT 74.3 vs zstd 94.5 (−21%), PET −33% |
| Table backend = **Vortex** | ✓ | smallest + fastest random-take + pushdown; real Rust codec | S0/S4/S7/S10/S11 + `tessera-io::table`, **ADR-0024** |
| Cubic 64³ chunking | ✓ | isotropic reads; size/read sweet spot — codec-independent | chunk sweep |
| Per-block selectable codec (`ArraySpec.codec`) | ✓ | `"pcodec"` (default) · `"zstd"` (fixed level 3) · `"auto"` (writer picks smaller, records concrete codec) — slice/ROI access unchanged across codecs (chunk grid owns locality) | #213 + `tessera-io::array::tests::{zstd_*,auto_*}` |
| Table row-groups (fixed 2¹⁶) — one encoder, batch == stream | ✓ | always-chunked Vortex; `encode_streaming` (lazy/bounded) byte-identical to `encode`; ≤2¹⁶-row tables unchanged (backward-compat) | `tessera-io::table::{encode,encode_streaming}` (#203, ADR-0026), `multi_rowgroup_*`, `encode_streaming_matches_batch_encode` |
| Sparse representation (substrate by nature) | ✓ | scatter-sparse → **COO table** `(idx,v)` via the Vortex encoder (no new primitive); block-sparse → dense+pcodec with `count=0` chunk-prune; crossover **measured** (#221-A: dense+pcodec wins on disk at storable scales → COO only for unstorable-ambient / selective-nnz) | **ADR-0031**; `array::to_coo` (`to_coo_emits_one_row_per_nonzero`), `chunk_index::prune`, #221-A (SPIKE-RESULTS) |

## C. Correctness gates (REQUIRED — binary)
| Gate | Status | Pass condition | Evidence |
|---|:--:|---|---|
| **Bit-exact lossless** (arrays+tables) | ✓ | `bytes ==` incl NaN/±inf/−0.0/denormal/int-limits | **S13** + Rust `array::tests` (pcodec) + `table::tests` (Vortex, 10 dtypes) |
| **Writer determinism** (same-ver) | ✓ | same input→byte-identical output (manifest + .tsra) | **S15** + `corpus_packs_deterministically` |
| Cross-version / cross-arch determinism | ◑ | golden hashes locked in `corpus.json`; drift fails CI | conformance gate (multi-release CI pending, S15 remain) |
| Pruning never lies | ✓ | conservative min/max overlap → a chunk that *could* match a range is never skipped (no false negatives); proven exhaustively | `chunk_index::tests::pruning_keeps_overlapping_chunks_only_and_never_drops_a_hit`, `array_chunk_index_*` |
| Docs-as-tests (3 layers gated) | ✓ | every public-API example + CLI transcript + book runs in CI, so docs can't drift from behaviour | `workspace-doctest` (4 doctests) · `tessera-cli/tests/cli.rs` trycmd (5 cases incl. real corpus inspect) · `mdbook-build` (book `{{#include}}`s the trycmd files) |

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
| Hash-on-write incremental Merkle | ✓ | valid root at every commit watermark, wired into the streaming path | `hash::MerkleAccumulator` folded per `append_block`; `StreamWriter` commits in push order → root==batch (#203) |
| Crash recovery (replay to watermark) | ✓ | resume from registry C; ignore >C | `WriteSession::recover` (drops torn tail), `write::tests` |
| Bounded-memory streaming write (parallel encode) | ✓ | producer decoupled from encode; RAM capped under burst; byte-identical to batch | `StreamWriter` (bounded `sync_channel` + N-thread encode pool + ordered committer); **6.2× vs synchronous**, cap=2 holds RAM flat (#203, `examples/stream_write`) |
| Unified Source/WriteSession surface | ✓ | push/from/seal/recover; one write path | `WriteSession` create/append/recover/seal + `StreamWriter` front |
| Streaming table accumulator (>RAM, bounded) | ✓ | push arbitrary batches → fixed 2¹⁶ fragments → lazy compact == batch; RAM ~2 row-groups | `TableStreamWriter` (#203, ADR-0026), `accumulator_equals_batch_over_odd_batches` |
| Metadata-first durable header | ◑ | `header.json` (product/name/metadata/study/extra) written at `create`, persisted on set, replayed by `recover` — before any data block | `WriteSession` (header.json); **gaps:** header not fsync'd, no `StreamWriter.with_field` passthrough |
| Sub-block Merkle + chunk-index (`{hash, stats}`, pruning, sub-block MMR root, additive block) | ✓ | `chunk_index` = monoid stats (count/min/max/sum) + `prune` + sub-block MMR root; wired into both encoders (`table_chunk_index`/`array_chunk_index`); emitted as the additive `BlockKind::ChunkIndex` companion block (digest rolls into `content_hash`, `verify()` passes); #221-B measured leaf granularity (knee ≈2¹⁴) | **ADR-0028 §3** (#214 superseded into it); `tessera_core::chunk_index::tests::*`, `table_chunk_index_groups_stats_and_prunes`, `array_chunk_index_*`, `tessera_io::chunk_index::tests::*` |

## F. Integrity, provenance & FAIR
| Feature | Status | Gate | Evidence |
|---|:--:|---|---|
| blake3 Merkle integrity | ✓ | any byte change → root change; tamper-evident | `tamper_*` tests |
| Inclusion proofs (per-block + per-chunk confirmation) | ✓ | audit path from a block/chunk leaf up to `content_hash`; confirm one item against the seal without re-reading; forgery/tamper fail | **ADR-0028 §6**; `hash::inclusion_proof`/`verify_inclusion` (`inclusion_proofs_verify_for_every_leaf`, `tampering_with_a_proof_step_fails_verification`), `chunk_index::tests::each_chunk_has_an_inclusion_proof_under_the_root` |
| Multiscale overview (aggregate stat-pyramid) | ✓ | coarse query at level *L* without touching data; summit == block aggregate | **ADR-0028 §3/§7**; `chunk_index::tests::stat_pyramid_rolls_up_to_the_aggregate` |
| Array multiscale pyramid + MIP projection (one fold abstraction) | ✓ | 2×-max-downsample pyramid (per-level `WorldFrame::at_level` geometry) + `max` projection (fold one axis → 2-D MIP, the true level-0); bit-exact vs whole-array refs; the same fold builds pyramid + projection (`(monoid, axes, depth)`) | **ADR-0028 §7 / Arrays-fold §4**; `tessera_io::array::{array_pyramid, downsample_max_3d, project_max_3d}` (`array_pyramid_halves_until_a_single_summit_voxel`, `downsample_max_3d_halves_and_maxes`, `project_max_3d_folds_one_axis_bit_exact`) |
| Fused `{hash, stats}` streaming fold (core) | ◑ | one carry folds the MMR integrity hash **and** the rolled-up monoid stats up the tree per appended leaf → live root + live aggregate; streamed == batch at **every** prefix length; block-level live root already wired (`running_root`). **Pending:** wire into the tessera-io ordered committer + persist interior nodes as sidecars | **ADR-0028 §5**; `tessera_core::chunk_index::MerkleStatsAccumulator` (`fused_streaming_fold_matches_batch_for_root_and_aggregate`), `write::running_root_advances_and_equals_sealed_content_hash` |
| `sources[]` DAG (typed roles) | ✓ | typed role + content_hash on each edge; resolve + walk | `provenance::{Source,Resolver,verify_chain}` |
| Source-rooted chain verify | ✓ | walk DAG, each edge's hash == parent seal; cycles rejected | `provenance::verify_chain`, 2 tests |
| Source-rooted **signing** | ○ | sign-at-source (cosign); needs signing keys | S16 (external key → P6) |
| WORM (Object-Lock) | ○ | overwrite/delete refused in retention | S16 |
| Units / descriptions / `_vocabulary`·`_code`·`default`·`extra/`·`study`·axes | ✓ | FAIR I1/I2 + AI-readable; fail-strict on missing required | `schema::FieldSpec`/`Coded`, `metadata`/`extra`/`study`, ArraySpec axes/unit/fill |
| Versioned product-schema registry (embedded, 13 schemas) | ✓ | additive evolution, stable ids, offline-valid, domain-agnostic; +ADR-0029 §5 multi-dim set (`dynamic_pet`/`diffusion_mri`/`multicontrast_mri`) | `SchemaRegistry::builtin`, `registry_has_all_builtins`, `dynamic_pet_requires_volume_and_frame_timing` |
| RO-Crate / DataCite export | ◑ | RO-Crate 1.1 JSON-LD + DataCite `dois` from the manifest; CLI `tessera export ro-crate\|datacite` | `export::{ro_crate,datacite}`, 4 tests (InvenioRDM/validation pending) |

## G. Layout, distribution, ingest, read
| Feature | Status | Gate | Evidence |
|---|:--:|---|---|
| Sealed `.tsra` (zip64, range-readable) | ✓ | STORED zip64, mimetype-first magic, central-dir index; 1-block read ≪ whole archive | `container::pack`, `range::CountingReader` (S6 proof), **ADR-0022** |
| OCI artifact / exploded prefix | ○ | push/pull; range-read; CoW versioning | #22 (P6) |
| Ingest: DICOM | ◑ | lossless int16 + rescale/units/modality + provenance + 3-D series-stack + PS3.15 de-id; CLI `tessera ingest dicom [--deidentify]`; golden corpus + JPEG transfer-syntaxes pending | `tessera-ingest::dicom`, ADR-0025 |
| Ingest: GE-HDF5 (listmode) | ◑ | lossless 2p+3p compound→columnar transpose (row-major #193 fix) + `ingested_from` provenance; CLI `tessera ingest ge-hdf5 --dataset`; chunked-stream for 7 GB files pending | `tessera-ingest::ge_hdf5` (#208) |
| Ingest: Siemens · raw · NIfTI | ○ | decode→re-encode open; lossless | S9 |
| Reader API (open/verify/block read) | ✓ | magic+seal verify on open; per-block read verified vs digest; partial-product; generic Read+Seek | `tessera-io::Reader`, container tests |
| Conformance corpus + SPEC.md | ✓ | 6 golden fixtures + `.tsra` test vectors locked in CI; `docs/SPEC.md` | `corpus/corpus.json`, `corpus/files/`, `tests/conformance.rs` |
| **Independent reader passes corpus** (v1.0 gate) | ✓ | a 2nd impl, from SPEC.md alone, reproduces all 6 goldens | `corpus/reference_reader/` (pure-Python, 6/6 first try) |
| Bindings (pyo3 → C-ABI → WASM) | ◑ | Python read+verify+decode+**write** shipped (abi3 `import tessera`: `Reader` open/manifest/blocks/read_block/read_array→numpy/read_array_subset (ROI)/read_table→numpy/verify + `Builder` add_array/add_table/set_field/add_source/pack + typed `TesseraError`); read_table_column (Vortex column projection, #212 — fastest column read in the #143 bench); hermetic check does a full numpy write→read→verify round-trip over the corpus. Arrow zero-copy + WASM pending | `tessera-py` (#210) |

## H. Release gates (definition of shippable)
**Shippable = all four green on the supported matrix: ① conformance corpus · ② bit-exact roundtrip · ③ perf-SLA (§D) · ④ writer-determinism.**

| Milestone | Done-when |
|---|---|
| **v0.1 — format frozen** | core + io (write+read), conformance corpus, §C+§D gates green, zip layout, CLI (pack/unpack/verify/inspect/schema/ingest) |
| **v0.2 — DICOM ingest** | `tessera-ingest::dicom` lossless + PS3.15 verify + golden DICOM corpus |
| **v0.3 — vendor raw + integrity** | GE-HDF5/Siemens/raw plugins; minimal cosign signing; WORM on MinIO |
| **v0.5 — Python + ops** | pyo3 parity; reference podman-compose stack; perf-SLA CI gates; migration CLI |
| **v1.0 — spec stabilized** ○ | 2nd independent reader passes conformance ✓ (#211); 4 gates green ✓ — but the **format is still maturing** (chunked tables, streaming, sub-block Merkle), so spec is NOT yet frozen. SPEC self-designates **"v0, pre-1.0"**; the premature `tessera-1.0` tag was **dropped** → versioning on the **v0.2** line. v1.0 = spec frozen + 12-mo zero-breaking, not yet. |

## How to use as the baseline
- **Regression:** re-run the benches; any §D row below its floor = fail the build.
- **Correctness:** §C is binary and required every release (S13/S15 harness).
- **Progress:** flip ○→◑→✓ as gates pass; the matrix is the single source of "where are we."
- **Conformance:** §G corpus is the cross-impl + cross-version gate (catches the S15 archival risk).
