# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-alpha.1](https://github.com/vig-os/tessera/releases/tag/tessera-ingest-v0.1.0-alpha.1) - 2026-09-23

### Added

- *(ingest)* offline crypto-shred de-identification for DICOM (ADR-0047, #269) ([#279](https://github.com/vig-os/tessera/pull/279))
- *(ingest)* blob-series — multi-file series as one .tsra, a block per file (no tar) ([#328](https://github.com/vig-os/tessera/pull/328))
- *(ingest)* populate DICOM series world_frame + surface rescale ([#271](https://github.com/vig-os/tessera/pull/271)) ([#280](https://github.com/vig-os/tessera/pull/280))
- *(core+ingest)* land ADR-0058 generation-provenance on dev (supersedes #346) ([#415](https://github.com/vig-os/tessera/pull/415))
- *(dist)* static-HDF5 self-contained builds + cargo-dist broad channel ([#374](https://github.com/vig-os/tessera/pull/374)) ([#375](https://github.com/vig-os/tessera/pull/375))
- *(table)* Bool + Utf8 ColumnData — first-class boolean & string columns ([#354](https://github.com/vig-os/tessera/pull/354))
- *(ingest)* actionable non-image DICOM handling — route raw objects to blob ([#301](https://github.com/vig-os/tessera/pull/301)) ([#319](https://github.com/vig-os/tessera/pull/319))
- *(ingest)* dicom-series --rescale-mode global-int16 — quantitative PET per-slice rescale ([#300](https://github.com/vig-os/tessera/pull/300)) ([#317](https://github.com/vig-os/tessera/pull/317))
- *(ingest)* dicom-series --rescale-mode global-int16 — quantitative PET per-slice rescale ([#300](https://github.com/vig-os/tessera/pull/300))
- *(format)* self-contained .tsra — embed signature + ingest provenance as non-sealed aux/ members ([#259](https://github.com/vig-os/tessera/pull/259))
- *(ingest)* port curated DICOM tags to recon schema fields + full header to extra/ (PHI-scrubbed on de-id)
- *(provenance)* source-file integrity hashes + sealed producer stamp
- PHI hygiene (--source-label + dicom-series --deidentify) + sensitivity tier (ADR-0040 §1) ([#242](https://github.com/vig-os/tessera/pull/242))
- *(blob)* cloud-aware extract + verify-while-pack + opt-in parallel blake3 ([#234](https://github.com/vig-os/tessera/pull/234))
- *(schema)* recommended-field severity (warn tier) + generic ingest --meta ([#233](https://github.com/vig-os/tessera/pull/233))
- *(blob)* bounded-memory streaming ingest + extract (closes #231) ([#232](https://github.com/vig-os/tessera/pull/232))
- opaque blob/"junk" block — bit-faithful preservation of un-parsed vendor raw ([#229](https://github.com/vig-os/tessera/pull/229)) ([#230](https://github.com/vig-os/tessera/pull/230))
- *(ingest)* validate-on-seal — honor the fd5 schema contract at ingest
- *(ingest,cli)* declarative ingest engine + cross-block query over ingested multi-block listmode (goal pt1↔pt2)
- *(io,ingest)* multi-block listmode ingest — parallel encode + constant-memory >RAM (ADR-0026/0034 §3)
- *(ingest)* DICOM JPEG Baseline decodes pure-Rust + golden-hash lock — flip matrix row ✓
- *(ingest)* ADR-0026 §3 streaming HDF5 reader + flip GE-HDF5 row ✓
- *(ingest)* raw headerless binary reader + flip ingest row ✓ (NIfTI + raw)
- *(ingest)* NIfTI-1 reader → recon product with LPS world_frame ([#208](https://github.com/vig-os/tessera/pull/208))
- *(cli)* tessera ingest dicom / ge-hdf5 subcommands
- *(ingest)* GE-HDF5 2-photon listmode events ([#208](https://github.com/vig-os/tessera/pull/208))
- *(ingest)* GE-HDF5 listmode vendor-raw ingest ([#208](https://github.com/vig-os/tessera/pull/208))
- *(ingest)* PS3.15 de-identification — strip + verify PHI (P5, #207)
- *(ingest)* DICOM multi-slice series stacking → 3-D volume (P5, #207)
- *(ingest)* tessera-ingest crate — lossless DICOM → recon product (P5, #207)

### Fixed

- *(ingest)* --source-label also redacts the sealed blob filename ([#269](https://github.com/vig-os/tessera/pull/269)) ([#281](https://github.com/vig-os/tessera/pull/281))
- *(cli)* collection verify/ls resolve members by the sanitized name ([#323](https://github.com/vig-os/tessera/pull/323)) ([#339](https://github.com/vig-os/tessera/pull/339))
- *(ingest)* make the --spec collection engine atomic ([#302](https://github.com/vig-os/tessera/pull/302)) ([#318](https://github.com/vig-os/tessera/pull/318))
- *(ingest)* loud PHI warning when a DICOM ingest seals without --deidentify ([#269](https://github.com/vig-os/tessera/pull/269))
- *(ingest)* apply spec [product.metadata] before seal (was silently ignored)

### Other

- ADR-0057 Phase 0: `tessera info` + Gate B + drop static-hdf5 from the PR clippy matrix ([#404](https://github.com/vig-os/tessera/pull/404))
- *(#235)* PET/CT study migration shape — fine-grained --meta collection (recon + blob)
- *(ingest)* generic HDF5-compound reader — delete hardcoded Rec2p/Rec3p ([#222](https://github.com/vig-os/tessera/pull/222))
- *(ingest)* extract shared 2p/3p transpose helpers (prep ADR-0026 §3 streaming)
- *(adr)* ADR-0026 streaming chunked table writes for >RAM ingest
