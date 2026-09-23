# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-alpha.1](https://github.com/vig-os/tessera/releases/tag/tessera-io-v0.1.0-alpha.1) - 2026-09-23

### Added

- *(io)* close the array dtype envelope — i1/u1/b1/f2 via checked transparent widening ([#418](https://github.com/vig-os/tessera/pull/418)) ([#420](https://github.com/vig-os/tessera/pull/420))
- *(io)* nullable table columns — a validity mask, not a sentinel ([#360](https://github.com/vig-os/tessera/pull/360))
- *(table)* Bool + Utf8 ColumnData — first-class boolean & string columns ([#354](https://github.com/vig-os/tessera/pull/354))
- *(cli)* --verify deep opt-in on tree/inspect; honest seal-vs-verified badge (#268 part 1) ([#341](https://github.com/vig-os/tessera/pull/341))
- *(core)* table Column carries unit/description/short_name/scale ([#307](https://github.com/vig-os/tessera/pull/307)) ([#312](https://github.com/vig-os/tessera/pull/312))
- *(core)* table Column carries unit/description/short_name/scale ([#307](https://github.com/vig-os/tessera/pull/307))
- *(format)* self-contained .tsra — embed signature + ingest provenance as non-sealed aux/ members ([#259](https://github.com/vig-os/tessera/pull/259))
- *(blob)* cloud-aware extract + verify-while-pack + opt-in parallel blake3 ([#234](https://github.com/vig-os/tessera/pull/234))
- *(blob)* bounded-memory streaming ingest + extract (closes #231) ([#232](https://github.com/vig-os/tessera/pull/232))
- opaque blob/"junk" block — bit-faithful preservation of un-parsed vendor raw ([#229](https://github.com/vig-os/tessera/pull/229)) ([#230](https://github.com/vig-os/tessera/pull/230))
- *(signing)* bind signed_at + key_format; ssh-ed25519 loader; usage tests-as-docs
- *(io,cli)* tessera push / pull — in-Rust OCI registry client
- *(io,cli)* forget + gc — reclaim space from deleted lineages (ADR-0036)
- *(io)* content-addressed repository — CoW versioning object store (ADR-0036)
- *(io,cli)* #225 cloud-read landing — public open_url + tail-prefetch + cohort prune-before-fetch (goal pt3)
- *(io,py)* cross-block read/query over multi-block tables — LogicalTableView (goal pt1)
- *(io,cli)* adaptive thread allocator — WriteConfig::balanced + tessera bench write --auto
- *(io,ingest)* multi-block listmode ingest — parallel encode + constant-memory >RAM (ADR-0026/0034 §3)
- *(io,cli)* WriteConfig (SSoT workers+ram_budget) + tessera bench write — size your system, ADR-0034-honest
- *(core,io)* #223 / ADR-0033 — content-addressed collection model + 3 projections
- *(io)* #225 spike — range-read a .tsra from object storage (MinIO/S3), prune-before-fetch
- *(io)* durable atomic header write — close metadata-first row gaps, flip ◑→✓
- *(io)* local-filesystem WORM enforcement (write_retention + worm_remove/overwrite)
- *(io)* encode-path tracing — raw→encoded→ratio→codec on every block (observability #26)
- *(io)* OCI artifact manifest for .tsra distribution ([#209](https://github.com/vig-os/tessera/pull/209))
- *(io)* WORM retention mechanism (compliance/governance hold + mutation guard)
- *(io)* sign/verify a .tsra container via a .sig.json sidecar (S16 end-to-end)
- *(io)* structured write-path tracing (SSoT observability on every committed block)
- ADR-0030 §5 — deformable-warp apply (deformation-field point resolve)
- *(io)* ADR-0028 §5 — wire the fused fold into TableStreamWriter (live index)
- *(io)* ADR-0028 §5 — wire fused sidecar emission into the StreamWriter committer
- *(io)* ADR-0028 §5 — table_block_with_index fused emit (table counterpart)
- *(io)* ADR-0028 §5 — array_block_with_index fused emit primitive
- *(io)* ADR-0028 — array MIP projection (the fold-one-axis case)
- *(io)* array_pyramid — full array multiscale pyramid (ADR-0028 §7)
- *(io)* downsample_max_3d — array multiscale pyramid level (ADR-0028 §7)
- *(io)* array::to_coo — concrete COO sparse encoding (ADR-0031 §2)
- *(io)* derived-sidecar tag on the chunk-index block (ADR-0028 §4)
- emit chunk-index as an additive companion manifest block (ADR-0028 §3)
- *(io)* array_chunk_index — n-D chunk-grid {hash,stats} index (ADR-0028 §3)
- *(io)* ArrayData::as_i64 — integer accessor for array chunk-stats (ADR-0028 §3)
- *(io)* wire chunk-index into the table encoder (ADR-0028 §3)
- *(io)* streaming table accumulator + matrix sync to v0.2 (#203, ADR-0026)
- *(io)* encode_streaming — lazy bounded-RAM table encode == batch (#203, ADR-0026)
- *(io)* always-chunked table encoder (fixed 2^16 row-groups) (#203, ADR-0026)
- *(io)* StreamWriter — bounded-memory parallel-encode streaming engine ([#203](https://github.com/vig-os/tessera/pull/203))
- *(io)* pluggable array codecs — pcodec (default) · zstd · auto ([#213](https://github.com/vig-os/tessera/pull/213))
- *(io,py)* table column projection — read one column, not the whole block ([#212](https://github.com/vig-os/tessera/pull/212))
- *(py)* write path — Builder packs .tsra from numpy ([#210](https://github.com/vig-os/tessera/pull/210))
- *(py)* decode array/table blocks to numpy ([#210](https://github.com/vig-os/tessera/pull/210))
- *(io)* perf-SLA Rust benches + machine-independent ratio gate (P4, #206)
- *(io)* range-read validation — CountingReader proves .tsra cloud-readability (P2/S6, #196)
- *(io)* streaming write engine — WriteSession fragment-append + crash-recovery (P3/S17, #203)
- *(io)* real Vortex table payloads in conformance; ALP-exclusion determinism (P3, #203)
- *(io)* real table backend — Vortex columnar encode/decode (P3, #203)
- *(io)* real array payloads in conformance corpus; pcodec default (P3, #203)
- *(io)* real array backend — zarrs+pcodec encode/decode (P3, #203)
- *(io)* P4 — conformance corpus + SPEC.md; deterministic .tsra ([#204](https://github.com/vig-os/tessera/pull/204))
- *(cli)* tessera CLI — pack/unpack/verify/inspect ([#205](https://github.com/vig-os/tessera/pull/205))
- *(io)* P2 — .tsra container reader/writer, read-path-first ([#202](https://github.com/vig-os/tessera/pull/202))

### Fixed

- *(table)* register Vortex Pco for float columns — f64 tables were stored raw ([#380](https://github.com/vig-os/tessera/pull/380)) ([#384](https://github.com/vig-os/tessera/pull/384))
- *(io)* let b1/str columns through the streaming write path ([#359](https://github.com/vig-os/tessera/pull/359))
- *(signing)* sign the envelope + carry the sidecar through push/pull; ADR-0037
- *(io)* downsample derives the level world_frame (ADR-0028 §7 ∘ ADR-0030 §3)
- *(io)* to_coo honours explicit fill_value (ADR-0031 §4 — audit finding)

### Other

- *(table)* grid-parallel numeric decode — uncap the column-count fan-out ([#352](https://github.com/vig-os/tessera/pull/352)) ([#385](https://github.com/vig-os/tessera/pull/385))
- *(verify)* fan the L2 payload probe across the worker pool ([#371](https://github.com/vig-os/tessera/pull/371))
- *(integrity)* pin everyday tamper/trust cases + the L1/L2 verify ladder ([#369](https://github.com/vig-os/tessera/pull/369))
- *(io)* random take reads only the selected rows (Vortex row-index pushdown) ([#361](https://github.com/vig-os/tessera/pull/361))
- *(table)* make reads fast — projection, pooled runtime, column-parallel materialise (3.2x) ([#355](https://github.com/vig-os/tessera/pull/355))
- *(io)* parallel blob hash via blake3 update_mmap_rayon (one native call) ([#237](https://github.com/vig-os/tessera/pull/237))
- *(io)* streaming-write throughput harness (size your DAQ ingest rate)
- *(io)* runnable doctest for warp_world (ADR-0030 §5 deformable warp apply)
- *(io)* gated doctest for to_coo (docs-as-tests coverage)
- *(io)* gated doctest for downsample_max_3d (docs-as-tests coverage)
- *(io)* gated doctest for table_chunk_index (docs-as-tests coverage)
- *(io)* gated doctest for array_chunk_index (docs-as-tests coverage)
- *(io)* array_chunk_index edge-chunk regression + close review gap
- *(#221-B)* chunk-index leaf granularity measured — overhead vs pruning
- *(#221-A)* sparse dense-vs-COO crossover — measured; ADR-0031 §5 reframed
- *(io)* cross-substrate comparison vs bare Zarr/Vortex ([#143](https://github.com/vig-os/tessera/pull/143))
- *(io)* .tsra-vs-bare-codec comparison — container tax + 3-D ROI (#206, #143)
- *(io)* commit concrete .tsra conformance test vectors (P8 prep, #211)
