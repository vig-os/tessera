# Migrating a streamed table to `push_table`

This page is for a Rust producer that streams one large table through `tessera-io`'s `StreamWriter`
(MorePET/visionx-rs's `pairs` table is the case it was written for, [#677]). Tessera
**v0.1.0-alpha.5** changes how such a table is written, under what names it is sealed, and two public types.
Readers keep one logical name for it. [ADR-0064] is the design. This page tells you what to change.

Code references are `file:line` at the tag `v0.1.0-alpha.5` (`5373f61`), under `tessera/crates/` unless they
say otherwise.

## What changed, in one table

| | alpha.4 and earlier | alpha.5 |
| --- | --- | --- |
| Streaming a table of any size | `table_job_streaming` (one block of any size) or `encode_streaming` in a job | `StreamWriter::push_table` |
| A streamed table above `BLOCK_ROWS` (4,194,304) rows | one block, `pairs` | leaves `pairs/0000`, `pairs/0001`, … with a partition marker |
| A table of at most `BLOCK_ROWS` rows | one block, `pairs` | unchanged, byte for byte |
| `table_job_streaming` above `BLOCK_ROWS` | writes one block | fails with `Error::StreamedTable` (`NeedsPartition`), and is `#[deprecated]` |
| Reading `pairs` | the one block | the same name, every leaf in order (`logical_table`, `read`, `sql`, Python `table`) |
| `TableSpec { .. }` literal | three fields | a fourth, `partition` |
| Unresolvable `logical_table` name | `Error::Container("no blocks for prefix …")` | `Error::LogicalTable { name, reason }` |
| Peak `RssAnon`, 2.4 G rows, `W = 4` | grew with the table (4.4 GiB at 268 M rows, [#677]) | 218.6 MiB, measured, and flat within each run |

## 1. Writing: switch to `StreamWriter::push_table`

### Before

The consumer reproducer (MorePET/visionx-rs [PR #41], `crates/visionx-tsra/examples/stream_memory.rs:247-267`
at `c4aff94`, abridged: its progress counter is left out) encodes the whole table inside one in-memory job:

```rust,ignore
writer.push(Box::new(move || {
    let groups = (0..rows).step_by(chunk as usize).map(move |start| {
        let n = chunk.min(rows - start) as usize;
        pairs(start, n, nonnullable_ms)
    });
    let mut spec = spec(Table::Pairs, rows);
    if nonnullable_ms {
        spec.columns[1].nullable = false; // the --nonnullable-ms diagnostic
    }
    let bytes = table::encode_streaming(&spec, groups)?; // the whole encoded table in RAM
    let block = BlockRef {
        name: "pairs".into(),
        kind: BlockKind::Table,
        digest: Some(tessera_core::hash::digest(&bytes)),
        spec: serde_json::to_value(spec)?,
    };
    Ok(((block, BlockPayload::new("pairs", bytes)), vec![]))
}))?;
```

`encode_streaming` (`tessera-io/src/table.rs:1053-1062`) pulls its input lazily, but it returns the whole
encoded table as one `Vec<u8>`, so the job's memory grows with the table. alpha.4's file-backed form,
`writer.push_staged(table_job_streaming("pairs", spec, groups))`, staged that output on disk but still wrote one
block of any size. Above `BLOCK_ROWS` rows it now refuses the table (section 2).

The snippet above no longer compiles on alpha.5 for an unrelated reason: `BlockRef::name` is a validated
`BlockName` (`tessera-core/src/block/mod.rs:48`), built with `BlockName::new("pairs")?` or `"pairs".parse()?`,
not `.into()`. The migration below deletes that code.

### After

The groups are pulled on the calling thread, so the iterator moves out of the job closure, and the
`BlockRef`/`BlockPayload` bookkeeping goes away. This is the change the RSS proof built
(`tessera/bench/ksb/677-rss-proof/stream_memory.patch:122-133`):

```rust,ignore
use tessera_io::{StreamWriter, WriteSession};

let groups = (0..rows).step_by(chunk as usize).map(move |start| {
    let n = chunk.min(rows - start) as usize;
    pairs(start, n, nonnullable_ms)
});
// Not `spec`: that would shadow the spec() helper the small tables below still call.
let mut pairs_spec = spec(Table::Pairs, rows); // spec.rows must be exact; partition: None
if nonnullable_ms {
    pairs_spec.columns[1].nullable = false;
}
let leaves = writer.push_table("pairs", pairs_spec, groups)?; // 573 leaves for 2.4 G rows
// … push the small `markers` / `ms_stats` tables as before (table_job, single blocks) …
let (manifest, stats) = writer.finish_with_stats(&output)?;
```

`StreamWriter::push_table(name, spec, groups) -> Result<u64>` (`tessera-io/src/stream.rs:856-937`):

- **Partition.** It partitions the table at `BLOCK_ROWS` (`tessera-io/src/table.rs:854`). At most `BLOCK_ROWS`
  rows is the one block `name`, byte-identical to `table_job_streaming`. A larger table becomes the leaves
  `name/0000`, `name/0001`, … It returns the number of leaves.
- **Input.** `groups` is pulled on the calling thread, in order. Each group holds at most `ROWS_PER_GROUP`
  (65,536, `table.rs:844`) rows with `spec`'s columns. Groups of another size are re-chunked to that grid, so
  the leaves don't depend on your chunking. `spec.rows` must equal the rows the input yields: an input that
  runs past it, or ends short, is `Error::StreamedTable` (`TooManyRows`, `TooFewRows`). A larger group is
  `OversizedGroup` (`tessera-core/src/block/partition.rs:186-223`).
- **Markers.** `spec.partition` must be `None`, since the writer sets each leaf's marker (section 4).
- **Nullable columns** are written as declared. The nullable `ms` that hit a dtype assertion at `9424f90`
  ([#677]) is in `push_table`'s test schema (`stream.rs:1688-1700`), and the RSS proof (section 5) ran the
  exact nullable schema.
- **Backpressure.** The call blocks while every encode worker has a job or the window is full, and returns
  once the last leaf is handed to a worker. `finish` commits and seals as before.
- **Failure.** A failed call never hands over the last leaf, so for a partitioned table `finish` refuses to seal
  (`Error::LogicalTable`) rather than seal a truncated one. A table of at most `BLOCK_ROWS` rows is never handed
  over at all, so a caller that ignores the error and still calls `finish` seals a product without that table.
  Propagate the error.

**Put the session directory on disk.** Each block's input is staged in the session's `staging/` directory
before it is encoded: at most `W + 1` blocks for `W` workers, `(W + 1) · BLOCK_ROWS · 24 B` = 503.3 MB at
`W = 4` for the consumer's 24-byte rows. Those files are page cache, which counts toward a cgroup's
`memory.current` and is reclaimed under pressure. On tmpfs they are RAM that can't be reclaimed. The
reproducer's `tempfile::tempdir()` is `$TMPDIR`, often `/tmp`, often tmpfs. Create the `WriteSession` under a
disk-backed directory (the proof added `--session-dir` for that, `stream_memory.patch:94-98`).

**Asserting the bound in your own tests.** `finish_with_stats` (`stream.rs:1110-1112`) returns `StreamStats`
(`stream.rs:350-385`). `peak_staged_input_blocks` (at most `W + 1`) and `peak_staged_input_bytes` count the
staged input, and `peak_in_flight_jobs` (at most the window's `max_in_flight`) counts the jobs held, including
the ones waiting to commit in order. That last one is the public high-water mark [#677]'s reorder diagnostic
lacked: its "pending lower bound" can become an exact assertion.

## 2. `table_job_streaming` refuses tables above `BLOCK_ROWS`

`table_job_streaming` (`tessera-io/src/stream.rs:130-170`) keeps its signature and, at or below `BLOCK_ROWS`
rows, its bytes. Above that it fails with
`Error::StreamedTable { reason: StreamedTableError::NeedsPartition { rows, max } }`, whose message names
`StreamWriter::push_table`. The alpha.4 "one block of any size" contract is withdrawn (ADR-0064 §5.4). The job
runs on a worker, so the error surfaces at `finish`, not at `push_staged`. It is `#[deprecated]`, so a build
with `-D warnings` flags every call site.

`table::encode_streaming` is not deprecated. It is still the right call for a table that fits in memory, and
`table::encode_streaming_to` (`table.rs:1096`) streams one block to any `std::io::Write`.

## 3. Names: `pairs/NNNN` leaves, `pairs` stays the one logical name

A table above `BLOCK_ROWS` rows is sealed as the leaves `pairs/0000`, `pairs/0001`, … (ADR-0064 §1). The leaf of
index `i` is `format!("{i:04}")`: four digits, five past 9999 (`pairs/10000`), never a fixed width. The manifest
lists the leaves in index order, which is the logical row order. 2.4 G rows are 573 leaves, `pairs/0000` to
`pairs/0572`. A table of at most `BLOCK_ROWS` rows is still the one block `pairs`.

Every logical reader resolves `pairs` through one function, `Reader::table_group`
(`tessera-io/src/container.rs:445`, over `tessera_core::block::table_group`), and presents the leaves as one
table:

| Surface | Call | Since |
| --- | --- | --- |
| Rust | `reader.logical_table("pairs")?` → `LogicalTableView` (`tessera-io/src/multiblock.rs:392`): `row_count`, `column(&mut reader, "pkt")`, `take`, `select_blocks_overlapping` | alpha.5 |
| CLI | `tsra read FILE pairs`, `tsra sql FILE "SELECT … FROM pairs"` (`--features sql`); `ls`/`tree` show one `pairs/  table, 573 blocks, … rows` node | alpha.5 |
| Python | `tessera.open(p).table("pairs")`, `table_arrow`, `table_dict`, `column("pairs", "pkt")`, `logical_column("pairs", "pkt")` (`tessera-py/python/tessera/__init__.py:123-152`) | alpha.5 |

One leaf still reads by its full name (`reader.read_block("pairs/0003")`). [Tables & SQL](./tables-sql.md) and
[Bindings](./bindings.md) show the read paths.

**Legacy `_NNNN` products still read.** Products written before alpha.5 by tessera's multi-block writers (GE
ingest's `events_0000`, …, `TableMultiBlockSink`) resolve under the same name, in manifest order, for the life
of format v0 (ADR-0064 §4). No product is rewritten. A product holding both `pairs_NNNN` and `pairs/NNNN` is
refused as `LogicalTableError::Ambiguous`, never merged.

**Readers need alpha.5 too.** An alpha.1 to alpha.4 reader opens and verifies a new product (ADR-0064 §6: the
marker is an optional `spec` field an old reader keeps when it checks the seal) and reads each leaf
by its full name. For the logical `pairs` it fails with "no blocks for prefix", never with wrong rows. Move your
reading tools to alpha.5 together with the writer.

**Hashes.** Leaf names and markers are bound by `manifest_hash`, not by `content_hash`. The same rows written as
one alpha.4 block and as alpha.5 leaves are different payloads, so both hashes differ. Between alpha.5's
writers, the leaves are byte for byte the batch partition `TableMultiBlockSink` writes, and `content_hash` doesn't
depend on the worker count (ADR-0064 §6). For one consequence of how Cargo resolves vortex, see
[Depending on alpha.5](#6-depending-on-v010-alpha5).

## 4. Two source-level breaks

### `TableSpec` has a new public field

`TableSpec` (`tessera-core/src/block/table.rs:138-149`) gains
`partition: Option<Partition>`, the per-leaf marker `{"index": i, "last": b}`
(`tessera-core/src/block/partition.rs:19-27`). `TableSpec` has no `Default` and isn't `#[non_exhaustive]`, so a
struct literal outside tessera stops compiling until it names the field. In visionx-rs that is
`crates/visionx-tsra/src/metadata.rs:199-203` on `main`:

```rust,ignore
TableSpec {
    columns,
    rows,
    row_index: None,
    partition: None, // new in alpha.5; push_table sets each leaf's marker itself
}
```

`None` is skipped when serialized, so a single-block spec seals byte for byte as before.

### `logical_table` errors are typed: `Error::LogicalTable`

`Reader::logical_table` used to report a name it couldn't resolve as `Error::Container` with a message
("no blocks for prefix …"). It now returns
`tessera_core::Error::LogicalTable { name, reason }` (`tessera-core/src/error.rs:130-137`), with a
`tessera_core::block::LogicalTableError` reason (`partition.rs:93-184`): `NotFound`, `NotATable { kind }`,
`Ambiguous`, or one of the invalid-partition reasons (`Gap`, `NoTerminal`, `ForeignChild`, `SchemaMismatch`,
…). Both enums are `#[non_exhaustive]`, so keep a wildcard arm:

```rust,ignore
use tessera_core::{block::LogicalTableError, Error};

match reader.logical_table("pairs") {
    Ok(view) => { /* … */ }
    Err(Error::LogicalTable { reason: LogicalTableError::NotFound, .. }) => { /* no such table */ }
    Err(e) => return Err(e.into()),
}
```

Code that matched the `Error::Container` string must match `LogicalTableError::NotFound` instead. An array block
passed as a table is now `NotATable { kind: "array" }`. `Reader::block_group` is deprecated in favour of
`table_group`, since it sees only the legacy `_NNNN` form, and `LogicalTableView::prefix` in favour of `name`.

## 5. The memory bound, and its measurement

For `push_table` with `W` encode workers writing a table of `n` leaves, the heap is bounded by (rustdoc of
`push_table`, `stream.rs:883-889`; ADR-0064 §8 as amended by #760):

```text
peak heap  ≤  min(W, n) · p_block  +  s  +  c_leaf · n
s          =  2 · ROWS_PER_GROUP · r_mem + 4 MiB            (7 MiB for 24-byte rows)
c_leaf     =  2 · (e_leaf + b_leaf),  b_leaf = 1 KiB
```

- `p_block` is one leaf's measured write peak in the same binary: it depends on the host's CPU count and the
  table's width, not on its row count.
- `r_mem` is the input's bytes per row.
- `e_leaf` is a leaf's canonical manifest entry. For the consumer's 12-column schema it is at most 2,966 B, so
  `c_leaf` = 7,980 B.

Beyond the per-leaf term, which is 4.6 MB at 573 leaves, the bound doesn't grow with the rows. The staged input
is on disk, not in the heap (section 1).

**Measured on ksb** (ADR-0064 §8.1 RSS proof, [#715] G r4, 2026-11-11, recorded in
[ADR-0026][ADR-0026-measured]):

- **Setup.** The consumer reproducer at `c4aff94` with the switch above (exact nullable schema), built against
  tessera `26159de`, the post-#759 writer, which `v0.1.0-alpha.5` contains with no change to `tessera-io` or
  `tessera-core` sources. Run with `ksb run --cpu 4 --exclusive-cpus`.
- **Staging.** On disk-backed `--scratch` under `--mem 4Gi`. The parallelism run used `/dev/shm` under `--mem 6Gi`.

| Job | Runs | Exit |
| --- | --- | ---: |
| `tessera-715-r4-sweep-w4-2` | `W = 4`, every point, 8 of its 9 repeats | 124 (480 min limit) |
| `tessera-715-r4-sweep-w4-k2` | `W = 4`, 2.4 G rows, repeat 3 | 0 |
| `tessera-715-r4-sweep-w1-k0` … `-k5` | `W = 1`, repeats 1 to 6 | 0 each |
| `tessera-715-r4-par-1` | parallelism, `W = 1` and `W = 4` at 268 M rows on `/dev/shm` | 0 |

`run.py evaluate` over the eight sweep shards: 83 checks, all pass, exit 0. Over `tessera-715-r4-par-1`: exit 0.

| What | Measured |
| --- | --- |
| Highest peak `RssAnon`, `W = 4`, 573 leaves (2.4 G rows) | **218.6 MiB**, against the 305.5 MiB ceiling and your 8 GiB budget |
| Highest peak `RssAnon`, `W = 1` | 64.3 MiB, against 114.9 MiB |
| Growth with the table (within-run slope, graded) | `W = 4`: −5,082 B/leaf; `W = 1`: 3,899 B/leaf; both within `c_leaf` plus margin |
| Page cache, 2.4 G rows | all 9 runs complete under `--mem 4Gi`; peak `shmem` 0.0 MiB; cgroup `anon` within 1.0 MiB of `RssAnon` |
| Bytes and names, 268 M rows | the 9 runs at `W = 1` and `W = 4` seal one canonical manifest and one `content_hash` |
| Parallelism, 268 M rows on `/dev/shm` | `W = 1` 42.9 s, `W = 4` 15.3 s: 2.80× |

The across-run slope (each run's peak against its `n`) is reported as context, not graded: 40,437 B/leaf at `W = 4`,
outside the slope rule, because across runs the peak tracks the shared disk's load, 99–100 % busy (ADR-0064 C.1);
5,072 B/leaf at `W = 1`, within it.

For comparison, [#677] measured 4,758,114,304 B (4.4 GiB) peak `RssAnon` at 268 M rows on alpha.3's single-block
path, with `--nonnullable-ms` (the nullable schema hit a dtype assertion at `9424f90`) under an 8 GiB address-space
cap. On ksb's shared HDD the wall times are context, not a benchmark (ADR-0064 A.4). At 268 M rows, the `W = 4`
runs took 848–1,451 s.

## 6. Depending on v0.1.0-alpha.5

The release is [v0.1.0-alpha.5], published 2026-11-11 with prebuilt `tessera` binaries. Its notes (the root
`CHANGELOG.md`) list the `push_table`, naming, `TableSpec` and error changes on this page. The `BlockName` change
is in `tessera/crates/tessera-core/CHANGELOG.md`.
Tessera isn't published to crates.io yet, by design (see `release-plz.toml`), so depend on the git tag, not on
`branch = "dev"`, which moves:

```toml
[dependencies]
tessera-core = { git = "https://github.com/vig-os/tessera", tag = "v0.1.0-alpha.5" }
tessera-io   = { git = "https://github.com/vig-os/tessera", tag = "v0.1.0-alpha.5" }
```

Then run `cargo update -p tessera-core -p tessera-io` to move `Cargo.lock` off the old pin. visionx-rs
`main` pins `branch = "dev"` at `4504efb` (between alpha.4 and alpha.5); PR #41 at `c4aff94` pins `9424f90`
(alpha.3).

**Known limit: vortex resolves from crates.io in your build ([#758]).** Tessera pins vortex to a fork carrying
two determinism fixes (#468, #472) through a `[patch.crates-io]` block in its own workspace manifest
(`tessera/Cargo.toml:148-195`). Cargo applies `[patch]` only in the root workspace, so a dependent resolves
vortex 0.75.0 from crates.io without those fixes. The RSS proof above was built that way. Two consequences:

- A table holding both `+inf` and `-inf` in one float column can seal a different `content_hash` on x86-64 than
  on aarch64.
- A debug build can seal different bytes than a release build.

The consumer's schema has no float columns, and release builds are unaffected by the second defect. Until #758
lands, a dependent that needs tessera's exact bytes can copy that whole `[patch.crates-io]` block, at the same
`rev`, into its own root `Cargo.toml`. Copy every vortex crate in it, since patching only some of them gives two
copies of the shared vortex crates. `cargo tree -i vortex-array` then shows
`vortex-array v0.75.0 (https://github.com/gerchowl/vortex?rev=608d8a1…)` instead of a bare `vortex-array v0.75.0`.

[#677]: https://github.com/vig-os/tessera/issues/677
[#715]: https://github.com/vig-os/tessera/issues/715
[#758]: https://github.com/vig-os/tessera/issues/758
[PR #41]: https://github.com/MorePET/visionx-rs/pull/41
[ADR-0064]: https://github.com/vig-os/tessera/blob/dev/docs/adr/0064-logical-tables-over-numbered-leaves.md
[ADR-0026-measured]: https://github.com/vig-os/tessera/blob/dev/docs/adr/0026-streaming-table-ingest.md
[v0.1.0-alpha.5]: https://github.com/vig-os/tessera/releases/tag/v0.1.0-alpha.5
