# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-alpha.6](https://github.com/vig-os/tessera/compare/tessera-py-v0.1.0-alpha.5...tessera-py-v0.1.0-alpha.6) - 2026-11-19

### Added

- *(io)* [**breaking**] record the codec profile in every sealed table spec
- *(io)* [**breaking**] record the vortex edition in every sealed table spec

### Fixed

- *(py)* name the I-JSON rule for an int a seal cannot hold exactly
- *(core)* seal only integers a double holds exactly (I-JSON)
- *(core,io)* refuse a block name listed twice in one product
- *(core)* verify a manifest's seal over the object it parsed
- *(io)* hold every .tsra record to the bytes the writer emits
- *(io,py,deps)* read a .tsra the one way every zip reader does (GHSA-m665-v7pm-7grj)

### Other

- *(py)* expect the neutral seal-time wording in the smoke test
- *(corpus)* regenerate the table goldens for the vortex 0.93 format event

## [0.1.0-alpha.5](https://github.com/vig-os/tessera/compare/tessera-py-v0.1.0-alpha.4...tessera-py-v0.1.0-alpha.5) - 2026-11-11

### Added

- *(py)* read tables by logical name and type the not-a-block error
- *(core)* add the ADR-0064 partition marker to TableSpec

## [0.1.0-alpha.3](https://github.com/vig-os/tessera/compare/tessera-py-v0.1.0-alpha.2...tessera-py-v0.1.0-alpha.3) - 2026-10-19

### Fixed

- *(py)* link the _native extension dylib on macOS

## [0.1.0-alpha.2](https://github.com/vig-os/tessera/compare/tessera-py-v0.1.0-alpha.1...tessera-py-v0.1.0-alpha.2) - 2026-10-19

### Added

- *(py)* expose with_producer / with_generation on Builder
- *(py)* pin a parent's seal on Builder.add_source
- *(py)* expose the array codec on Builder.add_array

### Other

- *(py)* prove the codec knob round-trips and the default is unchanged
- ingest cookbook, generated product-schema reference, tested Python write example, cli.md stubs ([#484](https://github.com/vig-os/tessera/pull/484))
- clear the lint backlog the new CI gate surfaced

## [0.1.0-alpha.1](https://github.com/vig-os/tessera/releases/tag/tessera-py-v0.1.0-alpha.1) - 2026-09-23

### Added

- *(io)* close the array dtype envelope — i1/u1/b1/f2 via checked transparent widening ([#418](https://github.com/vig-os/tessera/pull/418)) ([#420](https://github.com/vig-os/tessera/pull/420))
- *(core)* table Column carries unit/description/short_name/scale ([#307](https://github.com/vig-os/tessera/pull/307))
- *(py)* ergonomic Python layer — numpy / polars / pyarrow returns
- *(io,py)* cross-block read/query over multi-block tables — LogicalTableView (goal pt1)
- *(io,py)* table column projection — read one column, not the whole block ([#212](https://github.com/vig-os/tessera/pull/212))
- *(py)* read_array_subset — 3-D ROI reads from Python ([#210](https://github.com/vig-os/tessera/pull/210))
- *(py)* write path — Builder packs .tsra from numpy ([#210](https://github.com/vig-os/tessera/pull/210))
- *(py)* decode array/table blocks to numpy ([#210](https://github.com/vig-os/tessera/pull/210))
- *(py)* Tessera Python bindings — read + verify ([#210](https://github.com/vig-os/tessera/pull/210))

### Fixed

- *(py)* decode str and b1 table columns in the ergonomic reads ([#421](https://github.com/vig-os/tessera/pull/421)) ([#422](https://github.com/vig-os/tessera/pull/422))

### Other

- *(py)* document the full table-block dtype set — i1/u1/b1/str are table-only ([#414](https://github.com/vig-os/tessera/pull/414))
