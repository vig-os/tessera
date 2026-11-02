# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Added

- `tessera-core`: one manifest-only resolver for a table name, `block::table_group`, with the typed
  `Error::LogicalTable` and its `LogicalTableError` reasons, and the seal-time check `block::check_partitions`, which
  `ProductBuilder::seal` now runs (ADR-0064 PR A, #709).
- `tessera-io`: `Reader::table_group` resolves a table name through that resolver, and `Reader::logical_table`
  (`LogicalTableView`) is built on it, so it reads a partitioned table (`pairs/0000`, `pairs/0001`, …) as one table
  as well as a single block or a legacy `events_NNNN` group (ADR-0064 PR B, #710). `tessera-core` adds
  `block::group_specs`, the one schema check for a resolved group, and `TableGroup::blocks`.
- `tessera-py`: `Reader.table`, `table_arrow`, `table_dict` and `column` (and the native `read_table` /
  `read_table_column`) take a logical table name, so a partitioned table (`pairs/0000`, `pairs/0001`, …) reads as
  one table, every leaf's rows in leaf order, as a legacy `events_NNNN` group does (ADR-0064 PR C2, #717). A name
  that is not a block passed to `read_block`, `array` or `array_roi` raises the new `tessera.NotABlockError`, a
  subclass of `TesseraError`.
- `tessera-cli`: `ls` and `tree` show a partitioned table as one node, `pairs/  table, 3 blocks, 8 rows`, where they
  listed each leaf; `ls FILE pairs` lists its columns and leaves, and `tree --full` lists the leaves under the node.
  An invalid partitioned table shows its typed error instead. `read` and `sql` answer `pairs` as one table, and
  `read` on a node that holds no table says so and points to its blocks (ADR-0064 PR C1, #711). Legacy
  `events_NNNN` blocks are listed one per line, as before.

### Changed

- **Breaking for struct literals (pre-1.0), `tessera-core`:** `TableSpec` has a new public field,
  `partition: Option<Partition>` (ADR-0064, #728). A `TableSpec { .. }` literal outside this workspace no longer
  compiles until it adds `partition: None`; downstream crates such as visionx-rs need that one-line change.
  Serialized specs are unchanged, because `None` is skipped.
- `tessera-io`: `Reader::logical_table` reports a name it can't resolve, or blocks that disagree on their schema, as
  `Error::LogicalTable` with a typed `LogicalTableError` reason, where it returned `Error::Container` messages; a
  caller matching `Error::Container` for "no blocks for prefix" must match `LogicalTableError::NotFound`. A single
  non-table block (an array) is now `NotATable`, and a legacy `events_NNNN` block whose spec doesn't parse as a
  `TableSpec` is `SchemaMismatch` (it was `Error::Serde`). `Reader::block_group` is deprecated in favour of
  `table_group`; its behaviour (the legacy flat rule only) is unchanged.
- `tessera-io`: `LogicalTableView::prefix` is deprecated in favour of `LogicalTableView::name`. The view is resolved
  from a table name, which is no longer always a block-name prefix; both return the same string.
- `tessera-io`: `unpack` and `pack_dir` refuse a product with an invalid partitioned table (`Error::LogicalTable`,
  ADR-0064 §2.4) before writing anything, as `ProductBuilder::seal` and so `WriteSession::seal` already do.

### Removed

### Fixed

- `tessera-core`: a partition marker whose forged index makes the canonical leaf name longer than 255 bytes is
  refused as `LogicalTableError::NonCanonicalLeaf`, not a bare `Error::InvalidBlockName` (#733).
- `tessera-core`: `table_group` refuses a marked legacy-named block under a node (`a/pairs_0001`) with the error the
  seal gives for its parent node, instead of resolving a legacy flat group around it (#733, ADR-0064 §3 rule 4
  clarified).

### Security
