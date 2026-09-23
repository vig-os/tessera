# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-alpha.1](https://github.com/vig-os/tessera/releases/tag/tessera-cli-v0.1.0-alpha.1) - 2026-09-23

### Added

- *(io)* close the array dtype envelope — i1/u1/b1/f2 via checked transparent widening ([#418](https://github.com/vig-os/tessera/pull/418)) ([#420](https://github.com/vig-os/tessera/pull/420))
- *(ingest)* offline crypto-shred de-identification for DICOM (ADR-0047, #269) ([#279](https://github.com/vig-os/tessera/pull/279))
- *(ingest)* blob-series — multi-file series as one .tsra, a block per file (no tar) ([#328](https://github.com/vig-os/tessera/pull/328))
- *(core+ingest)* land ADR-0058 generation-provenance on dev (supersedes #346) ([#415](https://github.com/vig-os/tessera/pull/415))
- *(dist)* static-HDF5 self-contained builds + cargo-dist broad channel ([#374](https://github.com/vig-os/tessera/pull/374)) ([#375](https://github.com/vig-os/tessera/pull/375))
- *(config)* resource caps — flag > env > conf > default (first global config file) ([#372](https://github.com/vig-os/tessera/pull/372))
- *(io)* nullable table columns — a validity mask, not a sentinel ([#360](https://github.com/vig-os/tessera/pull/360))
- *(table)* Bool + Utf8 ColumnData — first-class boolean & string columns ([#354](https://github.com/vig-os/tessera/pull/354))
- *(cli)* --verify deep opt-in on tree/inspect; honest seal-vs-verified badge (#268 part 1) ([#341](https://github.com/vig-os/tessera/pull/341))
- *(cli)* collection new — assemble a collection from pre-sealed .tsra (#304 Part 1) ([#321](https://github.com/vig-os/tessera/pull/321))
- *(ingest)* dicom-series --rescale-mode global-int16 — quantitative PET per-slice rescale ([#300](https://github.com/vig-os/tessera/pull/300)) ([#317](https://github.com/vig-os/tessera/pull/317))
- *(ingest)* tessera ingest ge-hdf5 --quantize — opt-in float→int16 transform ([#310](https://github.com/vig-os/tessera/pull/310)) ([#316](https://github.com/vig-os/tessera/pull/316))
- *(core)* table Column carries unit/description/short_name/scale ([#307](https://github.com/vig-os/tessera/pull/307)) ([#312](https://github.com/vig-os/tessera/pull/312))
- *(core)* collection level schemas — terminology as registered/versioned data (ADR-0050, #294) ([#315](https://github.com/vig-os/tessera/pull/315))
- *(cli)* recursive collection verify — descend into sub-collection members (ADR-0049 Part 2) ([#309](https://github.com/vig-os/tessera/pull/309))
- *(cli)* collection consumer verbs — inspect / ls / verify ([#272](https://github.com/vig-os/tessera/pull/272)) ([#282](https://github.com/vig-os/tessera/pull/282))
- *(cli)* tsra pyramid — build a multiscale image pyramid → a derived .tsra (#260 phase 2)
- *(format)* self-contained .tsra — embed signature + ingest provenance as non-sealed aux/ members ([#259](https://github.com/vig-os/tessera/pull/259))
- *(cli)* tsra project — axis projection (MIP / mean / sum) of an array block (#260 phase 1)
- *(cli)* tsra slice --world — mm addressing via the stored affine (#253 phase 2)
- *(cli)* tsra sql — DataFusion SQL over a table block (feature-gated, #251 spike)
- *(cli)* ls/tree surface the embedded schema, extra/ namespace, and sidecar files
- *(cli)* schema field roster — `tsra schema` lists declared fields (tier + sensitivity + populated/missing)
- *(cli)* array exploration — `tsra stats` + `tsra slice` (index-native) (#253 phase 1)
- *(ingest)* port curated DICOM tags to recon schema fields + full header to extra/ (PHI-scrubbed on de-id)
- *(cli)* richer `tsra read` slicing — open/negative --rows, --head/--tail/--at, comma columns; clear array-block error
- *(provenance)* source-file integrity hashes + sealed producer stamp
- *(core)* embed the product schema in every sealed manifest (obligatory, self-describing)
- PHI hygiene (--source-label + dicom-series --deidentify) + sensitivity tier (ADR-0040 §1) ([#242](https://github.com/vig-os/tessera/pull/242))
- *(blob)* cloud-aware extract + verify-while-pack + opt-in parallel blake3 ([#234](https://github.com/vig-os/tessera/pull/234))
- *(schema)* recommended-field severity (warn tier) + generic ingest --meta ([#233](https://github.com/vig-os/tessera/pull/233))
- *(blob)* bounded-memory streaming ingest + extract (closes #231) ([#232](https://github.com/vig-os/tessera/pull/232))
- opaque blob/"junk" block — bit-faithful preservation of un-parsed vendor raw ([#229](https://github.com/vig-os/tessera/pull/229)) ([#230](https://github.com/vig-os/tessera/pull/230))
- *(signing)* bind signed_at + key_format; ssh-ed25519 loader; usage tests-as-docs
- *(cli)* keygen + trust store + verify-sig defaults to it (ADR-0037 alpha tier)
- *(io,cli)* tessera push / pull — in-Rust OCI registry client
- *(cli)* tessera ingest dicom-series — multi-file CT/PET stack → one recon
- *(io,cli)* forget + gc — reclaim space from deleted lineages (ADR-0036)
- *(io,cli)* commit --add-block / --remove-block — compose encoded blocks (ADR-0036)
- *(cli)* tessera diff / publish / seal — complete the ADR-0036 verb set
- *(io,cli)* evolve + tessera init/import/commit/log — CoW versioning verbs (ADR-0036)
- *(cli)* tessera tree / ls / read — navigate + extract a .tsra hierarchy
- *(io,cli)* #225 cloud-read landing — public open_url + tail-prefetch + cohort prune-before-fetch (goal pt3)
- *(ingest,cli)* declarative ingest engine + cross-block query over ingested multi-block listmode (goal pt1↔pt2)
- *(io,cli)* adaptive thread allocator — WriteConfig::balanced + tessera bench write --auto
- *(io,ingest)* multi-block listmode ingest — parallel encode + constant-memory >RAM (ADR-0026/0034 §3)
- *(io,cli)* WriteConfig (SSoT workers+ram_budget) + tessera bench write — size your system, ADR-0034-honest
- *(signing)* flip FEATURE-MATRIX signing row ✓ (fresh-audit-verified) + verify-sig checks payloads
- *(cli)* tessera sign / verify-sig verbs (S16 end-to-end CLI)
- *(cli)* tessera export ro-crate / datacite
- *(cli)* tessera ingest dicom / ge-hdf5 subcommands
- *(cli)* `tessera schema` — validate a .tsra against its product schema (P7, #210)
- *(cli)* tessera CLI — pack/unpack/verify/inspect ([#205](https://github.com/vig-os/tessera/pull/205))

### Fixed

- *(table)* register Vortex Pco for float columns — f64 tables were stored raw ([#380](https://github.com/vig-os/tessera/pull/380)) ([#384](https://github.com/vig-os/tessera/pull/384))
- *(ci)* make the local gates actually run — hook install, dead hooks, trycmd sandbox ([#357](https://github.com/vig-os/tessera/pull/357))
- *(verify)* stream block payloads + typed, located integrity errors ([#268](https://github.com/vig-os/tessera/pull/268)) ([#340](https://github.com/vig-os/tessera/pull/340))
- *(cli)* collection verify/ls resolve members by the sanitized name ([#323](https://github.com/vig-os/tessera/pull/323)) ([#339](https://github.com/vig-os/tessera/pull/339))
- *(cli)* usability quick-wins from the 5-persona review
- *(ingest)* apply spec [product.metadata] before seal (was silently ignored)

### Other

- *(book)* expand the mdBook 2→15 chapters, mapped to the feature matrix ([#285](https://github.com/vig-os/tessera/pull/285))
- ADR-0057 Phase 0: `tessera info` + Gate B + drop static-hdf5 from the PR clippy matrix ([#404](https://github.com/vig-os/tessera/pull/404))
- AX onboarding quick wins: SIGPIPE fix + why-tessera comparison + quickstart ([#390](https://github.com/vig-os/tessera/pull/390)) ([#391](https://github.com/vig-os/tessera/pull/391))
- *(verify)* fan the L2 payload probe across the worker pool ([#371](https://github.com/vig-os/tessera/pull/371))
- *(integrity)* pin everyday tamper/trust cases + the L1/L2 verify ladder ([#369](https://github.com/vig-os/tessera/pull/369))
- *(cli)* grouped/structured `tsra help` by command family ([#249](https://github.com/vig-os/tessera/pull/249))
- *(cli)* elide + group multi-file provenance in tree/ls/inspect
- *(#235)* PET/CT study migration shape — fine-grained --meta collection (recon + blob)
- *(cli)* terse top-level --help + positional descriptions ([#243](https://github.com/vig-os/tessera/pull/243)) ([#244](https://github.com/vig-os/tessera/pull/244))
- *(cli)* persona lifecycle as e2e tests-as-docs (incl. the crypto) + run trycmd in CI
- *(cli)* end-to-end inspect walkthrough over a corpus fixture (trycmd)
- *(cli)* more trycmd walkthroughs (version + subcommand help)
- *(cli)* trycmd CLI walkthroughs — docs-as-tests layer 2 (gated)
