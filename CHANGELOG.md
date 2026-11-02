# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Added

- `tessera-core`: one manifest-only resolver for a table name, `block::table_group`, with the typed
  `Error::LogicalTable` and its `LogicalTableError` reasons, and the seal-time check `block::check_partitions`, which
  `ProductBuilder::seal` now runs (ADR-0064 PR A, #709).

### Changed

- **Breaking for struct literals (pre-1.0), `tessera-core`:** `TableSpec` has a new public field,
  `partition: Option<Partition>` (ADR-0064, #728). A `TableSpec { .. }` literal outside this workspace no longer
  compiles until it adds `partition: None`; downstream crates such as visionx-rs need that one-line change.
  Serialized specs are unchanged, because `None` is skipped.

### Removed

### Fixed

### Security
