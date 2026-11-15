# ADR-0064 — Logical tables over numbered leaves: `<table>/NNNN`, a sealed partition marker, one logical reader

**Status:** Accepted (2026-11-02), by the architect after a fresh review; proposed 2026-10-30. Implements the
owner's decision on
[#677](https://github.com/vig-os/tessera/issues/677) (comment of 2026-10-29: option **P-logical** of the
[#681](https://github.com/vig-os/tessera/issues/681) design note, `docs/spikes/681-vortex-writer-backpressure.md`,
merged in PR #692 at `b6de2d1`) and specifies it so that the implementation can be briefed from this ADR
alone. Resolves the five re-review nits recorded on #681 (§11). Amends SPEC §5b, ADR-0026, ADR-0062
(`RowAlignment` only) and ADR-0063 (§9). Code references are at `origin/dev` `4504efb` (re-checked at `41b62c3`,
which changes no crate), under `tessera/crates/`, unless they say otherwise. **Amended by #760 (2026-11-04):** §8's
bound gains a per-leaf term `c_leaf · n`, §8.1's plateau compares medians of 3 runs at `W = 4`, and parallelism is
measured in its own job on `/dev/shm`, the only graded timing: shared-disk timings are context, never evidence
(exo-pet/exo-fleet#980; the amendment at the end). **Amended by #770 (2026-11-08):** §8.1's plateau is a slope
test, an OLS fit of every run's peak `RssAnon` against `n` from `16 W` leaves on, whose slope must be at most
`c_leaf` plus its one-sided 95 % confidence margin, with a resolution check; `W = 1` runs `k₁ = 6` times per size
from 16 leaves, so that its check resolves a growth of `c_leaf` beyond `c_leaf`; `τ` is retired (the second amendment
at the end). **Amended by #797 (2026-11-11):** criterion 2's slope is fitted within runs, by fixed effects over the
tenths of each run's input from `16 W` leaves on, with a standard error cluster-robust by run and `t` at `G − 1`
degrees of freedom; the checks' form, their targets and "inconclusive fails" are unchanged, the pre-#759 writer still
fails, and the across-run slope is reported as context confounded by the shared disk (the third amendment at the end).

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
   from the block count, so naming block `i` needs nothing but `i`. So the encoding is deterministic: the same
   rows, partitioned by the same `BLOCK_ROWS`, always give the same names and markers, and so the same manifest
   (§6 says which hash binds what).
3. **There is no overflow.** Past 9999 the leaf grows to five digits (`10000`), and so on. The only bound is
   ADR-0063's 255-byte name limit, which a `u64` index (at most 20 digits) can't reach for any `<table>` of
   at most 234 bytes; a longer `<table>` is refused by the grammar at the writer, as any over-long name is.
4. **Order is the index, not the string.** Past 9999 the lexicographic order of the names (`10000` sorts
   before `9999`) is not the logical order. The leaves appear in the manifest in increasing index order, which
   the seal and every reader check (§2.5), so the manifest order *is* the logical order. No reader sorts names,
   and a zip or S3 listing past 9999 is merely displayed out of order.
5. **Canonical leaf form.** A leaf whose segment parses as index `i` but is not exactly `format!("{i:04}")`
   (`pairs/00003`, `pairs/3`) is non-canonical: readers refuse it inside a partitioned node (§3), writers never
   produce it. One index has one name and the leaves have one order (§2.5), so one partition has one manifest,
   for a hand-sealed product as much as for a writer's.
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
   - **Children.** Every other block under `<table>/`, at any depth, belongs to a **registered child class**
     (below). Anything else (an unmarked table leaf, an array, a blob, a deeper name such as `pairs/0000/x`) is a
     **foreign child**.
   - **Indices.** The indices are unique and form `0..=k` with no gap, and `k ≥ 1`: a partitioned table has at
     least two leaves, because a table of one block is the bare `<table>` (§1.6). A one-leaf partition is refused
     (`SingleLeaf`), so one table has one encoding.
   - **Terminal.** Exactly one leaf has `last: true`, and it is the leaf with index `k`.
   - **Schema.** Every leaf has the same column count, names and dtypes (the check
     `multiblock.rs:396-415` makes today).
   **The registered child classes** are one closed list in `tessera-core` (`PARTITION_CHILD_CLASSES`, PR A), each
   entry a name pattern relative to the node plus the block kind and spec it requires. This ADR registers exactly
   one:

   | Class | Name | Kind and spec | Since |
   | --- | --- | --- | --- |
   | chunk-index sidecar | `<table>/<leaf>.cidx` for a leaf that exists (`tessera-io/src/chunk_index.rs:21-23`) | `chunk_index`, `indexes` = that leaf | ADR-0064 |

   Adding a class (for example #719's write-time aggregation views as `pairs/<view>`) is an amendment of this ADR
   that adds a row, with a name pattern no canonical leaf can match (a canonical leaf segment is all digits). This
   ADR doesn't assume how #720 classifies derived blocks for identity (a `.cidx` spec already says
   `"class": "derived"`, `tessera-io/src/chunk_index.rs:42`; whether such blocks belong in `content_hash` is #720's
   open question). The cost is stated: a reader older than the new row sees the new child as foreign and
   refuses the logical name (`ForeignChild`), while every leaf stays readable by its full name. That is the safe
   failure, and the reason the list is closed rather than "anything not marked is ignored": an ignored child is
   how a user's `pairs/0003` would slip back in (nit 2).
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
5. **Manifest order.** The leaves MUST appear in the manifest in strictly increasing index order. Other blocks
   (a leaf's `.cidx`, for one) may sit between them. Writers emit them so; the seal (§2.4) and every
   reader (§3) check it, and a reader refuses an out-of-order node with `OutOfOrder`. Checking it on read, not only
   at seal, is what makes §1.5 true of hand-sealed products: if readers sorted by index instead, two manifests that
   differ only in leaf order (and so in `manifest_hash`, §6) would resolve to the same table, and one partition
   would have two manifests. Sorting was rejected for that reason; the check costs one comparison per leaf.
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
- `Partitioned { leaves }`: `name` is a valid partitioned node (§2.1, §2.5), leaves in index order (which is
  their manifest order);
- `LegacyFlat { blocks }`: blocks matching the legacy `_NNNN` rule (§4), in manifest order.

The rule, in order:

1. **Collect the candidates.** `marked` = the blocks exactly one segment below `name` (§2.1) that carry a
   marker. `flat` = the blocks `block_group` matches today: `name` itself plus every `name_` + four ASCII digits
   (§4).
2. **`marked` and `flat` both non-empty: refuse** with `Ambiguous { flat, node }` (§4). A conforming writer never
   produces this, and the seal refuses it (§2.4), so it comes only from a hand-sealed product.
3. **`marked` non-empty:** validate the node (§2.1, §2.5) and return `Partitioned`, or the typed error.
4. **`flat` non-empty:** return `Single` when it is the one block `name`, else `LegacyFlat`, exactly as today.
   Unmarked blocks under `name/` don't take part: a legacy product with `events_0000…0002` and an unrelated
   `events/notes` blob (valid under the leaf rule) resolves `events` as it does today. If any block of `flat` whose name has no
   parent node carries a marker, the result is `StrayMarker` (§2.2), for `Single` and `LegacyFlat` alike, so a
   reader refuses what the seal refuses; a marked leaf named in full (`pairs/0003`) is `Single`.
   *Clarified (#733, 2026-11-02):* a marked block of `flat` that has a parent node (`a/pairs_0001`) is checked
   the way the seal checks it: its parent node `a` is validated (§2.1), and its error is the result
   (`LogicalTable { name: "a", .. }`; `pairs_0001` is never a canonical leaf, so `a` is always refused). A marked
   block is refused by the same rule on both paths. The only exemption is a leaf named in full that is the whole
   group: next to a `pairs/0001_0000` it is checked too, and that block is a foreign child of `pairs`.
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
    OutOfOrder { leaf: String, after: String },
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
  not this ADR's. A legacy product is never rewritten in place. Re-ingesting its source gives the new layout:
  a new `manifest_hash` and the same `content_hash` (§6).
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
| GE whole-file ingest above `BLOCK_ROWS` (`ge_hdf5.rs:1007`) | `events_NNNN` | `events/NNNN` + marker | names, block specs, `manifest_hash`; **not** `content_hash` |
| `TableMultiBlockSink` (`accumulate.rs:575`): GE streaming ingest, `bench --input` | `<prefix>_NNNN` | `<prefix>/NNNN` + marker | same |
| Generic `ingest table` (PR #538, if landed first) | `table_NNNN` | `table/NNNN` + marker | same |
| `table_job_streaming` above `BLOCK_ROWS` (alpha.4 contract) | one block, any size | typed error; `push_table` writes leaves | the consumer's output (§5.4) |
| Products already on disk (alpha.1 to alpha.4) | `_NNNN` | read unchanged (§4) | nothing |

- **`manifest_hash` binds the leaf names and the partition marker; `content_hash` does not (normative).**
  - `ProductBuilder::seal` builds `content_hash` as the Merkle root of the block digests, in manifest order, and
    nothing else (`tessera-core/src/product.rs:174-182`), and a table block's digest is the blake3 of its Vortex
    payload bytes (`tessera-io/src/table.rs:1661`). A block's name and its `spec`, the marker included, are not in
    the payload. A `.cidx` sidecar records the leaf it indexes in its spec (`indexes`,
    `tessera-io/src/chunk_index.rs:49`), not in its payload. `manifest_hash` is computed last, over the whole
    manifest, block names and specs included (`product.rs:207-210`).
  - So the writer switch of PR D moves **`manifest_hash` only**. Every product above `BLOCK_ROWS` rows written
    after PR D has new names and markers and so a new `manifest_hash`; its payloads, digests and manifest order are
    the batch partition's as before, so its `content_hash` is **unchanged** for the same rows. Single-block products
    move in neither. This holds whatever #720 decides about derived blocks in `content_hash`, since neither a name
    nor a marker reaches any payload.
  - **What it means for identity.** The same rows under `events_NNNN` and under `events/NNNN` are the **same
    content** (equal `content_hash`, "changes iff payload bytes change", `tessera-core/src/manifest.rs:6-7`) but
    **different manifests**, so different versions: ADR-0036's model keeps `id` as the lineage handle and
    `manifest_hash` as the version (`docs/adr/0036-versioning-audit-object-store.md:92`); ADR-0028 §4.1 defines `content_hash` as the root over
    the flat payload digests, so that encoder choices which keep the bytes keep the content
    (`docs/adr/0028-unified-hierarchy.md:88-101`), and naming is such a choice; and ADR-0052 keeps the
    seal (`manifest_hash`) the format identity that a deliberate format change moves. A provenance edge already
    pins its parent by `manifest_hash` (`Source.content_hash` holds the parent's seal, SPEC §8,
    `tessera-core/src/provenance.rs:20-23`), so a downstream product tells the two layouts apart. `content_hash`
    equality says the rows are the same, never that the logical tables are named the same.
  - **Consequence for tests.** A test that compares only `content_hash` can't catch a writer that misnames a leaf
    or drops its marker. Every writer-equivalence test in the plan therefore compares names and markers as well as
    `content_hash`, and is shown failing once against a writer that misnames a leaf and once against one that drops
    the marker. Which comparison depends on what the two sides share:
    - **Same session inputs** (PR E's `push_table` against `TableMultiBlockSink`, §8.1 criterion 5's `W = 1`
      against `W = 4`: one binary, the same description, sources and column specs): the **canonical manifests**
      are equal with `producer`, `timestamp` and what follows from them (`id`, `id_inputs`, `manifest_hash`)
      removed, so every block's name, kind, digest and spec is compared.
    - **Different ingest paths** (PR D's streamed against whole-file GE ingest): the manifests legitimately
      differ outside the partition, in `description` (`tessera-ingest/src/ge_hdf5.rs:764` against `:982`), in
      `sources` (a file `content_hash` on the streamed edge, `tessera-ingest/src/provenance.rs:33-35`, a bare
      `Source::new` on the whole-file one, `ge_hdf5.rs:1012`) and in the column annotations (`annotate_columns`,
      `:750`, against name and dtype only, `:963-972`). So that test compares a **block projection**: per block, in
      manifest order, `name`, `kind`, `digest`, `spec.rows`, `spec.row_index` and `spec.partition`, plus
      `content_hash`. Aligning the two paths' description, sources and annotations is outside this ADR.
  - Nit 1 on #681 had the same wrong premise ("hashes change beyond #677"); it is corrected here (§11).
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

  **`block_group`'s callers.** PR B marks `Reader::block_group` `#[deprecated]` (§7), and CI runs clippy with
  `-D warnings`, so the same PR handles every caller at `4504efb`:
  - `tessera-io/src/multiblock.rs:377` (`logical_table`, production): migrated to `table_group`;
  - `tessera-io/src/multiblock.rs:524` (logical-view test): migrated to `table_group`;
  - `tessera-io/src/container.rs:1912` (legacy grouping): kept, as a test of the deprecated legacy rule, under a
    test-local `#[allow(deprecated)]`;
  - `tessera-io/src/accumulate.rs:1049` (`TableMultiBlockSink` manifest order): kept under a test-local
    `#[allow(deprecated)]` in PR B, while the sink still writes `events_NNNN`; PR D renames the sink's output to
    `events/NNNN`, which `block_group` doesn't match, so it moves with PR D to `table_group` (it is in the list
    above);
  - `tessera-io/tests/block_names.rs:415` (an old-style pyramid's `volume` is a single block): migrated to
    `table_group`, asserting `Single`. *Erratum (#710, 2026-11-02):* that `volume` is an **array** block, so §3
    resolves it to `NotATable { kind: "array" }` (`Single` is a table block), and the migrated test asserts that.

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
  - **Known limit: old binaries edit without the §2.4 checks.** alpha.1 to alpha.4 keep a block's `spec`
    verbatim when they re-seal: `ProductBuilder::from_manifest` clones the parent's `BlockRef`s
    (`tessera-core/src/product.rs:61`, the same line at `v0.1.0-alpha.1` and `v0.1.0-alpha.4`) and
    `commit --add-block` copies the source block's `spec` (`tessera-cli/src/version.rs:166`), so `commit --set`
    does not drop a marker. What an old binary lacks is the seal-time validation: `commit --remove-block
    pairs/0002`, `commit --add-block pairs/0003=…` of an unmarked block, or a renaming `--add-block` that copies a
    leaf's marker under another index, seals a product whose `pairs` node is invalid. A new reader then refuses
    the logical name with `Error::LogicalTable { name: "pairs", reason }`, `reason` being `Gap { missing: 2 }`
    (`NoTerminal` if the removed leaf was the last), `ForeignChild { child: "pairs/0003" }` or `NonCanonicalLeaf`
    respectively, and every block
    stays readable by its full name. No wrong rows come out, so this is accepted and documented, not prevented.
    The same refusal meets any tool that rebuilds a leaf's spec through a `TableSpec` that lacks the field (a
    library user re-encoding a leaf with an older `tessera-core`): the marker is dropped, the leaf is a foreign
    child, or, with every marker gone, `pairs` is `NotATable { kind: "node" }`. The fix is to re-seal with a
    current binary from the original leaves.
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
| `Reader::block_group` | `tessera-io/src/container.rs:438-461`, legacy flat only | unchanged; `#[deprecated]`, pointing to `table_group`; its callers per §6 | B |
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
| ADR-0062 streaming provider, `aligned_to`, zip | not built (ADR-0062 Accepted, `cf3f980`) | designed on `LogicalTableView`; resolves through `table_group` | (#666, #667) |
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

*(Amended by #760, below: the first line is `peak heap ≤ min(W, n) · p_block + s + c_leaf · n`, with
`c_leaf = 2 · (e_leaf + b_leaf)` for the manifest entry each committed leaf keeps until the seal.)*

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

*(Amended by #760, below: the ceiling gains `c_leaf · n`, and the plateau is
`peak(n₂) ≤ (1 + τ) · peak(n₁) + c_leaf · (n₂ − n₁)`, with `τ` unchanged.)*
*(Amended by #770, below: the plateau is a slope test, the slope of peak `RssAnon` against `n` over every run at
`n ≥ 16 W` at most `c_leaf` plus its one-sided 95 % confidence margin; `τ` is retired.)*

`τ = 5 %` is six times the measured plateau variation (0.8%) and under a third of the smallest regression it must
catch: the zone-statistics half of `c · rows` alone, reappearing across the whole table, adds 0.029 B/row, which is
23 MB (+16%) from 268 M to 1.07 G rows; the full 0.058 B/row adds 47 MB (+33%) (note §4.2). A tolerance above ~15%
would let the zone-stats regression through; one at the measured 0.8% would fail on scheduler noise. `τ` and `s` may
be tightened by the proof; loosening either needs an amendment of this ADR. The proof also records, for every run,
the input row-group at which the peak fell, so the 16-to-64 rise gets an explanation or a bound of its own before
`τ` is tightened. *(Amended by #770, below: `τ` is retired. The slope test's level and its resolution target, the
0.029 B/row zone-statistics half, take its place; loosening either needs an amendment.)*

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
  row-group at its `RssAnon` peak. *(Amended by #760, below: each `W = 4` point at 64 blocks or more runs 3 times.)*
- **Sampling.** Every 10 ms: `RssAnon` from `/proc/<pid>/status`; `memory.current` and `memory.stat`'s `anon`,
  `file` and `shmem` from the job's cgroup. Before the first row: `statfs` of the session directory is not
  `TMPFS_MAGIC` (`0x01021994`), else the job fails. *(Amended by #760, below: this guard is the criteria 1–5
  job's; criterion 6 runs in its own job on tmpfs, with the converse guard.)*
- **Pass criteria.** With `R₀` the peak `RssAnon` at 0 rows (binary, runtime) and `R₁` at one block, same binary:
  1. **Ceiling:** `RssAnon(n, W) ≤ R₀ + min(W, n) · (R₁ − R₀) + s + s_rss` at every point, 16 blocks included, with
     `s_rss = 32 MiB + 8 MiB · (W + 1)` for allocator retention (one glibc arena's retained top per thread) and
     thread stacks the heap model doesn't see;
     *(Amended by #760, below: plus `c_leaf · n`, checked on every run.)*
  2. **Plateau:** `RssAnon(n₂) ≤ 1.05 · RssAnon(n₁)` for 64 ≤ `n₁` < `n₂` at `W = 4` (64 = 16 W), and the same at
     `W = 1` from 16 blocks;
     *(Amended by #760, below: plus `c_leaf · (n₂ − n₁)`, on the medians of 3 runs at `W = 4`.)*
     *(Amended by #770, below: a slope test over every run at `n ≥ 16 W`, at both `W`.)*
     *(Amended by #797, below: the slope is fitted within runs, over the tenths of each run's input, C.2.)*
  3. **Page cache is reclaimable:** the 2.4 G-row run completes under `--mem 4Gi` (its output is ~18 GB, so
     unreclaimable staging would OOM), peak `memory.stat` `shmem` stays under 16 MiB, and `anon` agrees with
     `RssAnon` within `s_rss`;
  4. **The consumer's budget:** peak `RssAnon` < 8 GiB at 2.4 G rows (expected well under 1 GiB);
  5. **Bytes and names:** at 268 M rows the `W = 1` and `W = 4` products have equal canonical manifests (with
     `producer`, `timestamp`, `id`, `id_inputs` and `manifest_hash` removed, §6) and equal `content_hash`, so a
     `W`-dependent naming or marker difference fails it, not only a wrong payload (the naming itself is PR D's and
     PR E's direct check);
  6. **Parallelism:** at 268 M rows `W = 4` is faster than `W = 1` by at least 1.5× wall time. If it isn't, the disk
     reader is the bottleneck the note's harness had (note §4.7), and the PR stops with a design question rather than
     merging a serial pipeline. *(Amended by #760, below: measured in a separate job with staging on `/dev/shm`;
     the shared-scratch throughput is recorded as context with the disk's `%util`.)*
- **Recorded** in the ADR-0026 amendment (a table like its 2026-10-26 one) and the FEATURE-MATRIX row, with the job
  names and exit codes.

The in-tree test (PR F, `tessera-io/tests/staged_job_memory.rs`, counting allocator, the consumer's 24 B/row
schema, a `with_block_rows` test seam on `push_table` as `TableMultiBlockSink` and GE ingest have) has two parts. The
input is generated lazily, one freshly allocated row-group per pull, so the generator holds no more than the group
the reader is serializing. With `ROWS_PER_GROUP` = 65 536 and `r_mem` = 24 B, `s_stage` = 3 MiB and `s` = 7 MiB.

1. **Held heap (the fail-before test).** At `block_rows = 4 · ROWS_PER_GROUP` (one block of input is
   `A = 262 144 × 24 B = 6 MiB`) and `W = 2`, all workers are held at PR E's barrier before they read anything, and
   rows are offered until the reader stalls. The test asserts `heap_while_held − heap_before ≤ s`. No encode runs
   while the workers are held, so the heap is the input the writer keeps in memory, and `p_block` doesn't enter:
   - the correct writer holds at most the row-group it is serializing (1.5 MiB) plus its fixed bookkeeping, `≤ s`;
   - a writer that fills blocks in memory holds the `W` dispatched blocks and the full one waiting for a worker,
     `(W + 1) · A = 18 MiB`, which exceeds `s` by 11 MiB. It fails.
2. **Ceiling.** `peak(n, W) ≤ min(W, n) · p_block + s` at `W = 2`, `n ∈ {4, 8}` and `W = 1`, `n = 4`, with
   `p_block` the peak of the `W = 1`, `n = 1` write measured in the same binary (it depends on `P`, §8). That run is
   the **calibration** `p_block` is measured from, not a pass/fail point: it would compare `p_block` with itself and
   could never fail. Every run uses
   `block_rows = 16 · ROWS_PER_GROUP` (`A = 1 048 576 × 24 B = 24 MiB`; 8 blocks are 8.4 M rows). The block size is
   chosen so that this part fails too. A writer that fills blocks in memory inflates its own `p_block` by one block,
   `A`, since it holds the block's arrays through the encode; at `n ≥ W + 1` its peak is
   `min(W, n) · (p_enc + A) + A`, the `+ A` being the block it fills while `W` encode. Against the bound
   `min(W, n) · (p_enc + A) + s`, it exceeds by `A − s` = 24 − 7 = **17 MiB**. At the old
   `block_rows = 4 · ROWS_PER_GROUP` the excess was `6 − 7 = −1 MiB`, so a ceiling check alone at that size could
   never fail, which is why part 1 exists and part 2 uses the larger block.

*(Amended by #760, below: the test doesn't add the `c_leaf · n` term; at its 8 leaves the term is at most
25.3 KB, 0.34 % of `s`.)*

It does not assert `τ`: at test-size blocks the `c · rows` term is too small against `p_block` to resolve a 5%
plateau. The plateau is the ksb proof's job, at real `BLOCK_ROWS`. *(Amended by #770, below: `τ` is retired and
the plateau is a slope test; it is still the ksb proof's job.)*

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
   whose children carry ADR-0064 partition markers is one logical table; its children are only its leaves, in
   increasing index order, and children of a class §2.1 registers (today their `.cidx` sidecars). The leaf rule
   itself doesn't change.

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
| 1. P-logical's cost includes "writers switch": every multi-block writer renamed, hashes change beyond #677, corpus goldens move; "readers don't change" only for P | §6 (cost table, the path list, "readers don't change" withdrawn), §4 (migration). The nit's premise is corrected: only `manifest_hash` changes beyond #677, never `content_hash`, which covers payload digests only (`product.rs:174-182`, `table.rs:1661`); no corpus golden moves |
| 2. Grouping isn't unambiguous; a mixed `pairs_NNNN` / `pairs/NNNN` product | §2 (sealed marker, validation, seal-time refusal), §3 (typed errors), §4 (mixed products) |
| 3. Disk staging's page cache counts toward `memory.current`, and is RAM on tmpfs | §8 (documented tmpfs term), §8.1 (samples `RssAnon`, `memory.current` and `memory.stat`; `statfs` check; runs under `--mem 4Gi`) |
| 4. `staged_peak` counts complete blocks only; an explicit `s` and tolerance | §8 (`s` explicit, `τ = 5 %` justified, `peak_staged_input_*` counts the partial block, the barrier test) |
| 5. ADR-0062's `RowAlignment` doc (`events = events_NNNN`) | §7, §9.3 (amended in this PR) |

## Implementation plan

Ordered; each PR is one concern, at most ~400 diff lines, with the test that fails before it. Each is a sub-issue of
#677 (labelled `feature`, the repository's label for enhancements).

| PR | Issue | Concern | Acceptance (the test fails before the PR) |
| --- | --- | --- | --- |
| A | #709 | `tessera-core`: `Partition`, `TableSpec.partition`, `LogicalTableError`, `PARTITION_CHILD_CLASSES` (§2.1), the shared group validation (manifest order included, §2.5) and legacy rule (§3), seal-time checks in `ProductBuilder::seal` | a sealed `pairs/0000..0002` (marked) plus an unmarked `pairs/0003` is refused (`ForeignChild`); gap, duplicate, missing terminal, after-terminal, non-canonical `pairs/00003`, mixed `pairs_0000` + `pairs/0000` (`Ambiguous`), a marker on a top-level block (`StrayMarker`), a one-leaf partition (`SingleLeaf`) and leaves listed `0001` before `0000` (`OutOfOrder`) each give their variant; a `pairs/0000.cidx` sidecar is accepted and a `pairs/view` array is `ForeignChild`; a marked leaf named in full (`pairs/0003`) resolves `Single`; a legacy `events_0000..0002` product with an unrelated unmarked `events/notes` blob resolves `events` as legacy flat; index 10000 is named `10000` and orders after 9999; a single-block spec serializes byte-identically (corpus untouched) |
| B | #710 | `tessera-io` readers: `Reader::table_group`, `logical_table` over it, `block_group` deprecated with its callers migrated or `#[allow(deprecated)]` (§6), `WriteSession::seal`, `unpack` and `pack_dir` checks (`container.rs:927,1012`); a committed legacy fixture written by the pre-switch writer | `logical_table("pairs")` over a marked `pairs/NNNN` product returns the concatenation (fails before: "no blocks for prefix"); `.cidx` pruning skips leaves; the legacy fixture `tests/fixtures/legacy_flat_events.tsra` (3 `events_NNNN` blocks, pinned `content_hash`) reads identically before and after |
| C1 | #711 | CLI `ls`/`tree`/`read`/`sql` on logical names (§7), trycmd cases | `tsra ls FILE` shows one `pairs/` line, `ls FILE pairs` lists leaves, `tree` groups them, `read`/`sql` answer `pairs` (each fails before on a partitioned fixture) |
| C2 | #717 | Python `read_table*`/`column`/`table*` on logical names, `read_block` on a node (§7); `docs/book` bindings, `api_drift` | `tessera.open(p).table("pairs")` returns every row and `read_block("pairs")` is the typed error (both fail before); the bindings doc and `api_drift` pass |
| D | #712 | Writers switch: `table::block_name` → `<table>/{idx:04}` + marker; GE whole-file and `TableMultiBlockSink` through it (and #538's lanes if landed); the §6 tests move; `feat(io)!` | a multi-block GE ingest and a `TableMultiBlockSink` product have leaves `events/0000…` with markers and pass the shared validation (fails before: `events_0000`); streamed and whole-file GE ingest seal equal block projections (per block `name`, `kind`, `digest`, `spec.rows`, `spec.row_index`, `spec.partition`, §6) and the same `content_hash`, and the `content_hash` equals the pre-switch writer's for the same rows; the comparison is shown failing once against a writer that misnames a leaf and once against one that drops the marker; the legacy fixture still reads |
| E | #713 | `StreamWriter::push_table` (§5): per-block disk staging, dispatch on fill, names known from `spec.rows`, `peak_staged_input_*`; `table_job_streaming` refuses above `BLOCK_ROWS`, deprecated | the barrier test of §8 (`peak_staged_input_blocks == W + 1`, partial block counted); `push_table` seals a canonical manifest equal to `TableMultiBlockSink`'s (minus the §6 fields) and the same `content_hash` for `W ∈ {1, 2, 4}`, shown failing once against a `push_table` that misnames a leaf and once against one that drops the marker, including nullable columns; a table of ≤ `BLOCK_ROWS` rows is byte-identical to `table_job_streaming`'s |
| F | #714 | The heap model in-tree (`tests/staged_job_memory.rs`), `push_table` rustdoc bound, the ADR-0026 *Known limit* replaced by §8 | the two parts of §8's in-tree test: held heap `≤ s` at `block_rows = 4 · ROWS_PER_GROUP`, `W = 2` (an in-memory fill holds 18 MiB against `s` = 7 MiB), and the ceiling `peak(n, W) ≤ min(W, n) · p_block + s` at `block_rows = 16 · ROWS_PER_GROUP`, `W = 2`, `n ∈ {4, 8}` and `W = 1`, `n = 4`, `p_block` calibrated from the `W = 1`, `n = 1` run, which is not a pass/fail point (an in-memory fill exceeds it by `A − s` = 17 MiB); both shown failing once against a `push_table` that fills blocks in memory |
| G | #715 | The ksb RSS proof (§8.1): job flake and sampler script, results recorded in ADR-0026 and FEATURE-MATRIX | all six §8.1 criteria pass on ksb; the job exits non-zero on any failed criterion or a tmpfs staging dir |
| H | #716 | Tell the consumer: a #677 comment and a MorePET/visionx-rs issue with the `push_table` switch, the new leaf names, the logical read APIs and the release carrying them; #677 closes after G and H | the visionx-rs issue is linked from #677 and names the tessera release |

D lands only after B, C1 and C2, so no writer on `dev` writes a form some reader on `dev` can't read. E builds on D's
naming. F and G need E. H follows G.

## Consequences

- The consumer gets `pairs` as one name with a bounded RSS: measured 128-143 MB of heap from 67 M to 1.07 G rows at
  `W = 4` (note §4.7), to be proven as RSS by §8.1.
- Multi-block products written after PR D have other names and another `manifest_hash` than before, and the same
  `content_hash` (§6). Old products read unchanged.
- The logical table becomes a first-class, validated object of the format rather than a reader convention.
- A table's leaves are a directory `blocks/pairs/` in an exploded product and an S3 prefix `…/blocks/pairs/`,
  mirroring the names as ADR-0063 intends.
- Disk: up to `(W + 1)` blocks of raw input are staged (`5 × 4,194,304 × 24 B` = 503.3 MB at `W = 4` for the
  consumer shape; the note's harness measured 402.7 MB, counting complete blocks only, note §4.7),
  on top of ADR-0026's staged output and fragments.

## Amendment (#760, 2026-11-04) — a per-leaf term, a median-of-3 plateau, parallelism on tmpfs

The first run of the §8.1 proof (#715: jobs `tessera-715-sweep-1` and `tessera-715-diag-1`, both exit 1, tessera
`a3df683`) passed criteria 1, 3, 4 and 5 and failed 2 and 6. The diagnosis,
`docs/spikes/715-rss-proof-diagnosis.md` (branch `test/715-ksb-rss-proof` at `252af87`, "the diagnosis" below),
found three causes, and the owner ruled on each on #715 (comment of 2026-11-04). This amendment applies the
rulings to §8 and §8.1. The text it replaces is kept above, marked *(Amended by #760, below)*. Nothing else in
§8 or §8.1 changes: `τ = 5 %`, `s`, `s_rss`, the ceilings, the 1.5× threshold, the `--mem 4Gi` and 8 GiB budgets
and criteria 3, 4 and 5 stand as written. *(Amended by #770, below: `τ` is retired, the rest stands.)*

### A.1 — §8: the bound gains `c_leaf · n`

**Why.** A write session keeps every committed leaf's manifest entry until the seal, which must write it
(`WriteSession::blocks`, `tessera-io/src/write.rs`), so the heap grows with the number of leaves, not only with
`min(W, n)`. §8 had no term for it. At `W = 1` the diagnosis measured 12.8 KB of live heap and ~25.4 KB of
`RssAnon` per leaf for the consumer's 12-column schema (§1 there). About 3.1 KB of that is the entry's content,
which has to reach the manifest. The rest is the entry's form, a `serde_json::Value` tree per leaf, and the
allocator retention around it. #759 replaces that form with a compact one.

**The bound, normative** (replaces §8's first block; the other lines are unchanged):

```text
peak heap      ≤  min(W, n) · p_block  +  s  +  c_leaf · n
s              =  s_stage + s_fixed
s_stage        =  2 · ROWS_PER_GROUP · r_mem
s_fixed        =  4 MiB
c_leaf         =  2 · (e_leaf + b_leaf)
b_leaf         =  1 KiB
staged input   ≤  W + 1 blocks, including the one being filled
staged bytes   ≤  (W + 1) · BLOCK_ROWS · r_stage
```

- **`e_leaf`** is the length in bytes of the leaf's entry (`name`, `kind`, `digest`, `spec`) in the sealed
  manifest's canonical JSON (RFC 8785, `tessera_core::canonical`), the largest over the table's leaves. It is
  computed from the schema or read off the sealed manifest, never measured on the heap. It depends on the schema:
  the column specs, their descriptions included, are repeated in every leaf. For the consumer's schema it is
  3,180 B (the 3,053 B spec the diagnosis measured, plus 127 B of name, kind, digest and keys).
- **`b_leaf`** covers the per-leaf bookkeeping outside the entry's bytes: the `blocks` slot
  (`size_of::<BlockRef>()`, up to twice that while the `Vec` grows) and the allocator's chunk headers and size
  rounding on the entry's allocations. The diagnosis measured 387 B of requested bytes on the `append_staged`
  stack (headers and rounding excluded), so 1 KiB covers it with room to spare. The diagnosis counted the Merkle
  accumulator's per-leaf state in that row; the accumulator holds only `O(log n)` MMR peaks
  (`tessera-core/src/hash.rs`), not one value per leaf, so it stays in `s_fixed`.
- **The factor 2 is the allocator-retention allowance:** `e_leaf + b_leaf` of retained but free memory per leaf,
  on top of the same amount live. glibc returns memory to the kernel only from the top of a heap, so a
  long-lived allocation placed among the encoder's large short-lived buffers keeps the freed space around it
  resident. That retention scales with the long-lived bytes, not with the encoder's buffers, which are freed
  and reused leaf after leaf. glibc gives no fragmentation bound, so this is a stated allowance, like `s_rss`,
  not a theorem. It is not fitted: the measured pre-#759 rate (25.4 KB per leaf) would be a fit, and would let
  the #759 defect through. Criterion 2 checks the allowance. A run that retains more fails it, and that is a
  finding about the writer, not a reason to raise the factor. Raising the factor or `b_leaf` needs an amendment,
  like `τ`. *(Amended by #770, below: criterion 2 checks the allowance to within the slope test's margin; the live
  part is checked exactly in-tree, B.3. `τ` is retired; raising the factor or `b_leaf` still needs an amendment.)*
- **For the consumer:** `c_leaf` = 2 · (3,180 + 1,024) B = 8,408 B. That is 4.8 MB at its 573 leaves, and
  0.002 B/row (one leaf is `BLOCK_ROWS` = 4,194,304 rows). *(Amended by #770, below: 3,180 B isn't the canonical
  entry length this bullet list defines; the sealed manifests of G r3 give `e_leaf` ≤ 2,966 B, so `c_leaf` =
  7,980 B, and every 8,408 B in A.1 and A.2 is 7,980 B by the definition, B.3.)*
- **It holds once #759 lands** (`WriteSession` no longer keeps a `Value` tree per leaf, and #759's counting-allocator
  test bounds the held heap per leaf by the serialized entry plus a stated constant; that constant is `b_leaf` = 1 KiB,
  so the test checks the live part `e_leaf + b_leaf`). Until then a leaf holds several times `e_leaf` (12.8 KB live for
  a 3.2 KB entry), so the term understates the writer on `dev`.
- **The seal.** At the seal no encode is in flight. The seal serializes the manifest, a transient of a few times
  `n · e_leaf`, 1.8 MB of JSON for the consumer's 573 leaves against its `p_block` of 46.1 MB.
- **`n`, not `n − min(W, n)`.** `p_block` is measured on a one-leaf write, so it already holds one leaf's entry,
  and the term counts up to `min(W, n)` entries twice. That is at most 4 × 8,408 B = 33.6 KB at `W = 4`, far
  below what the proof can resolve. The simpler form is kept as the conservative one.

**Size independence, restated.** The ceiling is `min(W, n) · p_block + s + c_leaf · n` at every `n`. The plateau
(replaces §8's second block) is flat beyond the per-leaf term:

```text
peak(n₂)  ≤  (1 + τ) · peak(n₁)  +  c_leaf · (n₂ − n₁)      for  16 W ≤ n₁ < n₂,   τ = 5 %
```

*(Amended by #770, below: the plateau is the slope of peak `RssAnon` against `n`, at most `c_leaf` plus its
confidence margin; `τ` is retired. The next paragraph's argument for `τ` is replaced by #770's power analysis,
B.4.)*

`τ` still catches the regression it was set for. The zone-statistics half of `c · rows` adds 23 MB from 268 M to
1.07 G rows at `W = 4` (§8). *(Amended by #770, below: 8,408 B is 7,980 B by A.1's definition, B.3.)* The allowance
over that step is `τ · 143 MB + 192 · 8,408 B` = 7.2 + 1.6 = 8.8 MB on the
heap; on `RssAnon`, which criterion 2 checks (199–237 MB at `W = 4`, the diagnosis §2), it is at most
`0.05 · 237 MB + 1.6 MB` = 13.5 MB, still under 23 MB. The per-leaf term is 0.002 B/row, about a fifteenth of the
0.029 B/row regression.

**The in-tree test (§8's PR F text).** `tests/staged_job_memory.rs` doesn't add the term, and it doesn't need to, at its
sizes. Its schema (the consumer's 12 columns, without descriptions) has `e_leaf` = 558 B (557 B on the terminal leaf,
whose `"last":true` is a byte shorter), so `c_leaf` = 2 · (558 + 1,024) B = 3,164 B.

- In part 1 (held heap), no leaf is committed while the workers are held, so the term is 0 there.
- In part 2 (ceiling) it is at most 8 × 3,164 B = 25.3 KB at `n = 8`. That is 0.34 % of `s` = 7 MiB, and 0.14 %
  of the 17 MiB by which a writer that fills blocks in memory fails.

So the test's bound `min(W, n) · p_block + s` is the amended bound minus at most 25.3 KB. That makes it stricter,
by a margin it can't resolve. Both parts keep their fail-before margins.

### A.2 — §8.1 criteria 1 and 2: the ceiling over every run, the plateau over medians of 3

**Why.** At `W = 4` the live encode peak varied by up to +14.4 % between identical runs (163.09–186.53 MB at 256 leaves,
the diagnosis §2; +14.9 % from 64 to 256 leaves within one job), with no consistent sign between 64 and 256 leaves.
*(Erratum (#715, 2026-11-09): this read "+14.9 % between identical runs (159.6–186.5 MB at 256 leaves)"; the
159.6 MB run had 64 leaves.)* The measured spikes were +5.8 % and +14.4 %. A
spike is the `W` encoders' own peaks happening to coincide at an arbitrary row-group, an event of one run, not a
function of `n`. A single-run plateau comparison at `τ = 5 %` therefore fails on noise three times `τ`, and loosening
`τ` would let the zone-statistics regression through (§8). A median of three runs discards one coincidence per point.
*(Amended by #770, below: in G r3 it didn't, B.1; the slope test replaces the medians.)* A systematic per-leaf term
shows in all three runs, and so in the median. At `W = 1` five identical runs of 64 leaves peaked within 0.72 % of
each other (48.81–49.16 MB of live heap), and two proof runs' `RssAnon` at 64 leaves within 0.8 % (59.5 and
60.0 MiB), so `W = 1` points run once. The ceiling is different: it is what one run must hold against a cgroup limit,
so it is checked against every run, not against a median that would hide the worst one. The highest `RssAnon`
measured, 226.0 MiB, was under the ceiling of 298.4 MiB.

**Sweep** (an addition to §8.1's *Sweep*): each `W = 4` point at 64 blocks or more (268 M, 1.07 G and 2.4 G
rows) runs **k = 3** times in the same job. Every other point runs once. *(Amended by #770, below: each `W = 1`
point at 16 blocks or more runs `k₁ = 6` times, B.5.)*

**Criteria 1 and 2** (replace §8.1's 1 and 2):

1. **Ceiling:** `RssAnon(n, W) ≤ R₀ + min(W, n) · (R₁ − R₀) + s + s_rss + c_leaf · n` for **every run** (the
   maximum over all runs, the `k` repeats included), 16 blocks included. `s_rss = 32 MiB + 8 MiB · (W + 1)` and
   `c_leaf` is A.1's, with `e_leaf` read off the run's sealed manifest.
2. **Plateau:** `med RssAnon(n₂) ≤ 1.05 · med RssAnon(n₁) + c_leaf · (n₂ − n₁)` for 64 ≤ `n₁` < `n₂` at `W = 4`
   (64 = 16 W), where `med` is the median of the point's 3 runs. The same inequality must hold at `W = 1` from 16
   blocks, on its single runs.
   *(Amended by #770, below: the medians failed on noise in G r3; criterion 2 is the slope of every run's peak
   against `n`, B.3. The `W = 4` sweep and `k = 3` above stand; `W = 1` runs `k₁ = 6` times from 16 blocks, B.5.)*

**Still sharp.** *(Amended by #770, below: B.4 shows the pre-#759 writer failing the slope test on its own data.)*
The pre-#759 writer still fails criterion 2. At `W = 1`, from 64 to 573 leaves, its `RssAnon`
rose from 59.5 to 71.7 MiB, which is 12.8 MB. The allowance is `0.05 · 62.4 MB + 509 · 8,408 B` = 3.1 + 4.3 =
7.4 MB.

### A.3 — §8.1 criterion 6: parallelism on `/dev/shm`, in its own job

**Why.** On ksb, both scratch volumes are `emptyDir`s on the node's one kubelet disk, which every pod on the node
shares. During the proof that disk was 79–98 % busy. At 268 M rows, identical passes of one job differed 2.4×,
and `--cpu 8` gave 1.14×. On tmpfs, `W = 4` was **2.78×** faster than `W = 1`, CPU-bound on 4 cores (the
diagnosis §3, job `tessera-715-r2-par-shm`). On shared scratch, criterion 6 measures the neighbours' I/O, not
the writer. `/dev/shm` is the only volume a ksb job can get that isn't that disk.

**Criterion 6** (replaces §8.1's 6):

**6. Parallelism:** in a **separate ksb job** (`ksb run --cpu 4 --exclusive-cpus`, the same binary and consumer
reproducer), with the session directory and the output on `/dev/shm`, at 268 M rows `W = 4` is faster than
`W = 1` by at least 1.5× wall time. `--mem` is sized for what tmpfs holds, since its pages count toward it:

```text
--mem  ≥  ceiling(W = 4, n = 64)  +  (W + 1) · BLOCK_ROWS · r_stage  +  2 · T  +  (J + min(W, J)) · S,   plus 1 GiB
```

The terms are criterion 1's ceiling, the staged input, and ADR-0026's disk paragraph: `2 · T` is the committed fragments
and the output `.tsra`, each the product's size `T`, and `(J + min(W, J)) · S` is the encoded blocks staged before their
commit plus the files of the encodes in progress, with `S` the largest staged block. `T` is the size of the 268 M-row
product from the criteria 1–5 job. Each run's session and output are deleted before the next run. For the consumer shape
that is about 0.3 + 0.5 + 4.0 + 0.3 ≈ 5.1 GB = 4.7 GiB (`T` ≈ 2.0 GB, from ~18 GB at 2.4 G rows), so with 1 GiB of
headroom, rounded up, `--mem 6Gi`. On tmpfs no disk waits, so a shortfall is the writer's. As before, the PR then stops
with a design question rather than merging a serial pipeline.

**Criteria 1–5 stay on disk-backed `--scratch`,** as in §8.1's *Where*. §8.1's tmpfs guard (`statfs` of the
session directory is not `TMPFS_MAGIC`, else the job fails) applies to their job only. The criterion 6 job
applies the converse guard: its session directory **is** `TMPFS_MAGIC`, else it fails, so neither job can
silently measure the other's setup. Criterion 6 is kept apart because tmpfs pages are RAM: they would make
criterion 3's `shmem` and `memory.current` checks fail by design.

**Recorded as context, not a criterion:** the criteria 1–5 job also records, at 268 M rows, the `W = 1` and
`W = 4` wall times on scratch and the session device's measured `%util` over each run (busy time over wall time,
from `/sys/block/<dev>/stat`). They go in the same table, so a reader can see the shared-disk throughput next to
the tmpfs result without either being mistaken for the other.

### A.4 — Timings on shared disks are context, never evidence (owner, 2026-11-04)

**The known limit.** The ksb node's scratch volumes sit on a shared HDD that is overwhelmed. This is a node
configuration problem, tracked as [exo-pet/exo-fleet#980](https://github.com/exo-pet/exo-fleet/issues/980), not a
property of the writer.

- **No graded timing on a shared disk.** Every disk-throughput or wall-time number measured on a ksb disk-backed
  scratch volume, or on the seat's `/mnt/HDD`, is dominated by the environment. It is never pass/fail evidence for
  any criterion, in this proof or in a review of it. Criterion 6's `/dev/shm` run (A.3) is the only graded timing.
- **Criteria 1 and 3 stay on disk-backed scratch,** because a slow disk makes them harder, not easier. A slow
  commit keeps the window full, so the most encodes overlap (criterion 1), and write-back that lags keeps more staged
  pages dirty and resident (criterion 3). A pass there is conservative. A criterion-3 failure (an
  OOM under `--mem 4Gi`, or `anon` apart from `RssAnon`) must be checked against the disk's measured `%util` for
  that run before anyone calls it a writer defect. A saturated disk points to exo-fleet#980 first.
- **The shared-disk throughput table (A.3) is context only,** until exo-fleet#980 gives ksb jobs an uncontended
  scratch class. When it does, a timing criterion on disk-backed scratch needs an amendment of this ADR. It is not
  adopted silently.

## Amendment (#770, 2026-11-08) — the plateau as a slope test over every run

The third run of the §8.1 proof (#715 G r3: job `tessera-715-r3-sweep-2`, exit 1, PR #767 at `0b66160`, on the
post-#759 writer) passed criteria 1, 4 and 5, and criterion 6 in its own job (`tessera-715-r3-par-3`, exit 0, 2.68×).
It failed criterion 2 at `W = 4` (criterion 3's failure was an evaluator defect, fixed in `0b66160`). The owner
ruled on #715 (comment of 2026-11-08) that criterion 2 becomes a slope test. This amendment applies that ruling to
§8 and §8.1, as amended by #760. The text it replaces is kept above, marked *(Amended by #770, below)*.

### B.1 — Why the median-of-3 plateau fails on noise

A.2 compared the medians of 3 runs pairwise. In r3, at `W = 4`, the 9 runs from 64 leaves on were (MiB of peak
`RssAnon`, the job's `records.json`):

| Leaves | Rows | Run 0 | Run 1 | Run 2 | Median | Spread within the point |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 64 | 268,435,456 | 180.95 | 189.54 | 190.20 | 189.54 | +5.1 % |
| 256 | 1,073,741,824 | 189.14 | 205.27 | 202.86 | 202.86 | +8.5 % |
| 573 | 2,400,000,000 | 191.54 | 195.86 | 206.23 | 195.86 | +7.7 % |

From 64 to 256 leaves the median rose to 202.86 MiB against A.2's allowance of
`1.05 · 189.54 MiB + 192 · 7,980 B` = 200.5 MiB, so the pairwise check failed. The medians aren't monotone in `n`:
the 573-leaf median is below the 256-leaf one. And the spread within one point (up to +8.5 %) is larger than `τ`.
A.2's premise, that a median of 3 discards the one encode-peak coincidence per point, didn't hold: two of the three
256-leaf runs were high, so the median was too. The peaks fell at row-groups spread over the whole input (380 to
27,315), as in the diagnosis §2. That is noise from concurrent encode peaks, not growth in `n`. A pairwise test
compares two medians, one point's against another's, so a point that draws two high runs decides the verdict.

A slope fitted to every run uses all 9 runs at once. Noise that doesn't depend on `n` widens the slope's
confidence interval. It doesn't bias the slope. A regression that grows with `n` moves the slope itself.

### B.2 — The estimator: ordinary least squares over every run, with its standard error

*(Amended by #797, below: this across-run fit is reported as context, not graded; criterion 2 is graded on the
within-run fit of C.2.)*

**The runs.** For each `W`, every run at `n ≥ 16 W` leaves, each repeat its own point (no medians): at `W = 4` the
`k = 3` runs at each of 64, 256 and 573 leaves (`N = 9`); at `W = 1` the `k₁ = 6` runs at each of 16, 64, 256 and
573 leaves (`N = 24`). The `W = 4` sweep and `k` are A.2's, unchanged; `k₁` is derived in B.5 (r3 ran `W = 1` once
per point, `N = 4`, and B.4's r3 numbers are that design's). The runs below `16 W` are out, as in §8: the rise from
16 to 64 leaves at `W = 4` is in the regime §8 doesn't model, and criterion 1 bounds it.

**The fit.** With `yᵢ` the run's peak `RssAnon` in bytes and `nᵢ` its leaves, ordinary least squares (OLS) of
`y = a + b · n`:

```text
b̂     =  Σ (nᵢ − n̄) (yᵢ − ȳ)  /  Sₙₙ,          Sₙₙ = Σ (nᵢ − n̄)²
σ̂²    =  Σ (yᵢ − â − b̂ nᵢ)²  /  (N − 2)
SE(b̂) =  σ̂ / √Sₙₙ
```

`b̂` is in bytes per leaf; divided by `BLOCK_ROWS` = 4,194,304 it is bytes per row.

**Why OLS, not Theil–Sen.** The noise is what decides it, and the measured noise has three properties:

- **It doesn't depend on `n`.** The spikes are the `W` encoders' peaks coinciding at an arbitrary row-group (the
  diagnosis §2; in r3 the peaks fell anywhere from row-group 380 to 27,315). The spread within a point is similar at
  every `n` (+5.1 %, +8.5 % and +7.7 % above). Noise of that kind is variance, not bias, for an OLS slope, and
  `SE(b̂)` measures it from the run's own residuals. Nothing is fitted to a previous run. `SE(b̂)` treats the runs
  as independent. Repeats of one point run one after another in the same job, so a condition that lasts across them
  (a neighbour's load) would correlate them, and `SE(b̂)` would understate the noise. That is the same limit A.4
  states for timings, and it is one more reason the level is nominal.
- **It is one-sided and skewed.** A coincidence only raises a peak. OLS doesn't need symmetric noise for its slope
  to be unbiased. The `t` quantile below is exact only for Gaussian noise, so the stated level is nominal (B.3).
- **There are only three or four distinct `n`, with a few runs each.** Theil–Sen's robustness to outliers is
  what it would buy here. But its median of the 27 pairwise slopes between different points is set by the
  within-point spread, and its confidence interval would come from a bootstrap. A bootstrap over three runs per point
  has only 10 distinct resamples per point, and it underestimates the variance by a factor of `(k − 1) / k`, so its
  interval would be narrower than the noise and grainy. OLS's bound is in closed form, deterministic and checkable
  by hand from the records, which is what an acceptance criterion should be. On r3 the two estimators agree
  (Theil–Sen 21,832 B/leaf against OLS 19,961 at `W = 4`, 5,750 against 5,787 at `W = 1`, both within 0.15 `SE`).
  Theil–Sen may be recorded as a cross-check. It isn't graded.

The fit uses every run, so a point whose median happens to be high, like 256 leaves in r3, carries no more weight
than its runs do.

### B.3 — The pass rule, and its margin

**The level.** One-sided, `α = 5 %` **per `W`**: at each `W`, a writer whose true slope is exactly `c_leaf` fails
that `W`'s slope check in at most one proof run in 20 (nominally; B.2). The margin is the one-sided 95 % confidence
bound of `b̂`:

```text
margin  =  t₀.₉₅,N−2 · SE(b̂)          t₀.₉₅,₇ = 1.895 (W = 4, N = 9),   t₀.₉₅,₂₂ = 1.717 (W = 1, N = 24)
```

(r3, with `N = 4` at `W = 1`, used `t₀.₉₅,₂` = 2.920, B.4.) The level is chosen before the data, not fitted to r3.
95 % is the conventional one-sided level, and B.4 and B.5 show that at it the test keeps the power it needs.

**Two tests, one proof.** The proof runs the slope check at both `W` and fails if either fails, so a writer at
exactly `c_leaf` at both `W` fails the proof's slope checks with probability `1 − 0.95²` = 9.75 % if the two are
independent, and at most 10 % whatever their dependence (the union bound). The level stays 5 % per `W`, without a
Bonferroni correction to 2.5 %, and the false-fail rate of the proof's two slope checks is stated as that
family-wise ≤ 10 %; with `W = 1`'s re-check it is about 15 % overall (B.5). The
correction would raise `t` from 1.895 to 2.365 at `W = 4` (+25 % on its margin and detection floor) and from 1.717
to 2.074 at `W = 1`, where B.5's `k₁` would have to grow to keep its floor at `c_leaf`. That trades the regressions
criterion 2 exists to catch for fewer false alarms, and the two errors don't cost the same: a false fail costs an
investigation of a run, a false pass ships the regression. 9.75 % is also the worst case: it needs a true slope of
exactly `c_leaf` at both `W`, and a writer below `c_leaf` fails less often. The resolution checks add to the
false-fail rate in the way B.5 states. A failing run is a finding about the writer, investigated like any
other. It isn't rerun until it passes, which would make the level meaningless.

**The resolution check.** A margin derived from the run's own noise widens with that noise, so a run noisy enough
would pass any slope. The test must still catch the regression it exists for: the zone-statistics half of `c · rows`,
**`δ_z` = 0.029 B/row = 121,635 B/leaf** (§8, note §4.2), the smallest regression §8 says the plateau must catch.
With probability at least 95 %, a slope of `c_leaf + δ` exceeds the pass bound when
`δ ≥ (t₀.₉₅ + t₀.₉₅) · SE(b̂)` (the usual normal-theory approximation, with the central `t` for both quantiles).
That `2 · t₀.₉₅,N−2 · SE(b̂)` is the run's **detection floor**. A run whose floor exceeds the growth its `W` must
catch can't resolve that growth. It is inconclusive, and an inconclusive run fails; it never passes. At `W = 4` the
growth to catch is `δ_z`. At `W = 1` it is `δ₁ = c_leaf`, an unaccounted growth as large as the whole allowance
(B.5); since `c_leaf` < `δ_z`, that check covers `δ_z` too.

**Criterion 2** (replaces A.2's criterion 2):

**2. Plateau:** for each `W ∈ {1, 4}`, over every run at `n ≥ 16 W` leaves (B.2), the OLS slope of peak `RssAnon`
against `n` satisfies both

- `b̂ ≤ c_leaf + t₀.₉₅,N−2 · SE(b̂)` (**slope**), and
- `2 · t₀.₉₅,N−2 · SE(b̂) ≤ δ_z = 121,635 B/leaf` at `W = 4`, and `≤ c_leaf` at `W = 1` (**resolution**),

with `c_leaf` A.1's, its `e_leaf` read off the run's sealed manifest (the maximum over the runs the fit uses), and
`N` the runs the fit uses (9 at `W = 4`, `4 k₁` = 24 at `W = 1`).

Equivalently, the slope check fails a run only when the data show, at 95 % confidence, that peak `RssAnon` grows by
more than `c_leaf` per leaf: `b̂ − t₀.₉₅,N−2 · SE(b̂) > c_leaf`.

*(Amended by #797, below: `b̂` and `SE(b̂)` are the within-run fit's, and `t` is `t₀.₉₅,G−1` with `G` the runs that
fit uses, C.2 and C.3. The two checks, their targets and "inconclusive fails" are unchanged.)*

**`W = 1` gets the same test,** on its `N = 24` runs from 16 leaves (`16 W`), with the tighter resolution target.
Its noise is smaller (r3 residual σ̂ = 1.90 MiB against 7.98 MiB at `W = 4`), which is why it is the `W` that can
resolve a growth of the order of `c_leaf` at an affordable `k₁`, and why B.5 spends the extra runs there.

**The evaluator.** PR #767's evaluator still implements A.2's medians and `τ`. The proof's evaluator implements this
criterion 2 (the fit, both checks at both `W`, and the record B.5 asks for) in G r4, under #715, which tracks it;
until then this criterion has no code on `dev`, and a run is graded against it only by hand from its records, as B.4
does for r3.

**What criterion 2 no longer checks.** A.1 says criterion 2 "checks the allowance" `c_leaf`. As a slope test it
checks it to within its margin: on r3, a growth up to 33,160 B/leaf at `W = 4` and 21,290 B/leaf at `W = 1`
passes, four and almost three times `c_leaf`. At `k₁ = 6`, `W = 1`'s bound falls to about 11,175 B/leaf at r3's
noise, 1.4 times `c_leaf` (B.5). The per-leaf held heap of the session is checked exactly, in-tree,
by #759's counting-allocator test (`tessera-io/tests/session_leaf_memory.rs`: the serialized entries plus 128 B per
leaf). Criterion 2 is what catches a growth in `n` that the in-tree tests can't see, at real `BLOCK_ROWS`: a
per-row term like zone statistics, or retention around the encoder.

**`τ` is retired.** It appeared only in the plateau (§8's second block, §8.1's criterion 2 and A.1/A.2's restatements
of both), and nothing else uses it. A pairwise tolerance was a fixed fraction of a noisy single value. The slope test
puts the margin derived from the run's own noise in its place, with the level (95 %) and `δ_z` that the old
"tightening `τ`" rule had stood for. Raising `δ_z` or lowering the level loosens criterion 2 and needs an amendment
of this ADR, as raising `τ` did. §8's row-group record (where each run's peak fell) stays, as the evidence for the
16-to-64 rise.

**Unchanged:** criterion 1 (the ceiling, checked on every run, A.2), `s`, `s_rss`, `c_leaf`'s definition (A.1),
the `W = 4` sweep and `k = 3` (A.2; `W = 1`'s repeats are B.5's), criteria 3–5 and their budgets (`--mem 4Gi`,
8 GiB), and criterion 6 with its `/dev/shm` job, its converse tmpfs guard and its `--mem` (A.3), and A.4.

**`c_leaf` in the numbers below.** A.2 reads `e_leaf` off the run's sealed manifest. In r3 it is at most
2,966 B (2,965 B at 16, 64 leaves; 2,966 B at 256 and 573), so `c_leaf` = 2 · (2,966 + 1,024) B = **7,980 B**.
A.1's worked figure, 8,408 B, used a 3,180 B entry that wasn't the canonical length A.1 defines. The criterion
uses the definition. Every verdict below is the same with 8,408 B, which moves each bound up by 428 B/leaf.

### B.4 — Power, with the recorded numbers

**r3 as measured** (`tessera-715-r3-sweep-2`, the post-#759 writer at `0b66160`; bytes per leaf):

| `W` | `N` | `b̂` | `SE(b̂)` | Margin | Bound `c_leaf` + margin | Slope | Resolution: floor `2 t SE` vs target |
| ---: | ---: | ---: | ---: | ---: | ---: | --- | --- |
| 4 | 9 | 19,961 | 13,290 | 25,180 | 33,160 | **pass** | 50,359 ≤ 121,635 (`δ_z`): **pass** |
| 1 | 4 | 5,787 | 4,558 | 13,310 | 21,290 | **pass** | 26,619 > 7,980 (`c_leaf`): **inconclusive** |

r3 ran `W = 1` once per point (`k₁ = 1`). Against the `δ_z` target it resolved (26,619 ≤ 121,635); against B.5's
`c_leaf` target it can't, and under the amended criterion 2 a run of that design fails as inconclusive. That is the
gap B.5's repeats close; r3 isn't regraded, and the rest of B.4 uses its numbers as the noise they measured.

The fit's inputs: at `W = 4` the 9 runs of B.1 (`n̄` = 297.67, `Sₙₙ` = 396,434, σ̂ = 7.98 MiB); at `W = 1` 59.88,
62.52, 59.98 and 64.18 MiB at 16, 64, 256 and 573 leaves (`n̄` = 227.25, `Sₙₙ` = 191,647, σ̂ = 1.90 MiB). The `W = 4`
slope, 19,961 B/leaf, is 0.90 `SE` above `c_leaf`: the data don't distinguish it from `c_leaf`, and they show no
significant growth beyond it at 95 %.

**A regression added to r3.** A growth of `δ` per leaf adds `δ · nᵢ` to every run. That moves `b̂` by exactly `δ`
and leaves the residuals, and so `SE(b̂)` and the margin, unchanged. So each row below is r3's `b̂ + δ` against the
bound above:

| Added growth | `δ` (B/leaf) | `W = 4`: `b̂ + δ` vs 33,160 | `W = 1`: `b̂ + δ` vs 21,290 |
| --- | ---: | --- | --- |
| Zone statistics, full: 0.058 B/row (+47 MB over 192 leaves, §8) | 243,270 | 263,231: **fails** | 249,057: **fails** |
| Zone statistics, half: 0.029 B/row (`δ_z`) | 121,635 | 141,596: **fails** | 127,422: **fails** |
| Reverting #759: r1's `W = 1` slope less r3's, 24,265 − 5,787 (below) | 18,478 | 38,439: **fails** | 24,265: **fails** |
| Pre-#759 per-leaf live heap (12.8 KB, the diagnosis §1) | 12,800 | 32,761: passes | 18,587: passes |

**The pre-#759 writer on its own data.** r1's sweep (`tessera-715-sweep-1`, tessera `a3df683`, before #759) is the
`W = 1` design exactly: 59.1, 59.5, 64.1 and 71.7 MiB at 16, 64, 256 and 573 leaves (the job log's 0.1 MiB
precision). OLS gives `b̂` = 24,265 B/leaf, `SE` = 915, a bound of 7,980 + 2.920 · 915 = 10,652 B/leaf: it
**fails**, by 2.3 times the bound. (r1 ran `W = 4` once per point, so it isn't the `W = 4` design, and r3's
fail-before job, `tessera-715-r3-failbefore-2`, has only 64 and 573 leaves from `16 W` on, two runs with no degree
of freedom; neither is graded here.)

The 12.8 KB row passes on r3's data, and #770 asked for that cost to fail; at `W = 4` this is a deliberate deviation.
12.8 KB is the pre-#759 session's **live** heap per leaf. Criterion 2 measures `RssAnon`, and in `RssAnon` that writer
grew by its measured 24,265 B/leaf, which fails. The factor of about 2 is that 24,265 against the 12,800 live, ×1.90:
the whole `RssAnon` slope against the whole live heap per leaf, which is A.1's allocator retention. The 18,478 B/leaf
of the regression row is a different comparison, the difference between two writers' `RssAnon` slopes, and its ×1.44
against 12,800 compares it with the wrong live figure; set against the difference of their live heaps, 12,800 less
the post-#759 writer's bound (#759's test: the entry plus 128 B, 3,094 B at `e_leaf` = 2,966 B), at least 9,706 B,
it is at most ×1.90 again.
An `RssAnon` growth of 12.8 KB/leaf above `c_leaf` at `W = 4` is below what `k = 3` can resolve (the next
paragraph), and resolving it there would take about 36–47 runs per point. At `W = 1` with `k₁ = 6` it is above the
detection floor, which is at most `c_leaf` by criterion 2 (B.5), so the slope test catches it there with probability
at least 95 %. The live heap per leaf is checked exactly in-tree (B.3), and that test fails on the pre-#759 writer.

**The smallest growth that fails, in r3's design** (`k = 3` at `W = 4`, one run per point at `W = 1`).

- **Added to r3, deterministically:** at `W = 4`, `δ` > 13,198 B/leaf (0.0031 B/row), a total slope above 33,160
  B/leaf; at `W = 1`, `δ` > 15,503 B/leaf (0.0037 B/row), a total above 21,290 B/leaf.
- **Detected with probability ≥ 95 % at r3's measured noise** (the minimum detectable growth above `c_leaf`,
  `2 · t₀.₉₅,N−2 · SE(b̂)`): at `W = 4`, **50,359 B/leaf = 0.0120 B/row**; at `W = 1`, **26,619 B/leaf =
  0.0063 B/row**.

`δ_z` = 121,635 B/leaf is 2.4 times the `W = 4` detectable growth and 4.6 times the `W = 1` one. At `W = 4` it lies
9.2 `SE` above `c_leaf` against a bound 1.9 `SE` above it, so its detection probability is above 0.999. **At
`W = 4`, `k = 3` is enough; `k` doesn't change.** Resolving a growth of the order of `c_leaf` itself at `W = 4` (say
12.8 KB/leaf) would take about 36–47 runs per point at r3's noise (36 with `t` at its value `t₀.₉₅,3k−2`, 47 with
`t` held at 1.895). That isn't `W = 4`'s job: `W = 1`, with its lower noise and B.5's repeats, and the in-tree test
resolve per-leaf terms.

### B.5 — `W = 1` repeats: `k₁` from the detection floor

The fresh review of this amendment (#770, ruling of 2026-11-09) found the gap B.4 shows: in r3's design an `RssAnon`
growth of about 8–13 KB/leaf beyond `c_leaf`, held outside `WriteSession` (the allocator, the encoder), passes at
both `W` (about +2–3 MB at 573 leaves), and the in-tree test pins only the session's own heap. The owner ruled that
`W = 1` closes it with repeats, and `W = 4` stays at `k = 3`.

**The target.** `W = 1` must catch, with probability at least 95 %, an `RssAnon` growth of **`δ₁ = c_leaf`** beyond
`c_leaf` (7,980 B/leaf in r3, 0.0019 B/row; the run's own `c_leaf` is used, as in the slope check). That is a
second term as large as the whole allowance, a per-leaf slope twice what §8 grants. It is the smallest excess that
matters at the scale §8 states its bound in: `c_leaf` already holds a factor 2 for allocator retention that isn't
a theorem (A.1), so a smaller unaccounted term is within the uncertainty the allowance itself carries, while a term
of `c_leaf` or more doubles the per-leaf cost the ADR promises. It covers the reviewer's whole 8–13 KB/leaf gap and
the 12.8 KB/leaf row of B.4.

**The arithmetic.** With `k₁` runs at each of the four sizes 16, 64, 256 and 573 leaves, `N = 4 k₁`, the leaves'
`Sₙₙ` is `k₁` times r3's 191,647, and

```text
floor(k₁)  =  2 · t₀.₉₅,4k₁−2 · σ₁ / √(k₁ · 191,647)          σ₁ = W = 1's per-run noise
```

**The noise estimate, and why it is provisional.** r3 ran each `W = 1` point once, so it has no within-point
spread to measure. σ₁ is estimated from the residuals of the four-point fit, σ̂ = 1,995,436 B (1.90 MiB), with
`N − 2` = 2 degrees of freedom. That estimate also holds any curvature of the mean in `n`, which would inflate it,
and with 2 degrees of freedom it is coarse: the 90 % interval for σ₁ is 0.58–4.4 times σ̂. (r1's pre-#759 `W = 1`
fit gives a residual of 0.38 MiB, at its log's 0.1 MiB precision, and A.2's two proof runs at 64 leaves peaked
0.5 MiB apart, so r3's σ̂ is more likely high than low.) The numbers below are r3's planning figures, provisional
until G r4 measures σ₁ with `N − 2 = 22` degrees of freedom.

| `k₁` | `N` | `t₀.₉₅,N−2` | `SE(b̂)` at r3's σ̂ | Floor `2 t SE` | Floor / `c_leaf` | P(r4 inconclusive) if σ₁ = σ̂ |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 (r3) | 4 | 2.920 | 4,558 | 26,619 | 3.34 | 0.91 |
| 2 | 8 | 1.943 | 3,223 | 12,526 | 1.57 | 0.88 |
| 3 | 12 | 1.812 | 2,632 | 9,539 | 1.20 | 0.73 |
| 4 | 16 | 1.761 | 2,279 | 8,028 | 1.006 | 0.46 |
| 5 | 20 | 1.734 | 2,038 | 7,070 | 0.89 | 0.19 |
| **6** | **24** | **1.717** | **1,861** | **6,391** | **0.80** | **0.046** |
| 7 | 28 | 1.706 | 1,723 | 5,877 | 0.74 | 0.006 |

The last column is the chance that G r4's own σ̂ comes out high enough to put its floor above `c_leaf`, if σ₁ is
r3's estimate: `P(χ²₍N−2₎ > (N − 2) · (c_leaf / floor)²)`, since `(N − 2) σ̂² / σ₁²` is `χ²` with `N − 2` degrees of
freedom for Gaussian noise (nominal, as B.2 says).

**`k₁ = 6`.** `k₁ = 4`, the upper end of the review's estimate of 3–4, leaves the floor just above the target
(8,028 > 7,980 B/leaf). `k₁ = 5` is the smallest whose projected floor is under it, but by 11 %, and a run of that
design would come out inconclusive about one time in five even if r3's σ̂ were exact. `k₁ = 6` is the smallest with
both the projected floor under the target and the chance of an inconclusive run at most 5 %, the level criterion 2
already uses for a false fail. It puts the projected floor at 6,391 B/leaf (0.0015 B/row), the slope bound at
`c_leaf + 1.717 · 1,861` ≈ 11,175 B/leaf, and the 12.8 KB/leaf row of B.4 above the floor. Every `W = 1` point at 16
leaves or more runs `k₁ = 6` times (24 runs); the points below 16 leaves run once, as in A.2, for criterion 1. The
cost is the runs: r3's `W = 1` points from 16 leaves took 12,884 s together (8,491 s of them at 573 leaves), so six
repeats are about 21.5 h of serial wall time. How G r4 schedules them is #715's. With the slope checks (B.3), a
writer at exactly `c_leaf` at both `W` then fails the proof with probability at most about 15 % if σ₁ is r3's
estimate (10 % from the slope checks, 4.6 % from `W = 1`'s resolution check, by the union bound), stated as such.

**The re-check rule.** The `k₁` above is chosen from r3's estimate; G r4 is graded on its own. Its resolution
check at `W = 1` uses its own `SE(b̂)` and `t₀.₉₅,N−2` at the `N` it fit (24 if every run completed): if
`2 · t₀.₉₅,N−2 · SE(b̂)` exceeds the run's `c_leaf`, the
proof **fails as inconclusive**. It does not pass, whatever `b̂` is, because a floor above the target means a growth
the ADR must catch could pass. An inconclusive run is recorded with its measured σ̂, `k₁` is re-derived from that σ̂
by the arithmetic above in an amendment of this ADR, and the next run is graded on its own data. A run isn't
repeated until it resolves.

**The stated limits.** These are the detection floors criterion 2 runs at, the smallest growth beyond `c_leaf` it
catches with probability at least 95 %:

- **`W = 4`, `k = 3`:** 50,359 B/leaf (0.0120 B/row) at r3's noise, required to be at most `δ_z` = 121,635 B/leaf.
  A growth beyond `c_leaf` below the floor can pass at `W = 4`; that `W` is there for the zone-statistics class.
- **`W = 1`, `k₁ = 6`:** projected 6,391 B/leaf (0.0015 B/row) at r3's noise, required to be at most `c_leaf`
  (7,980 B/leaf in r3). A growth beyond `c_leaf` smaller than the run's floor, so less than `c_leaf`, can pass.

Each proof run records both measured floors with its verdict, so the limit a run actually met is on file.

## Amendment (#797, 2026-11-11) — criterion 2 as a within-run slope

The fourth run of the §8.1 proof (#715 G r4: eight shards `tessera-715-r4-sweep-*` and `tessera-715-r4-par-1`, PR #767
at `fa09a81`, on the post-#759 writer) ran #770's full design, `k = 3` at `W = 4` (`N = 9`) and `k₁ = 6` at `W = 1`
(`N = 24`). It passed every criterion but criterion 2 at `W = 4`, which failed its across-run slope check. The owner
ruled on #797 (2026-11-11) that criterion 2 is judged by a within-run slope, because the ksb HDD is degraded
([exo-pet/exo-fleet#980](https://github.com/exo-pet/exo-fleet/issues/980)). This amendment applies that ruling to
§8.1 and B.2–B.5. The text it replaces is kept above, marked *(Amended by #797, below)*. It was written, and the
estimator fixed, before the estimator was computed on r4's records; whatever it gives on them is the verdict, and
changing the estimator again needs an amendment.

### C.1 — Why: across runs, the peak tracks the disk, not `n`

r4's across-run fit (B.2), from its records:

| `W` | `N` | `b̂` (B/leaf) | `SE(b̂)` | Bound `c_leaf + t · SE` | Slope |
| ---: | ---: | ---: | ---: | ---: | --- |
| 4 | 9 | 40,437 | 12,208 | 31,109 | **fails** |
| 1 | 24 | 5,072 | 1,247 | 10,121 | passes |

The `W = 4` slope comes from the runs, not from growth within them:

- **Within every `W = 4` run, `RssAnon` is flat over the tenths of its input.** The two highest 573-leaf runs were
  high from their first tenth (218.6 and 203.2 MiB over leaves 0–57, against 185–193 MiB for whole 64-leaf runs). The
  highest peaked at input row-group 448, about 7 leaves into its input, so per-leaf state can't be what lifts it.
- **Against the leaves committed at each run's peak,** instead of the run's `n`, the `W = 4` slope is
  −26,778 ± 46,234 B/leaf.
- **`W = 1` holds the same per-leaf state** and measures 5,072 B/leaf, under `c_leaf`.
- **The run's level follows the disk.** The scratch volumes are on the node's one shared HDD (A.3), 99–100 % busy
  over every r4 run from 16 leaves on (each run's recorded `%util`). A stalled commit keeps more encoded payloads
  in flight (A.4: a slow disk keeps the window full), so a run's peak rises with its disk's contention. The longest
  runs, the 573-leaf ones, overlapped the most contention, and nine shards ran at once on that disk.

A.4 rules that disk-backed timings are context, never evidence. The same reasoning applies to a level that the disk
moves from run to run: it is a property of the run's environment, which an across-run fit reads as a property of `n`.
B.2 had named this limit ("a condition that lasts across them … would correlate them"); r4 shows it biasing the slope,
not only its error.

A within-run fit compares a run with itself. A per-run intercept absorbs whatever stays fixed over that run (its I/O
state, its neighbours, where its encode peaks coincided), and the slope is what grows as the same run commits more
leaves. That is the quantity §8's bound is about: a per-leaf or per-row term grows within a run, and so does the
zone-statistics regression criterion 2 exists to catch. A disk whose contention changes within a run still adds noise,
which the run-clustered standard error (C.2) counts.

### C.2 — The estimator: fixed effects over the tenths of each run

**The data.** Every run records `tenths`, the peak `RssAnon` of its 10 ms samples in each tenth of its input
(`run.py`'s `peaks`): tenth `t ∈ {0, …, 9}` holds the samples taken while the generated row count is below
`(t + 1) · rows / 10`, and the last tenth runs on through the seal. For run `r` and tenth `t`:

- `y_rt` = that tenth's peak `RssAnon`, in bytes;
- `x_rt` = the leaves whose input is complete at the tenth's end, `⌊(t + 1) · rows / (10 · BLOCK_ROWS)⌋` for `t < 9`,
  and the run's `n` for `t = 9`, when the seal has committed every leaf.

The records hold the input position, not a commit count, so `x_rt` counts leaves through the reader. The leaves
committed lag it by at most the jobs in flight, `J` plus the one full block waiting for a worker (§5.2,
ADR-0026's credit window), a constant of the writer's configuration. A constant lag is absorbed by the run's
intercept; a varying one is bounded by those `J + 1` leaves, against the 45 (64 leaves at `W = 1`) to 459 (573 leaves
at `W = 4`) leaves over which a fitted run's `x` ranges. Within a tenth, which spans up to 57 leaves, the peak can
fall anywhere; under a real growth it falls near the tenth's end, where `x_rt` is taken, so the slope isn't biased,
and otherwise where it falls is noise, which the clustered standard error counts.

**Warm-up.** Only tenths with `x_rt ≥ 16 W` are fitted, the threshold B.2 used for runs, now applied within a run.
The records hold one peak per tenth, not per sample, so the rule is applied at the tenth's end: a fitted tenth may
begin before `16 W` (at `W = 4` and 573 leaves, tenth 1 covers leaves 57–114; at `W = 1` and 64 leaves, tenth 2
covers 12.8–19). That differs from #797's "samples only once `x ≥ 16 W`" by at most part of one tenth per run, and it
is the nearest the records allow.
A run with fewer than two such tenths has no within-run variation and adds nothing to the fit; it is left out of `G`
below. In the `job` design that leaves, at `W = 4`, the 6 runs at 256 and 573 leaves (8 and 9 tenths each; a 64-leaf
run has only its last); at `W = 1`, the 18 runs at 64, 256 and 573 leaves (a 16-leaf run has only its last). A fitted
tenth with no sample is missing data, and criterion 2 fails on it.

**The model.**

```text
y_rt  =  α_r  +  b · x_rt  +  ε_rt
```

one intercept `α_r` per run and one slope `b` common to the runs. `b̂` is OLS on the deviations from each run's own
means, `x̃_rt = x_rt − x̄_r` and `ỹ_rt = y_rt − ȳ_r`:

```text
b̂     =  Σ x̃_rt ỹ_rt  /  Sₓₓ,          Sₓₓ = Σ x̃_rt²,      e_rt = ỹ_rt − b̂ · x̃_rt
SE(b̂) =  √( G/(G − 1) · (N − 1)/(N − 2) · Σ_r (Σ_t x̃_rt e_rt)² )  /  Sₓₓ
df    =  G − 1
```

with `G` the fitted runs and `N` their fitted tenths.

**Why this standard error.** Tenths of one run are not independent: a run's encode peaks, its disk's stalls and its
allocator's retained top carry from one tenth to the next, so the tenths are autocorrelated and the classical OLS error,
which counts each tenth as a fresh draw, would be too small. The cluster-robust (sandwich) estimator, clustered by run,
allows any correlation and any variance within a run; it assumes only what B.2 already assumed, that runs are
independent of each other. `G/(G − 1) · (N − 1)/(N − 2)` is the CR1 small-sample factor in the convention of Stata's
`xtreg, fe` (the per-run intercepts nested in the clusters are not counted as regressors; the reported constant is); on
the demeaned data with one regressor the strict factor would be `G/(G − 1)` alone, so the convention is slightly
conservative, by about 1 % on `SE(b̂)` at r4's `N` (51 tenths at `W = 4`). `t` with `G − 1` degrees of freedom is the
usual reference for few clusters (Cameron and Miller, "A Practitioner's Guide to Cluster-Robust Inference", *Journal of
Human Resources* 50(2), 2015). A per-tenth model of the autocorrelation (AR(1), Newey–West) was rejected: it would
assume a form the data can't test with 8–10 tenths per run, while clustering assumes none. With `G = 6` at `W = 4` CR1
is known to run small, so, as in B.2, the level is nominal; the bound is still closed-form and checkable by hand from
the records.

**What the per-run intercepts cost.** Variation between runs no longer informs `b̂`. A per-run term that is constant
over a run (a level that depends on `n` but doesn't grow during the run) is invisible to this fit. Nothing in §8's
model has that form: every term of the bound is either fixed (`p_block`, `s`, `s_rss`) or grows as the session
commits leaves (`c_leaf · n`, and the zone-statistics regression). Criterion 1 still bounds every run's peak against
its own `n`, so a level that rose with `n` across runs would still have to stay under the ceiling.

### C.3 — The pass rule, unchanged; the across-run slope as context

**Criterion 2** (replaces B.3's criterion 2): for each `W ∈ {1, 4}`, over the tenths of every run at `n ≥ 16 W`
leaves with `x_rt ≥ 16 W` (C.2), the within-run slope `b̂` and its cluster-robust `SE(b̂)` satisfy

- `b̂ ≤ c_leaf + t₀.₉₅,G−1 · SE(b̂)` (**slope**), and
- `2 · t₀.₉₅,G−1 · SE(b̂) ≤ δ_z = 121,635 B/leaf` at both `W` (**resolution**), and at `W = 1` also
  `≤ c_leaf` (B.5's **re-check**),

with `c_leaf` A.1's, its `e_leaf` the maximum over the sealed manifests of the runs at `n ≥ 16 W`, as in B.3. Unchanged
from B.3: the level (one-sided 5 % per `W`), `δ_z`, the `W = 1` target `c_leaf`, and **an inconclusive run fails**; it
never passes. Fewer than two fitted runs at a `W` is no standard error, and fails. B.3's statement of the error rates
stands: the two slope checks fail a writer at exactly `c_leaf` with probability at most 10 % (family-wise, nominal),
about 15 % overall with `W = 1`'s re-check (B.5).

**The across-run slope is reported, never graded.** The proof still computes B.2's fit at each `W` and prints it with
B.3's checks, labelled as confounded by the shared disk (C.1). It is the context a reader compares the within-run
verdict with, like A.3's shared-disk timings, and it isn't evidence for or against the writer until
exo-fleet#980 gives ksb jobs an uncontended scratch class; grading it again then needs an amendment.

**Unchanged:** the sweep, `k = 3` at `W = 4` and `k₁ = 6` at `W = 1` (B.5's `k₁` was derived for the across-run fit;
the re-check on the run's own `SE(b̂)` is what binds, and an inconclusive `W = 1` fit re-derives `k₁` by amendment,
as B.5 says), every other criterion, `c_leaf`, `δ_z`, the stated limits B.5 asks each run to record (now the
within-run fit's floors), and A.4.

### C.4 — Still sharp: the pre-#759 writer fails

The pre-#759 writer's `RssAnon` grows within its runs, so the within-run fit sees it, on both of its recorded sweeps.

**r1** (`tessera-715-sweep-1`, tessera `a3df683`, B.4's fail-before, `k = 1` at both `W`) predates the per-tenth record,
and its 10 ms sample files at 256 and 573 leaves (3.7 and 14.5 MB) are over what `ksb out` fetches. Its job logs fetch,
and each holds the reproducer's own ~1 s sampler lines (`sample rows=… RssAnon=…`; 2,463 of them at `W = 1` and 573
leaves). The tenths are rebuilt from those lines by the same cut as `peaks` (`run.py log-tenths`). A 1 s sampler can
miss a short spike that the 10 ms one would catch, so each rebuilt tenth's peak is a lower bound of the 10 ms one; that
is good enough for a fail-before, which isn't a graded run. The fit's `e_leaf` is r3's 2,966 B, since r1's manifests
weren't kept; the `W = 1` slope would still fail with entries 2,367 B larger, and the `W = 4` slope with entries 22,569
B larger. PR #767's `tessera/bench/ksb/677-rss-proof/fixtures/r1-logs` holds the extract of each fitted log (one line
per tenth, under the source log's sha256), from which the self-tests reproduce the fit.

**`tessera-715-r3-failbefore-2`** (B.4; `W = 1`, 64 and 573 leaves) recorded its tenths at 10 ms; its 573-leaf run
rose steadily from 59.6 to 68.5 MiB over its ten tenths.

| Sweep | `W` | `G` | `N` | `b̂` (B/leaf) | `SE(b̂)` | `t₀.₉₅,G−1` | Bound `c_leaf + t · SE` | Slope | Floor `2 t SE` |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | --- |
| r1 (logs) | 1 | 3 | 28 | 19,761 | 2,414 | 2.920 | 15,028 | **fails** | 14,096 > `c_leaf`: inconclusive, **fails** |
| r1 (logs) | 4 | 2 | 17 | 73,901 | 3,292 | 6.314 | 28,763 | **fails** | 41,566 ≤ `δ_z`: resolved |
| r3-failbefore-2 | 1 | 2 | 18 | 17,219 | 199 | 6.314 | 9,237 | **fails** | 2,513 ≤ `c_leaf`: resolved |

Every fit fails its slope check, by 1.3 to 2.6 times its bound. With `G` of 2 or 3 a clustered standard error is
fragile, so each is a fail-before test, evidence that the fit sees that writer's growth, not a statement of the
test's power, which B.5's floors recorded with each run are. `run.py`'s self-tests pin these numbers.

## References

`docs/spikes/681-vortex-writer-backpressure.md` (PR #692, `b6de2d1`) · #677 (owner decision, 2026-10-29) · #681 (the
nits) · ADR-0026 · ADR-0052 · ADR-0062 · ADR-0063 · vortex-data/vortex#10046 · MorePET/visionx-rs PR #41.
