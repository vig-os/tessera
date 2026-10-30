# ADR-0064 — Logical tables over numbered leaves: `<table>/NNNN`, a sealed partition marker, one logical reader

**Status:** Proposed (2026-10-30). Records the owner's decision on
[#677](https://github.com/vig-os/tessera/issues/677) (comment of 2026-10-29: option **P-logical** of the
[#681](https://github.com/vig-os/tessera/issues/681) design note, `docs/spikes/681-vortex-writer-backpressure.md`,
merged in PR #692 at `b6de2d1`) and specifies it so that the implementation can be briefed from this ADR
alone. Resolves the five re-review nits recorded on #681 (§11). Amends SPEC §5b, ADR-0026, ADR-0062
(`RowAlignment` only) and ADR-0063 (§9). Code references are at `origin/dev` `4504efb`, under
`tessera/crates/`, unless they say otherwise.

## Context

#677 asks for "one scan product with single named `pairs`, `markers`, `ms_stats` table blocks and a
configurable buffer bound independent of input size". The consumer, MorePET/visionx-rs, streams list-mode
coincidences into one `pairs` table (2.4 G rows, 19.2 GB of packet-equivalent input, under an 8 GiB RSS
budget) and reads it back through tessera's logical APIs.

The #681 note measured why one canonical table block of unbounded rows can't have a bounded RSS with the
same bytes: Vortex's writer holds an order debt `D(width)` and a per-column working set `K(width)` per
encode, and its zone statistics and footer-bound per-chunk state grow by `c · rows` (0.058 B/row for the
consumer shape) until the footer is written (note §1.2-§1.4, §2). Something has to bound the rows one
encode holds. Of the options that do (note §5.2):

- **P** partitions at `BLOCK_ROWS` and names the blocks `pairs_0000`, `pairs_0001`, … (SPEC §5b today);
- **P-logical** partitions identically and names them `pairs/0000`, `pairs/0001`, … under ADR-0063's
  hierarchical grammar, so `pairs` is a node of the name tree and every logical reader presents it as one
  table;
- **R** writes Vortex row groups inside one block.

The owner chose **P-logical**. That retires ADR-0026's "one logical block stays **one canonical block**: no
suffixed physical blocks" (`docs/adr/0026-streaming-table-ingest.md:190-191`) for streamed tables, and with
it #677's literal "single physical block" wording. The Vortex writer gate (option F, vortex#10046) is a
separate upstream track and out of scope here (§10).

Today:

- **The naming function** is `table::block_name` (`tessera-io/src/table.rs:890-896`): `prefix` for one block,
  `{prefix}_{idx:04}` otherwise. Its writers are GE ingest's whole-file partition
  (`tessera-ingest/src/ge_hdf5.rs:1007`) and `TableMultiBlockSink::finish` (`tessera-io/src/accumulate.rs:575`),
  which GE streaming ingest (`ge_hdf5.rs:809`) and `tessera bench --input` (`tessera-cli/src/bench.rs:265`)
  drive. Open PR #538 adds the generic `ingest table` lanes as a third caller.
- **The grouping function** is `Reader::block_group` (`tessera-io/src/container.rs:438-461`): the bare
  `prefix` plus every `prefix_` + exactly four ASCII digits, in manifest order. A user's own `pairs_0003`
  block silently joins the group (note §5.2, "Unambiguous grouping").
- **`table_job_streaming`** (`tessera-io/src/stream.rs:121-143`) writes "ONE canonical table block of
  **any size**". alpha.4 (`v0.1.0-alpha.4`, `2e9f530`) shipped that contract.
- **A product holding `pairs/0000` and `pairs/0001`** is already valid for writers and readers:
  `ProductBuilder::seal` (`tessera-core/src/product.rs:172-173`) and the write session
  (`tessera-io/src/write.rs:771-783`) accept it under the leaf rule. But `block_group("pairs")` doesn't match
  it, so `read pairs`, `sql … pairs` and `logical_column("pairs")` fail with "no blocks for prefix"
  (`tessera-io/src/multiblock.rs:377-381`).

## Decision

### §1 — Naming: `<table>/{index:04}`, padded per index, never per count

A table partitioned into two or more blocks is written as the leaves

```text
<table>/0000, <table>/0001, …, <table>/9999, <table>/10000, …
```

1. **The leaf of index `i` is `format!("{i:04}")`**: the decimal index, left-padded with `0` to at least four
   digits and never longer than needed. Indices start at `0` and are contiguous. The full name is
   `<table>/<leaf>`, a valid ADR-0063 name whenever `<table>` is one (a leaf segment is `[0-9]+`).
2. **The width is a function of the index alone.** It is not fixed (no ceiling at 9999) and not derived
   from the block count, so naming block `i` needs nothing but `i`. So the encoding is deterministic: the same rows, partitioned by the same `BLOCK_ROWS`,
   always give the same names, the same manifest and the same `content_hash`.
3. **There is no overflow.** Past 9999 the leaf grows to five digits (`10000`), and so on. The only bound is
   ADR-0063's 255-byte name limit, which a `u64` index (at most 20 digits) can't reach for any `<table>` of
   at most 234 bytes; a longer `<table>` is refused by the grammar at the writer, as any over-long name is.
4. **Order is the index, not the string.** Past 9999 the lexicographic order of the names (`10000` sorts
   before `9999`) is not the logical order. Every reader orders leaves by the parsed index (§3), never by
   sorting names. Writers emit the leaves in index order in the manifest (§2.4), so a conforming product's
   manifest order *is* the logical order, and a zip or S3 listing past 9999 is merely displayed out of order.
5. **Canonical leaf form.** A leaf whose segment parses as index `i` but is not exactly `format!("{i:04}")`
   (`pairs/00003`, `pairs/3`) is non-canonical: readers refuse it inside a partitioned node (§3), writers never
   produce it. One index has one name, so one partition has one manifest.
6. **Small stays single, unchanged.** A table of at most `BLOCK_ROWS` rows (one block) is one block named
   with the bare `<table>`, byte-identical to every product written so far. It carries no partition marker
   (§2). The partition function (`table::block_count` / `partition_blocks`, `table.rs:872-883`) and
   `BLOCK_ROWS` itself don't change.

**Why.** The four-digit floor keeps the common case identical in shape to today's `_NNNN` suffix and to
`tessera pyramid`'s numbered levels. The consumer's 2.4 G rows are 573 blocks; 9999 blocks are 41.9 G rows,
which a one-hour acquisition at 10 M events/s (36 G rows) nearly reaches, so a fixed four-digit width would
put a format ceiling inside the realistic range. A count-derived width ties every name to the total, which the
writers of this plan know (`push_table` takes `spec.rows`, §5.3), but a future partitioning of an unknown-length
stream (ADR-0026's DAQ `TableStreamWriter`, not part of this plan) would not; the index-padded form costs
nothing and keeps that door open.

**Rejected.**

- *Fixed four digits with a typed overflow error past 9999.* A real ceiling (41.9 G rows) within reach of a
  long acquisition, for no gain: the index-padded form is identical below it.
- *A minimal-sufficient width derived from the block count* (`max(4, digits(n − 1))`). Deterministic, but it
  makes every name depend on the total, so a writer must know the row count before naming block 0. It
  brings nothing the index-padded form lacks, which is identical to it below 10000 blocks.
- *A wider fixed width* (six or ten digits). Changes every name for no reader benefit and still has a
  ceiling.

### §2 — Grouping is explicit: a sealed per-leaf partition marker

Every leaf of a partitioned table carries a **partition marker** in its sealed `TableSpec`:

```rust
pub struct TableSpec {
    pub columns: Vec<Column>,
    pub rows: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_index: Option<String>,
    /// Set on every leaf of a partitioned table (ADR-0064), never on a single block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition: Option<Partition>,
}

/// This leaf's position in its partitioned table.
pub struct Partition {
    /// The leaf's index: its name is `<table>/{index:04}` (§1).
    pub index: u64,
    /// True on exactly one leaf, the one with the highest index.
    pub last: bool,
}
```

In the manifest it is `"partition": {"index": 3, "last": false}`. Both fields are always serialized, so no
default has to be agreed on. `None` is skipped, so every single-block spec, every corpus golden and every
legacy product serializes byte for byte as before.

1. **What makes a node a partitioned table.** A node `<table>` (a name that is a proper `/`-prefix of at least
   one block name) is a partitioned table when at least one block **exactly one segment below** it (a name
   `<table>/<segment>` with no further `/`) carries a marker. A marker deeper down (`a/b/0000`) makes `a/b` the
   partitioned table, not `a`. A partitioned table is then **valid** only if all of these hold:
   - **Leaves.** Every marked block one segment below `<table>` is a `table` block whose segment is the canonical
     `format!("{index:04}")` of its own marker's `index`.
   - **Children.** Every other block under `<table>/`, at any depth, is a `chunk_index` sidecar named
     `<table>/<leaf>.cidx` (`tessera-io/src/chunk_index.rs:21-23`) for a leaf that exists. Anything else (an
     unmarked table leaf, an array, a blob, a deeper name such as `pairs/0000/x`) is a **foreign child**.
   - **Indices.** The indices are unique and form `0..=k` with no gap, and `k ≥ 1`: a partitioned table has at
     least two leaves, because a table of one block is the bare `<table>` (§1.6). A one-leaf partition is refused
     (`SingleLeaf`), so one table has one encoding.
   - **Terminal.** Exactly one leaf has `last: true`, and it is the leaf with index `k`.
   - **Schema.** Every leaf has the same column count, names and dtypes (the check
     `multiblock.rs:396-415` makes today).
2. **A marker belongs to a leaf.** A marked block whose name has no parent node (a top-level `pairs` carrying a
   marker) is refused (`StrayMarker`): a single block carries no marker (§1.6).
3. **Readers refuse an invalid node with a typed error** (§3), never a silent skip, a partial view or a
   best-effort merge.
4. **Writers refuse it where a set of names is finalised**, the same points that check ADR-0063's leaf rule
   (ADR-0063 §4): `ProductBuilder::seal` (`product.rs:172`), `WriteSession::seal` (`write.rs:1010`), `pack_dir`
   (which seals an exploded product) and `unpack`, before they write anything. They run the same validation,
   through one function in `tessera-core` (PR A of the implementation plan), over every node that holds a marked
   leaf, every stray marker, and the mixed-form rule of §4. An append can't check it, because the terminal leaf
   arrives last.
5. **Manifest order.** Writers MUST emit a table's leaves in index order, consecutively. The seal checks this
   too. Readers order by index and don't depend on it (§1.4).
6. **Per-leaf row counts are not checked.** SPEC §5b's partition puts `BLOCK_ROWS` rows in every leaf but the
   last, and every production writer follows it. Readers map rows from each leaf's `TableSpec.rows`, so they
   don't need it, and test products partitioned at a smaller `block_rows` (the `with_block_rows` seams,
   `ge_hdf5.rs:696`, `accumulate.rs` `with_block_rows`) stay readable.

**Why a marker, and why `{index, last}`.** It makes each leaf self-describing and makes completeness
checkable from the manifest alone:

- A user's `pairs/0003` added next to the writer's `pairs/0000…0002` (nit 2) carries no marker, so it is a
  foreign child; with a forged marker it comes after the terminal leaf, or duplicates an index. Either way the
  node is refused, at the seal for a product built with it (`commit --add-block`, the Python builder) and at
  read time for a hand-sealed one.
- A missing leaf is a gap, or a missing terminal; `commit --remove-block pairs/0002` is refused at the seal.
- `last` rather than a `count`: either detects a lost tail, but `last` needs no total, so a leaf's marker is
  fixed by its own position and the end of input, never by a number every other leaf must repeat. `push_table`
  knows its total (§5.3); a future unknown-length partitioning (above, §1) would not.

**Rejected.**

- *Reserve numbered leaves for the engine's writers* (generic paths refuse a name whose last segment is all
  digits). A writer-side policy the reader can't verify: a hand-sealed product, or one written before the rule,
  is indistinguishable from a conforming one. It would also reserve `tessera pyramid`'s `volume/0`,
  `volume/1`, … (ADR-0063 §3), which are array levels, not table leaves.
- *Require a contiguous run from `0000`, without a marker.* Doesn't solve nit 2: a user's `pairs/0003` after
  the writer's `0000…0002` extends the run and joins the table. Nor can it detect a lost tail leaf.
- *A product-level declaration* (a manifest map `logical_tables: {pairs: [leaf names]}`). It duplicates the
  name tree, must be kept in sync by every manifest editor (`commit --add-block` / `--remove-block` /
  `--set`), and isn't available when a write session commits its first leaf. The per-leaf marker is
  committed with its block, in the same journal line.
- *A `count` field instead of `last`.* Equivalent for the writers of this plan, but it repeats one number on
  every leaf and would forbid a future unknown-length writer; `last` is the smaller claim.

### §3 — Resolution: one function decides what a table name means

`Reader::table_group(name) -> Result<TableGroup>` (new, `tessera-io`) resolves a table name, over the
manifest only (no block bytes read), into one of:

- `Single(block)`: `name` is a `table` block and nothing else answers to it (a small table, or a legacy
  oversize block of any row count);
- `Partitioned { leaves }`: `name` is a valid partitioned node (§2.1), leaves in index order;
- `LegacyFlat { blocks }`: blocks matching the legacy `_NNNN` rule (§4), in manifest order.

The rule, in order:

1. **Collect the candidates.** `marked` = the blocks exactly one segment below `name` (§2.1) that carry a
   marker. `flat` = the blocks `block_group` matches today: `name` itself plus every `name_` + four ASCII digits
   (§4).
2. **`marked` and `flat` both non-empty: refuse** with `Ambiguous { flat, node }` (§4). A conforming writer never
   produces this, and the seal refuses it (§2.4), so it comes only from a hand-sealed product.
3. **`marked` non-empty:** validate the node (§2.1) and return `Partitioned`, or the typed error.
4. **`flat` non-empty:** return `Single` when it is the one block `name`, else `LegacyFlat`, exactly as today.
   Unmarked blocks under `name/` don't take part: a legacy product with `events_0000…0002` and an unrelated
   `events/notes` blob (valid under the leaf rule) resolves `events` as it does today. If any block of `flat` whose name has no
   parent node carries a marker, the result is `StrayMarker` (§2.2), for `Single` and `LegacyFlat` alike, so a
   reader refuses what the seal refuses; a marked leaf named in full (`pairs/0003`) is `Single`.
5. **Otherwise:** `NotATable` when `name` is a non-table block (`kind` is its block kind, `"array"`, `"blob"`, …)
   or a node with no marked child (`kind: "node"`; a `tessera pyramid` node such as `volume/` holds array
   levels), else `NotFound`.

A leaf can still be read on its own: `table_group("pairs/0003")` is `Single`, since `pairs/0003` is a block.

**Typed errors.** `tessera_core::Error::LogicalTable { name, reason: LogicalTableError }`, with

```rust
#[non_exhaustive]
pub enum LogicalTableError {
    NotFound,
    NotATable { kind: String },
    Ambiguous { flat: Vec<String>, node: String },
    StrayMarker { block: String },
    SingleLeaf { leaf: String },
    ForeignChild { child: String },
    NonCanonicalLeaf { leaf: String, expected: String },
    DuplicateIndex { index: u64 },
    Gap { missing: u64 },
    NoTerminal { max_index: u64 },
    AfterTerminal { leaf: String },
    SchemaMismatch { leaf: String, detail: String },
}
```

`LogicalTableView` construction (`multiblock.rs:376`) moves from today's `Error::Container(String)` messages to
these variants (`Error` is `#[non_exhaustive]`, `tessera-core/src/error.rs:13`). The single function is shared by
every reader below and by the seal (§2.4), so a product the seal accepts is one every reader resolves.

### §4 — The legacy flat form `_NNNN`: read forever in v0, never written again

- **Existing products keep reading.** Every product with `prefix_NNNN` blocks (GE ingest's `events_NNNN` and
  `TableMultiBlockSink`'s output, alpha.1 to alpha.4 and dev up to the writer switch) resolves through rule 4
  of §3, which is today's `block_group` verbatim: the bare `prefix` plus every `prefix_` + exactly four ASCII
  digits, in manifest order, without a marker and without the §2.1 checks. Its known weakness (a user's
  `events_0003` joins the group) is frozen with it; that weakness is why writers switch.
- **Readers keep the legacy rule for the life of format v0.** Dropping it is a format-v1 decision (ADR-0052 §1),
  not this ADR's. A legacy product is never rewritten in place. Re-ingesting its source gives the new layout,
  with a new `content_hash` (§6).
- **A product mixing `pairs_NNNN` and `pairs/NNNN`** is ambiguous (the bare `pairs` next to `pairs/NNNN` is
  already refused by ADR-0063's leaf rule at every writer; a hand-sealed one resolves as ambiguous too):
  readers refuse `pairs` with `LogicalTableError::Ambiguous`, naming both sets, and writers refuse to seal it.
  Each block stays readable by its own full name. Merging the two forms was rejected: there is no order between
  them that a writer ever declared, and guessing one would make the logical table depend on the reader.
- **`pairs_0003` next to a partitioned `pairs/`** is the same ambiguity: the legacy rule matches it, so
  `pairs` is refused. A product that needs both names must name the unrelated block differently.

**Migration: writers switch, in one PR, after every reader reads both forms.**

1. PR A and PR B (implementation plan) land the marker, the resolution and every reader. From then on every reader on `dev`
   reads both forms, and nothing writes the new one yet.
2. PR C1 and PR C2 land the remaining CLI and Python read surfaces.
3. PR D switches `table::block_name` to `<table>/{idx:04}` with the marker. Every multi-block writer goes
   through it, so all of them switch together: GE whole-file ingest, `TableMultiBlockSink` (GE streaming ingest,
   `bench --input`) and, if #538 has landed, the generic `ingest table` lanes. No writer keeps the old form behind
   a flag.
4. PR E adds the streaming partition for #677, already on the new names.

### §5 — The streaming partition (#677's writer)

1. **API.** `StreamWriter::push_table(name, spec, groups)` (new): one call per logical table. It partitions
   `groups` at `BLOCK_ROWS` (the `table::block_count` partition), names the blocks by §1 and marks them by §2,
   stages each block's input to disk, and dispatches each block as one staged job once its input is complete.
   Leaves are byte-identical to the batch partition of the same rows (`TableMultiBlockSink`, GE whole-file).
   Each leaf is a job under ADR-0026's credit window, so `StreamWindow`'s `J` and `B` apply per leaf.
   `groups` is pulled on the caller's thread, which is the reader; the call returns once the last leaf has been
   dispatched, and `finish` commits and seals as today. Precondition, as for `table_job_streaming`: each group has
   at most `ROWS_PER_GROUP` rows; `push_table` splits a group that straddles a block boundary, so every leaf but
   the last has exactly `BLOCK_ROWS` rows on the same row-group grid as the batch partition.
2. **Input staging.** The reader thread writes each block's row-groups to a staging file in the session's
   `staging/` directory as it fills them (the fragment format `TableMultiBlockSink` already writes,
   `accumulate.rs:485-528`), and dispatches a complete block to a free encode worker, blocking while none is free.
   The worker reads it back one row-group at a time and deletes the file once it has read it.
   **Order of operations** (what makes the `W + 1` bound of §8 hold): the reader creates block `b + 1`'s staging
   file only after block `b` has been handed to a worker. While it waits for a free worker, the full block `b` is
   the one extra file, and the reader pulls no further group.
3. **Names and markers are known up front.** `push_table` takes `spec.rows`, so `n = table::block_count(rows)`
   is known before the first row: whether the table is single (bare name, no marker, §1.6), every leaf's name and
   which leaf is `last`. An input that runs past `spec.rows` is refused when it does, one that ends short is
   refused at its end, before anything is sealed, as `table_job_streaming` refuses a mismatch today.
4. **`table_job_streaming`** keeps its signature for tables of at most `BLOCK_ROWS` rows, byte-identical to
   today. Above that it fails with a typed error that names `push_table`, and it is `#[deprecated]`, pointing to
   `push_table`. The "any size" contract of alpha.4 (`stream.rs:121`) is withdrawn: a single canonical block above
   `BLOCK_ROWS` breaks SPEC §5b's partition rule, and is exactly what can't be bounded (Context).
5. **Rejected input mechanisms** (note §6.4): filling a block in memory (up to `W + 1` blocks of arrays, 391.3 MB
   at `W = 4` for the consumer shape, note §4.7) and a bounded channel per block (68.5 MB, but it serializes
   the encoders, note §4.7). Disk staging is the mechanism; the proof's throughput criterion (§8.1, criterion 6) checks that
   it actually encodes in parallel.
6. **The in-memory single-block constructors stay single-block.** `table::table_block` and the Python
   `Builder.add_table` (`tessera-py/src/lib.rs:299-339`) write exactly the one block their caller names, from
   data already in RAM; they are not partitioning writers and SPEC §5b says so (§9.1). Readers accept a single
   table block of any row count. Generic `ingest table` is an engine writer but today seals one block of any
   size (`tessera-ingest/src/canonical.rs:461`); open PR #538 brings it under the partition, and with it under
   this ADR's names once PR D has landed.

### §6 — What changes for writers, and the format version (nit 1)

| What | Before | After | Moves |
| --- | --- | --- | --- |
| Table of ≤ `BLOCK_ROWS` rows, any writer | bare `<table>` | unchanged | nothing |
| GE whole-file ingest above `BLOCK_ROWS` (`ge_hdf5.rs:1007`) | `events_NNNN` | `events/NNNN` + marker | names, block specs, `manifest_hash`, `content_hash` |
| `TableMultiBlockSink` (`accumulate.rs:575`): GE streaming ingest, `bench --input` | `<prefix>_NNNN` | `<prefix>/NNNN` + marker | same |
| Generic `ingest table` (PR #538, if landed first) | `table_NNNN` | `table/NNNN` + marker | same |
| `table_job_streaming` above `BLOCK_ROWS` (alpha.4 contract) | one block, any size | typed error; `push_table` writes leaves | the consumer's output (§5.4) |
| Products already on disk (alpha.1 to alpha.4) | `_NNNN` | read unchanged (§4) | nothing |

- **`content_hash`es change beyond #677.** Every product above `BLOCK_ROWS` rows written after PR D gets new block
  names and a marker in each block's spec, so a new `manifest_hash` and a new `content_hash` for the same rows.
  The block payloads (the Vortex bytes) and their digests don't change: the partition is the same. Single-block
  products don't move.
- **"Readers don't change" held for P only, not for P-logical.** Under P-logical, `block_group` gains a second
  form, and the CLI and Python surfaces listed in §7 change; the note's "writer output changes; readers don't"
  (note "Decision needed") is withdrawn for this option.
- **Affected goldens and fixtures, by path.** No committed hash golden moves: the conformance corpus
  (`tessera/corpus/corpus.json`, `tessera/corpus/files/*.tsra`), the ingest corpus
  (`tessera/corpus/ingest-corpus.json`) and the trycmd inputs (`tessera-cli/tests/cmd/*.in/study.tsra`) are all
  single-block (verified by listing every manifest's block names at `4504efb`). The tests that assert the old
  writer names, and move with PR D:
  - `tessera-io/src/table.rs:3473-3504`, the partition and `block_name` unit test (`events_0000`, `events_0007`, `events_1234`);
  - `tessera-io/src/accumulate.rs:927` and `:1003-1051`, `TableMultiBlockSink` naming and manifest order;
  - `tessera-io/src/multiblock.rs:503-720`, the logical-view tests, whose helper names blocks through
    `table::block_name` (`:758`) and so switch with it;
  - `tessera-io/src/cloud.rs:600-650`, the cohort prune-before-fetch tests (`logical_table("events")`);
  - `tessera-ingest/src/ge_hdf5.rs:1680-1765`, streamed against whole-file multi-block GE ingest;
  - `tessera-io/tests/block_names.rs:465-470`, the list of generated names (gains `events/0000` and
    `events/10000`).

  These stay, as legacy-reader tests: `tessera-io/src/container.rs:1877-1916` (`block_group` over `events_NNNN`)
  and `tessera-core/src/block/name.rs:357` (grammar test names). PR B adds a committed legacy fixture written by
  the pre-switch writer (§8), so the legacy form keeps a real product under test after PR D.
- **The format version.** This is a **v0 format change under ADR-0052 §1, not a `TESSERA_VERSION` bump.**
  - It is a schema-compatible addition to the manifest: one optional field in a block's `spec`, skipped when
    absent. `BlockRef.spec` is a `serde_json::Value` (`tessera-core/src/block/mod.rs:49-50`), so an older reader
    keeps the field when it recomputes `manifest_hash` and verifies the seal of a new product; `TableSpec` doesn't
    deny unknown fields (`tessera-core/src/block/table.rs:139-145`), so it parses too.
  - An alpha.1 to alpha.4 reader opens, verifies and reads every leaf of a new product by its full name. Asked for
    the logical `pairs`, it fails with its typed "no blocks for prefix" error, never with wrong rows.
  - Bumping `TESSERA_VERSION` ("0.0.0", `tessera-core/src/manifest.rs:20`) would buy nothing: readers gate on the
    major version only (`SUPPORTED_MAJOR = 0`, `:24`), so `0.1.0` wouldn't make an old reader refuse, while it
    would move every product's `manifest_hash` (the producer stamps it, ADR-0052 update of 2026-07-03) and every
    corpus golden.
  - PR D is a breaking change for writers' output, so its commit is marked `feat(io)!:` with a `BREAKING CHANGE:`
    footer, and release-plz's notes carry it.

### §7 — Readers: one logical name everywhere

Every public read path below takes a table name and resolves it through `Reader::table_group` (§3), so `pairs`
means the same table in each:

| Path | Today (file:line at `4504efb`) | Under this ADR | PR |
| --- | --- | --- | --- |
| `Reader::block_group` | `tessera-io/src/container.rs:438-461`, legacy flat only | unchanged; `#[deprecated]`, pointing to `table_group` | B |
| `Reader::table_group` | — | new, §3 | B |
| `Reader::logical_table` / `LogicalTableView` | `tessera-io/src/multiblock.rs:376-430`, via `block_group` | via `table_group`; typed errors | B |
| `select_blocks_overlapping`, `block_stats` (`.cidx` pruning) | `multiblock.rs:225-323` | unchanged; iterate the resolved leaves; `cidx_name` gives `pairs/0000.cidx` | B |
| CLI `read` | `tessera-cli/src/nav.rs:1160-1196`, `logical_table` | works through B; the "not a table" guard (`:1178`) also answers a node | C1 |
| CLI `sql` | `tessera-cli/src/sql.rs:131-161`, `logical_table` → `MemTable` | works through B; registered under the logical name | C1 |
| CLI `ls FILE` | `nav.rs:958-960`, one line per block | a partitioned table is one line, `pairs/  table, k blocks, N rows` | C1 |
| CLI `ls FILE pairs` | `nav.rs:1048`, exact block name only | lists the logical table's columns and its leaves | C1 |
| CLI `tree` | `nav.rs:514-521`, one node per block | one node per logical table; leaves listed under `--full` | C1 |
| Py `read_table`, `read_table_column` | `tessera-py/src/lib.rs:116-153`, one block | resolve the name; a partitioned table concatenates its leaves | C2 |
| Py `read_logical_table_column` | `lib.rs:159-165` | works through B | C2 |
| Py `read_block` | `lib.rs:71` | raw bytes of one block; a node name is a typed "not a block" error | C2 |
| Py `column`, `table_dict`, `table_arrow`, `table` | `tessera-py/python/tessera/__init__.py:106-133` | resolve logical names (through `read_table*`) | C2 |
| ADR-0062 streaming provider, `aligned_to`, zip | not built (ADR-0062 Proposed) | designed on `LogicalTableView`; resolves through `table_group` | (#666, #667) |
| `tessera-wasm` | no table reads (`tessera-wasm/src/lib.rs`) | nothing to change | — |

**`RowAlignment` (nit 5)** is amended in this ADR (§9.3): `block` is the source's logical table name, resolved by
§3, and `source_digest` is over the resolved blocks' digests in logical order (index order for a partitioned table,
manifest order for a legacy flat group). ADR-0062's PR1 fixtures (#666) should cover a partitioned, a legacy flat and
a single-block table; that is a note for #666's brief, not an edit to ADR-0062's body.

### §8 — The memory bound and its proof (nits 3 and 4)

**The bound, normative.** For `push_table` with `W` encode workers writing a table of `n` blocks:

```text
peak heap      ≤  min(W, n) · p_block  +  s
s              =  s_stage + s_fixed
s_stage        =  2 · ROWS_PER_GROUP · r_mem
s_fixed        =  4 MiB
staged input   ≤  W + 1 blocks, including the one being filled
staged bytes   ≤  (W + 1) · BLOCK_ROWS · r_stage
```

- **`p_block`** is the peak heap of a one-block write of the same schema in the same binary with the same `P`
  (`available_parallelism`): measured, never a constant, because `P` sets Vortex's queue depths (note §4.4).
  46.1 MB for the consumer shape at 8 CPUs (note §4.7).
- **`r_mem`** is the input's in-memory bytes per row (the sum of the column widths, plus one validity bit per
  nullable column, rounded up per column to whole bytes); **`r_stage`** is the staged fragment's bytes per row.
  For the consumer shape both are 24 B, so `s_stage` = 3 MiB.
- **`s_stage`** is two row-groups of input: the one the reader is serializing into a staging file and the one an
  encoder has just read back. **`s_fixed`** covers the 8 KiB `BufWriter`s, the commit's 64 KiB verify buffer, and
  the job and window bookkeeping. The measured staging slack was 0.0 MB above `p_block` at `W = 1` (note §4.7), so
  `s` is an allowance for the terms the harness overlapped, not a fit to noise.
- The width terms stay: `p_block ≤` one block's encoded size `+ K(width)` (note §2, §5). That is a documented
  limit, not a defect; the escape hatch for a very wide schema is spilling (note §5, S), not built.

**Size independence, and its tolerance.** At `W = 4` the measured peaks are 128.0, 142.0 and 143.1 MB at 16, 64 and
256 blocks (note §4.7): +11% from 16 to 64 blocks, +0.8% from 64 to 256. The +11% is **not explained**: the window
is full for most of a 16-block run, so overlap saturation alone doesn't account for it, and the note recorded no
timeline of where the peak fell. What is measured is that the ceiling `4 × 46.1 = 184.4` held at every size and that
the rise flattened by 64 blocks. So this ADR asserts two things and doesn't model the rise in between:

- the **ceiling** `min(W, n) · p_block + s` at every `n`, including `n = 16` at `W = 4`, which the proof measures
  (§8.1), so the regime of the +11% is covered by an explicit bound with an explicit `s`;
- the **plateau** where the rise had stopped, `n ≥ 16 W`:

```text
peak(n₂)  ≤  (1 + τ) · peak(n₁)      for  16 W ≤ n₁ < n₂,   τ = 5 %
```

`τ = 5 %` is six times the measured plateau variation (0.8%) and under a third of the smallest regression it must
catch: the zone-statistics half of `c · rows` alone, reappearing across the whole table, adds 0.029 B/row, which is
23 MB (+16%) from 268 M to 1.07 G rows; the full 0.058 B/row adds 47 MB (+33%) (note §4.2). A tolerance above ~15%
would let the zone-stats regression through; one at the measured 0.8% would fail on scheduler noise. `τ` and `s` may
be tightened by the proof; loosening either needs an amendment of this ADR. The proof also records, for every run,
the input row-group at which the peak fell, so the 16-to-64 rise gets an explanation or a bound of its own before
`τ` is tightened.

**The `staged_peak` metric (nit 4).** `StreamStats` gains `peak_staged_input_blocks` and
`peak_staged_input_bytes`. A block counts **from the moment the reader creates its staging file**, while it is still
partial, until its worker deletes the file, and the bytes are the files' sizes on disk at each update, the partial
block included. Counting complete blocks only, as the note's harness did, can't see the `(W+1)`-th file, the one
being filled, and so never tests the bound. The test (PR E) holds all `W` workers at a barrier, feeds rows until the
reader stalls (`StreamStats::producer_stalls` rises), and asserts `peak_staged_input_blocks == W + 1` exactly (all
`W` dispatched, plus the full block waiting for a worker, §5.2) and no more after a further `W` blocks' worth of
input is offered; a second case checks a partial block mid-fill counts (`peak_staged_input_bytes` includes its
bytes while the `W` workers are held); on release, the product equals the batch partition. It is timing-free: a barrier and a
counter, no sleeps.

**Disk staging and page cache (nit 3).** Staging on disk takes the input out of anonymous memory, not out of
memory: the staged files' pages are page cache, which counts toward the cgroup's `memory.current`, and on tmpfs
(shmem) they are RAM that can't be reclaimed.

- **Documented** in `push_table`'s rustdoc and ADR-0026: the session directory (and so `staging/`) SHOULD be on a
  disk-backed filesystem. On tmpfs the bound gains `staged bytes` plus the staged output and the fragments
  (ADR-0026's disk paragraph), all in RAM. Refusing tmpfs was rejected: for a small table it's a legitimate, fast
  choice, and the writer can't know the caller's budget. A warning was rejected: nobody reads library warnings, and
  a documented bound is the contract.
- **Measured** by the proof (§8.1): it samples `RssAnon` **and** the job cgroup's `memory.current` and
  `memory.stat` (`anon`, `file`, `shmem`), and checks that the staging directory is not tmpfs before it measures
  anything, so it can't silently measure the wrong thing.

#### §8.1 — The RSS proof, as an acceptance test (PR G)

- **Where.** A ksb job (`ksb run --cpu 4 --exclusive-cpus`), never the seat or CI, with the session directory on
  the job's `--scratch` volume (disk-backed). The job runs the consumer reproducer (MorePET/visionx-rs
  `crates/visionx-tsra/examples/stream_memory.rs`, PR #41, switched to `push_table`) built against the PR's tessera
  with the patch overrides of its `docs/implementation/39-memory.md`, exact nullable decoder schema (`ms` nullable,
  no `--nonnullable-ms`).
- **Sweep.** `W ∈ {1, 4}` × rows `∈ {0, 4,194,304 (one block), 67,108,864 (16 blocks), 268,435,456 (64),
  1,073,741,824 (256), 2,400,000,000 (573, the consumer's acquisition)}`. Each run also records the input
  row-group at its `RssAnon` peak.
- **Sampling.** Every 10 ms: `RssAnon` from `/proc/<pid>/status`; `memory.current` and `memory.stat`'s `anon`,
  `file` and `shmem` from the job's cgroup. Before the first row: `statfs` of the session directory is not
  `TMPFS_MAGIC` (`0x01021994`), else the job fails.
- **Pass criteria.** With `R₀` the peak `RssAnon` at 0 rows (binary, runtime) and `R₁` at one block, same binary:
  1. **Ceiling:** `RssAnon(n, W) ≤ R₀ + min(W, n) · (R₁ − R₀) + s + s_rss` at every point, 16 blocks included, with
     `s_rss = 32 MiB + 8 MiB · (W + 1)` for allocator retention (one glibc arena's retained top per thread) and
     thread stacks the heap model doesn't see;
  2. **Plateau:** `RssAnon(n₂) ≤ 1.05 · RssAnon(n₁)` for 64 ≤ `n₁` < `n₂` at `W = 4` (64 = 16 W), and the same at
     `W = 1` from 16 blocks;
  3. **Page cache is reclaimable:** the 2.4 G-row run completes under `--mem 4Gi` (its output is ~18 GB, so
     unreclaimable staging would OOM), peak `memory.stat` `shmem` stays under 16 MiB, and `anon` agrees with
     `RssAnon` within `s_rss`;
  4. **The consumer's budget:** peak `RssAnon` < 8 GiB at 2.4 G rows (expected well under 1 GiB);
  5. **Bytes:** the `content_hash` at 268 M rows is the same for `W = 1` and `W = 4`;
  6. **Parallelism:** at 268 M rows `W = 4` is faster than `W = 1` by at least 1.5× wall time. If it isn't, the disk
     reader is the bottleneck the note's harness had (note §4.7), and the PR stops with a design question rather than
     merging a serial pipeline.
- **Recorded** in the ADR-0026 amendment (a table like its 2026-10-26 one) and the FEATURE-MATRIX row, with the job
  names and exit codes.

The in-tree test (PR F, `tessera-io/tests/staged_job_memory.rs`, counting allocator) asserts the heap ceiling
`peak(n, W) ≤ min(W, n) · p_block + s` at `W = 2`, `n ∈ {4, 16}` and `W = 1`, `n ∈ {1, 4}`, through a
`with_block_rows` test seam on `push_table` (as `TableMultiBlockSink` and GE ingest have) at
`block_rows = 4 · ROWS_PER_GROUP` so it stays cheap. It does not assert `τ`: at test-size
blocks the `c · rows` term is too small against `p_block` to resolve a 5% plateau. The plateau is the ksb proof's
job, at real `BLOCK_ROWS`.

### §9 — Amendments (exact text, applied in this PR)

1. **SPEC §5b** (`tessera/docs/SPEC.md`): the "Block partition" naming paragraph and the "Logical reads"
   paragraph are replaced by the text now in that file, with a dated note "Amended by ADR-0064 (2026-10-30)". It
   specifies `<prefix>/{index:04}`, the partition marker, the resolution rule and the legacy flat form for readers.
2. **ADR-0026**: a dated amendment ("Amended by ADR-0064 (2026-10-30) — streamed tables are partitioned") retires
   "one logical block stays one canonical block" for tables above `BLOCK_ROWS`, replaces the #677 amendment's
   *Known limit* with §8's bound, and points `table_job_streaming`'s contract to §5.4.
3. **ADR-0062**: the `RowAlignment.block` doc comment reads "The source's logical table name, resolved as in
   ADR-0064 §3 (`events` = `events/NNNN`, or legacy `events_NNNN`)", and `source_digest` is "in logical order",
   with a dated note under the struct. Nothing else in ADR-0062 changes.
4. **ADR-0063**: a dated amendment defines a numbered-leaf node: blocks stay the leaves of the name tree; a node
   whose children carry ADR-0064 partition markers is one logical table; its children are only its leaves and their
   `.cidx` sidecars. The leaf rule itself doesn't change.

### §10 — Rejected options (from the #681 note)

- **P, flat `pairs_NNNN`** (note §5, §5.2). Same partition and memory, but the group is a `_NNNN` convention in a flat
  name space: a user's `pairs_0003` silently joins it (nit 2), and nothing in the manifest says which blocks a writer
  meant. ADR-0063 already gives tessera a name tree in which a node is the grouping; P would leave it unused for the
  one case it fits best.
- **R, Vortex row groups in one block** (note §5, R; §5.2). Keeps one canonical block, but doesn't bound the RSS: a
  0.014 B/row footer term remains (34 MB at the consumer's 2.4 G rows). It needs a Vortex patch the maintainers have
  not agreed to, changes the canonical bytes of every large table, and encodes on one writer.
- **F, the writer gate alone** (note §2, "What does not bound it"). Deadlock-free and byte-identical, and the right
  upstream fix for sink starvation, but it can't remove the order debt `D(width)` or `c · rows`; a single canonical
  block stays unbounded. It goes upstream separately after the vortex#10046 conversation (note §6.3) and would lower
  `p_block` when it lands; #677 doesn't wait for it.

### §11 — The #681 nits, resolved

| Nit (recorded on #681) | Resolved in |
| --- | --- |
| 1. P-logical's cost includes "writers switch": every multi-block writer renamed, hashes change beyond #677, corpus goldens move; "readers don't change" only for P | §6 (cost table, the path list, "readers don't change" withdrawn), §4 (migration) |
| 2. Grouping isn't unambiguous; a mixed `pairs_NNNN` / `pairs/NNNN` product | §2 (sealed marker, validation, seal-time refusal), §3 (typed errors), §4 (mixed products) |
| 3. Disk staging's page cache counts toward `memory.current`, and is RAM on tmpfs | §8 (documented tmpfs term), §8.1 (samples `RssAnon`, `memory.current` and `memory.stat`; `statfs` check; runs under `--mem 4Gi`) |
| 4. `staged_peak` counts complete blocks only; an explicit `s` and tolerance | §8 (`s` explicit, `τ = 5 %` justified, `peak_staged_input_*` counts the partial block, the barrier test) |
| 5. ADR-0062's `RowAlignment` doc (`events = events_NNNN`) | §7, §9.3 (amended in this PR) |

## Implementation plan

Ordered; each PR is one concern, at most ~400 diff lines, with the test that fails before it. Each is a sub-issue of
#677 (labelled `feature`, the repository's label for enhancements).

| PR | Issue | Concern | Acceptance (the test fails before the PR) |
| --- | --- | --- | --- |
| A | #709 | `tessera-core`: `Partition`, `TableSpec.partition`, `LogicalTableError`, the shared group validation and legacy rule (§2, §3), seal-time checks in `ProductBuilder::seal` | a sealed `pairs/0000..0002` (marked) plus an unmarked `pairs/0003` is refused (`ForeignChild`); gap, duplicate, missing terminal, after-terminal, non-canonical `pairs/00003`, mixed `pairs_0000` + `pairs/0000` (`Ambiguous`), a marker on a top-level block (`StrayMarker`) and a one-leaf partition (`SingleLeaf`) each give their variant; a marked leaf named in full (`pairs/0003`) resolves `Single`; a legacy `events_0000..0002` product with an unrelated unmarked `events/notes` blob resolves `events` as legacy flat; index 10000 is named `10000` and orders after 9999; a single-block spec serializes byte-identically (corpus untouched) |
| B | #710 | `tessera-io` readers: `Reader::table_group`, `logical_table` over it, `block_group` deprecated, `WriteSession::seal`, `unpack` and `pack_dir` checks (`container.rs:927,1012`); a committed legacy fixture written by the pre-switch writer | `logical_table("pairs")` over a marked `pairs/NNNN` product returns the concatenation (fails before: "no blocks for prefix"); `.cidx` pruning skips leaves; the legacy fixture `tests/fixtures/legacy_flat_events.tsra` (3 `events_NNNN` blocks, pinned `content_hash`) reads identically before and after |
| C1 | #711 | CLI `ls`/`tree`/`read`/`sql` on logical names (§7), trycmd cases | `tsra ls FILE` shows one `pairs/` line, `ls FILE pairs` lists leaves, `tree` groups them, `read`/`sql` answer `pairs` (each fails before on a partitioned fixture) |
| C2 | #717 | Python `read_table*`/`column`/`table*` on logical names, `read_block` on a node (§7); `docs/book` bindings, `api_drift` | `tessera.open(p).table("pairs")` returns every row and `read_block("pairs")` is the typed error (both fail before); the bindings doc and `api_drift` pass |
| D | #712 | Writers switch: `table::block_name` → `<table>/{idx:04}` + marker; GE whole-file and `TableMultiBlockSink` through it (and #538's lanes if landed); the §6 tests move; `feat(io)!` | a multi-block GE ingest and a `TableMultiBlockSink` product have leaves `events/0000…` with markers and pass the shared validation (fails before: `events_0000`); streamed and whole-file GE ingest still seal the same `content_hash`; the legacy fixture still reads |
| E | #713 | `StreamWriter::push_table` (§5): per-block disk staging, dispatch on fill, names known from `spec.rows`, `peak_staged_input_*`; `table_job_streaming` refuses above `BLOCK_ROWS`, deprecated | the barrier test of §8 (`peak_staged_input_blocks == W + 1`, partial block counted); `push_table` seals the same leaves and `content_hash` as `TableMultiBlockSink` for `W ∈ {1, 2, 4}`, including nullable columns; a table of ≤ `BLOCK_ROWS` rows is byte-identical to `table_job_streaming`'s |
| F | #714 | The heap model in-tree (`tests/staged_job_memory.rs`), `push_table` rustdoc bound, the ADR-0026 *Known limit* replaced by §8 | `peak(n, W) ≤ min(W, n) · p_block + s` at `W = 2`, `n ∈ {4, 16}` and `W = 1`, `n ∈ {1, 4}`; shown failing once against a `push_table` that fills blocks in memory |
| G | #715 | The ksb RSS proof (§8.1): job flake and sampler script, results recorded in ADR-0026 and FEATURE-MATRIX | all six §8.1 criteria pass on ksb; the job exits non-zero on any failed criterion or a tmpfs staging dir |
| H | #716 | Tell the consumer: a #677 comment and a MorePET/visionx-rs issue with the `push_table` switch, the new leaf names, the logical read APIs and the release carrying them; #677 closes after G and H | the visionx-rs issue is linked from #677 and names the tessera release |

D lands only after B, C1 and C2, so no writer on `dev` writes a form some reader on `dev` can't read. E builds on D's
naming. F and G need E. H follows G.

## Consequences

- The consumer gets `pairs` as one name with a bounded RSS: measured 128-143 MB of heap from 67 M to 1.07 G rows at
  `W = 4` (note §4.7), to be proven as RSS by §8.1.
- Multi-block products written after PR D have other names and hashes than before (§6). Old products read unchanged.
- The logical table becomes a first-class, validated object of the format rather than a reader convention.
- A table's leaves are a directory `blocks/pairs/` in an exploded product and an S3 prefix `…/blocks/pairs/`,
  mirroring the names as ADR-0063 intends.
- Disk: up to `(W + 1)` blocks of raw input are staged (`5 × 4,194,304 × 24 B` = 503.3 MB at `W = 4` for the
  consumer shape; the note's harness measured 402.7 MB, counting complete blocks only, note §4.7),
  on top of ADR-0026's staged output and fragments.

## References

`docs/spikes/681-vortex-writer-backpressure.md` (PR #692, `b6de2d1`) · #677 (owner decision, 2026-10-29) · #681 (the
nits) · ADR-0026 · ADR-0052 · ADR-0062 · ADR-0063 · vortex-data/vortex#10046 · MorePET/visionx-rs PR #41.
