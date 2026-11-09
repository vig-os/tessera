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
- `tessera-io`: `StreamWriter::push_table(name, spec, groups)` writes a streamed table of any size as one logical
  table, partitioned at `BLOCK_ROWS` into the marked leaves `name/0000`, `name/0001`, …, byte for byte the leaves
  `TableMultiBlockSink` writes for the same rows (ADR-0064 PR E, #713); at most `BLOCK_ROWS` rows it is the one block
  `name`. Each block's input is staged on disk in the session's `staging/` directory and handed to a free encode
  worker once complete, so at most `W + 1` blocks of input are staged for `W` workers, counted by the new
  `StreamStats::peak_staged_input_blocks` and `peak_staged_input_bytes`. A row count that disagrees with
  `spec.rows`, or a row-group above `ROWS_PER_GROUP` rows, is `Error::StreamedTable` with a typed
  `StreamedTableError` reason (`tessera-core`).
- `tessera-io`: `StreamWriter::push_table` documents its heap bound, `min(W, n) · p_block + s + c_leaf · n` for `W`
  encode workers and `n` leaves, with `p_block` one leaf's measured write peak, `s = 2 · ROWS_PER_GROUP · r_mem + 4 MiB`
  (7 MiB for 24-byte rows) and `c_leaf = 2 · (e_leaf + b_leaf)` for the manifest entry each committed leaf keeps until
  the seal (`e_leaf` the entry's canonical JSON length, `b_leaf = 1 KiB`; ADR-0064's #760 amendment, holding once
  #759 lands). Beyond that per-leaf term the bound doesn't grow with the row count. The rustdoc also says that staged
  input is page cache, which counts toward a cgroup's memory and is RAM on tmpfs (ADR-0064 PR F, #714).
  `tests/staged_job_memory.rs` checks the bound on the heap (without the per-leaf term, at most 25.3 KB at its 8
  leaves), and that neither the reader filling a block nor a worker reading it back holds the block in RAM.
- `tessera-identity`: a new crate holding the ADR-0047 crypto-shred envelope primitives, `IdentityDocument`,
  `AUX_IDENTITY_NAME`, `encrypt_identity`, `decrypt_identity`, `parse_recipient` and `generate_keypair`, moved out of
  `tessera-ingest` (#762). It depends on `tessera-core`, `age` and serde and links no libhdf5, so a Rust producer
  writes an envelope without `tessera-ingest`. `tessera-ingest` re-exports it as `tessera_ingest::identity`, so every
  existing path still resolves to the same item, and the envelope format is unchanged. It also re-exports
  `age::x25519::Recipient` as `tessera_identity::Recipient`, so a caller needs no direct `age` dependency.

### Changed

- **Breaking for writer output, `tessera-io` / `tessera-ingest`:** a table above `BLOCK_ROWS` rows is written as the
  leaves `<table>/0000`, `<table>/0001`, … (`events/0000` for GE listmode), each carrying the partition marker
  `{"index": i, "last": b}`, where it was `<table>_0000`, … without one (ADR-0064 PR D, #712). This covers GE
  whole-file and streaming ingest and `tessera bench --input` (`TableMultiBlockSink`). The payloads, block digests
  and so `content_hash` are unchanged for the same rows; the names and markers move `manifest_hash`. A table of at
  most `BLOCK_ROWS` rows stays one bare-named block, byte-identical. Products written before still read unchanged.
  `table::block_name` now returns `Result<String>` (the leaf name is checked against the block-name grammar), and
  the new `table::block_partition` gives each block's marker.
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
- **Breaking for `table_job_streaming` above `BLOCK_ROWS` rows, `tessera-io`:** the job fails with
  `Error::StreamedTable` (`StreamedTableError::NeedsPartition`, naming `StreamWriter::push_table`) where it wrote one
  block of any size; the alpha.4 "any size" contract is withdrawn (ADR-0064 §5.4). At or below `BLOCK_ROWS` it is
  unchanged. It is deprecated in favour of `push_table`.

### Removed

### Fixed

- `tessera-core`: a partition marker whose forged index makes the canonical leaf name longer than 255 bytes is
  refused as `LogicalTableError::NonCanonicalLeaf`, not a bare `Error::InvalidBlockName` (#733).
- `tessera-core`: `table_group` refuses a marked legacy-named block under a node (`a/pairs_0001`) with the error the
  seal gives for its parent node, instead of resolving a legacy flat group around it (#733, ADR-0064 §3 rule 4
  clarified).
- `tessera-io`: a `WriteSession` holds each committed block until the seal as its journal line, the compact JSON of
  its manifest entry, instead of a cloned `BlockRef` whose spec is a `serde_json::Value` tree. Per leaf of a
  12-column table the session held 13.5 KB of heap in small allocations for a 2.4 KB entry (12.8 KB live for the
  #715 consumer's 3.1 KB entry); it now holds the entry's bytes in one allocation, plus the leaf's name.
  `tests/session_leaf_memory.rs` checks the bound. Sealed bytes are unchanged; no public API changes (#759).

### Security
