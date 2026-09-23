# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
