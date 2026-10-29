# ADR-0062 — Streaming table provider: bounded-memory queries over sealed table blocks

**Status:** Accepted (2026-10-29, design for epic [#654](https://github.com/vig-os/tessera/issues/654)).
**History:** Proposed 2026-10-25 in #661; Accepted 2026-10-29 with the owner's decisions of 2026-10-26 on the
signed alignment (§5, declared mode), the local integrity default (§7) and key mode under an early stop (§5,
"The release gate"). Builds on **ADR-0026** (fixed row groups, `BLOCK_ROWS` decomposition), **ADR-0034** (sync
public API; tokio only where it earns its keep), **ADR-0049** (recursive collections), **ADR-0057** (the thin
waist, the feature graph and Gates A/B) and **ADR-0059** (chunk-index statistics, in flight as PR #539).
Consumes [#225](https://github.com/vig-os/tessera/issues/225) (object-store range reader),
[#627](https://github.com/vig-os/tessera/issues/627) / [#632](https://github.com/vig-os/tessera/issues/632)
(NULLs in chunk statistics) and [#251](https://github.com/vig-os/tessera/issues/251) (`tessera sql`).
[#657](https://github.com/vig-os/tessera/issues/657) (recipe-only products) will execute through this
provider. The first consumer is MorePET list-mode: ~2.4×10⁹ rows per acquisition, joins of two row-aligned
blocks, cohort queries over many `.tsra`.

> Design only; §8 is the landing plan. Every claim about current behaviour is cited to `file:line` at `dev`
> `9424f90`. Every claim about DataFusion or Vortex is cited to the crate version pinned in
> `tessera/Cargo.lock` (DataFusion **54.0.0**, arrow **58.3.0**, the Vortex **0.75.0** fork at `608d8a1`,
> object_store **0.14.0** for `tessera-io`, zip **9.0.4**) and an item path in that crate's source. Seven
> claims were **probed** rather than read: Vortex's default split size, `SplitBy::RowCount`, the bytes a
> zone-pruned filter reads, exact row filtering (including `IS NULL`), reuse of one opened `VortexFile` across
> scans, one split per row-range window scan with its set-up cost, and the bytes window scans read with and
> without Vortex's layout-reader and segment caches.
> The probe was a scratch crate outside the repository, depending on `tessera-io` at `9424f90` and the
> same Vortex fork, and is not part of this change. It sealed one 1 049 353-row table block (17 row
> groups; a monotone `i64`, a random `i32`, a nullable `i16` with 20 % NULLs, an `f64`) with
> `tessera_io::table::encode`, then scanned it through a byte-counting `VortexReadAt`, release build.
> Its results are quoted where used (§3, §4).

## Context

`tessera sql` (#251) builds the `LogicalTableView` the `read` verb uses, decodes **every column of every
block** into one Arrow `RecordBatch` (`tessera-cli/src/sql.rs:161-183`), wraps it in a DataFusion
`MemTable` (`sql.rs:195`) and only then runs the query. The module says so itself: "Do NOT ship this as the
big-cohort path" (`sql.rs:9-14`). At consumer scale that is >50 GB of RAM per query, and a cohort of many
`.tsra` cannot be expressed at all.

The pieces a streaming provider needs mostly exist, but none is wired for it:

1. **The read path is whole-block.** `Reader::read_block` reads a block member into a `Vec<u8>` and
   checks its digest (`tessera-io/src/container.rs:456-489`). The table decoders take that `&[u8]` and
   copy it again into a Vortex buffer (`table.rs:1350-1376`, `open_buffer(ByteBuffer::copy_from(blob))`).
   A block is ≤ `BLOCK_ROWS = 2²²` rows (`table.rs:811`), so this is *bounded*, but the bound is "one
   whole block, every projected column, fully decoded", not "a few batches".
2. **Block-level pruning exists for one column.** `LogicalTableView::select_blocks_overlapping`
   (`multiblock.rs:225`) skips blocks using the `.cidx` sidecar's roll-up, and only when the predicate's
   column is the block's `row_index` (`multiblock.rs:284`), because the sidecar records no column name.
3. **Vortex can already do most of the scan work.** The table encoder writes through Vortex's default
   `WriteStrategyBuilder` (`table.rs:908-910`), which persists **zone maps every 8192 rows on every column**
   (`vortex-file/src/strategy.rs:160` `row_block_size: 8192`, `:282` `ZonedStrategy`). Vortex's
   `ScanBuilder` takes a projection, a filter, row ranges, a split policy and a concurrency limit
   (`vortex-layout/src/scan/scan_builder.rs:123-200`) and reads through any `VortexReadAt`
   (`vortex-io/src/read_at.rs:56`), not only an in-memory buffer (`vortex-file/src/open.rs:215`,
   `open_read`).
4. **The container is range-addressable.** Every `.tsra` member is STORED (enforced at open,
   `container.rs:330-381`), so a block's payload is one contiguous byte range of the file:
   `ZipFile::data_start()` + `compressed_size()` (`zip-9.0.4/src/read/zipfile.rs:337`, `:172`; `data_start`
   is an `Option`, filled once the entry has been located, so the planner opens each entry before reading
   its offset).

The epic adds new **public library API** with a **public DataFusion dependency**. That is why it starts
with an ADR: the crate it lives in, the semver promise it makes, and what "bounded memory" means are
expensive to change once a Rust consumer depends on them.

## §0 — The decision, in one place

| # | Question | Decision |
|---|---|---|
| 1 | Crate home | Engine-neutral sync `tessera_io::scan`; a new `tessera-query` holds only the DataFusion adapter |
| 2 | DataFusion coupling | Only `tessera-query` exposes it, re-exported; one major at a time; a major breaks |
| 3 | Bounded memory | Per query: ≤ `max_partitions` × a row budget of 65 536-row windows; anon-RSS test |
| 4 | Pushdown, pruning | Projection; a closed filter class (no floats, casts) via `.cidx`, zone maps; `Inexact` |
| 5 | Positional zip | Sealed `aligned_to` (derive paths only) or a key-only pre-pass proving a strict increase |
| 6 | Multi-product | (product, block, row-group run) units; members pinned by `manifest_hash`; identical schemas |
| 7 | Integrity | `VerifyBlocks`: local default, unspooled; object-store default pending #672; `SealOnly` opt-out |
| 8 | Phasing | #666 single product + pruning → #667 positional zip → #668 multi-product, local then S3 |

Each row is argued in its section, with the options and the rejected alternatives. One limit belongs here,
not only in §7: under **local** `VerifyBlocks`, the default for local sources, the rows are read in a second
pass after the hash, so "no row from unverified bytes" holds against corruption at rest, **not against a
concurrent writer**; a caller reading a remote or untrusted local source opts into the spool (§7).
That limit reaches the key-mode release gate of §5 too (decision (c) under (b)): on the default local
unspooled path the pre-pass and the row pass read the file separately, so against a concurrent writer that
swaps rows between them the row pass falls back to row-by-row key checks, which do not exclude a misaligned
pair. The spool opt-in closes it. Object-store scans under `VerifyBlocks` (their default pending #672) read
rows from the verified spool, so they have no such gap.

## §1 — Crate home and feature gating

### Options

| Option | What it is | Verdict |
|---|---|---|
| **A** | A `query` feature on `tessera-io` holding everything, DataFusion included | rejected |
| **B** | A `tessera-query` crate holding everything, Vortex scan included | rejected |
| **C** | Engine-neutral scan in `tessera-io`; DataFusion adapter alone in `tessera-query` | **recommended** |
| **D** | Adopt upstream `vortex-datafusion` 0.75 | rejected |

### Recommendation: C

**`tessera_io::scan`** (a module, no feature, no new dependency besides promoting `arrow-array` /
`arrow-schema` from transitive to direct) owns everything that touches Vortex, the container or the seal:
opening a block as a byte range, the `ScanBuilder` configuration, `.cidx` trust and pruning, digest
verification, the row-alignment check, member resolution and partition planning. Its public surface is
synchronous, iterator-shaped and engine-neutral:

```rust
// tessera-io — sync, no DataFusion, no tokio. Sketch; names are PR1's to finalise.
pub struct ScanOptions {
    pub projection: Option<Vec<String>>,   // None = every column
    pub predicate: Option<Predicate>,      // the pushable class of §4 only
    pub limit: Option<u64>,
    pub batch_rows: NonZeroUsize,          // DataFusion passes batch_size
    pub budget: ScanBudget,                // §3
    pub integrity: Integrity,              // §7
}
#[non_exhaustive]
pub enum Predicate {
    Compare { column: String, op: CmpOp, value: Literal },
    InList { column: String, values: Vec<Literal> },
    IsNull(String),
    IsNotNull(String),
    And(Vec<Predicate>),
    Or(Vec<Predicate>),
}
#[non_exhaustive]
pub enum Literal { Int(i64), UInt(u64), Bool(bool), Utf8(String) }

impl LogicalTableView {
    /// Plan the scan: manifest + `.cidx` only, no block payload is read.
    pub fn plan_scan(&self, opts: &ScanOptions) -> Result<ScanPlan>;
}
impl ScanPlan {
    pub fn schema(&self) -> arrow_schema::SchemaRef;
    pub fn units(&self) -> &[ScanUnit];   // work items, §6
    pub fn open<S: BlockSource>(&self, src: S, units: &[ScanUnit])
        -> Result<TableScan>;
}
/// `Iterator<Item = Result<arrow_array::RecordBatch>>`; `metrics()` is live.
pub struct TableScan { /* … */ }
```

**`tessera-query`** is a new workspace member that depends on `tessera-io` and `datafusion`, and contains
only the translation: DataFusion `Expr` → `Predicate`, a `TableProvider`, an `ExecutionPlan` whose
partitions drive `TableScan`s, the metrics bridge, and registration helpers for a product, a zip and a
collection. `tessera-cli`'s `sql` feature becomes `["dep:tessera-query"]` and `sql.rs` keeps only argument
handling and output formatting.

### Why C, under ADR-0057's rules

- **The WASM / thin-waist build stays DataFusion-free by construction.** `tessera-wasm` depends on
  `tessera-core` alone (ADR-0057 §3); neither option touches core. The pure-abi3 wheel depends on
  `tessera-io` and stays DataFusion-free too: under C, DataFusion is not even an optional dependency of the
  crate the wheel links.
- **One owner for Vortex sessions.** The read path's runtime and session handling is subtle enough to have
  produced a real bug: a short-lived runtime poisoned the pooled read session
  (`table.rs:661-676`, regression test `short_lived_runtime_does_not_poison_pooled_session`). Option B would
  put a second, independent user of Vortex sessions in another crate, with direct `vortex-*` dependencies.
  Under C, `tessera-query` never names a Vortex type.
- **ADR-0057 §3 is respected.** §3 keeps *canonicalisation* (foreign types → canonical columns, the write
  direction) out of `tessera-io`. This is the read direction — exporting already-sealed values to Arrow —
  which `tessera-io` already does (the `LogicalTableView` API that `sql.rs:3-7` builds on) and which cannot
  move a sealed byte.
- **The new crate meets ADR-0057 §2's extraction triggers on day one.** Trigger 2: Rust consumers outside
  the CLI (MorePET). Trigger 3: a release cadence driven by DataFusion's majors, not ours. Trigger 4: an
  embedder auditing `tessera-io` should not have to reason about DataFusion's ~30 crates. §2 declined a
  `tessera-arrow` crate because a feature delivered the same property; here a feature does not (see A).
- **The engine-neutral layer has other consumers.** The CLI's `read`/`export` verbs, the Python binding
  and #657's recipe executor can stream batches without DataFusion.

### Rejected

- **A — a feature on `tessera-io`.** DataFusion becomes an optional dependency of the seal-path crate,
  and every public DataFusion type sits in `tessera-io`'s API: a DataFusion major is then a breaking
  change of the crate *everything* depends on, including users who never query. It also mixes "query
  engine adapter" into the crate whose output is sealed, which is the responsibility argument ADR-0057 §3
  used to keep canonicalisation out.
- **B — everything in `tessera-query`.** Duplicates the Vortex read configuration in a second crate
  (see "one owner" above), forces `tessera-query` to depend directly on `vortex-*` crates, and leaves the
  CLI and Python without a DataFusion-free streaming read.
- **D — `vortex-datafusion` 0.75.** It is real and targets DataFusion 54 (`vortex-datafusion/src/v2/
  table.rs`, workspace `datafusion = "54"`), but: it enables `tokio/rt-multi-thread`; it needs the `vortex`
  umbrella crate, which our fork pin does not patch, so the 27-crate `[patch]` block grows; and it models
  "one Vortex file per object", while ours is "a Vortex file inside a STORED zip member, behind a seal". It
  knows nothing of `.cidx` pruning, block verification or row alignment, so we would wrap it as deeply as we
  would write C. (Its object_store **0.13** is not a reason against it: DataFusion 54 already locks
  object_store 0.13.2 beside `tessera-io`'s 0.14.0 — `datafusion`, `-catalog`, `-common`, `-datasource*`,
  `-execution` 54.0.0 all depend on 0.13.2 in `tessera/Cargo.lock` — so any `sql` build carries both copies
  today. §6 deals with what that split costs a consumer.)

**Honest cost of C:** a 7th workspace member to version, lint and document; one more `[features]` row in
the CLI. And PR1 trips **Gate B**: promoting `arrow-array`/`arrow-schema` to direct dependencies of
`tessera-io` and adding `tessera-query` to the workspace changes the inverted trees that
`scripts/feature-snapshots.sh` records. Both are inert (no feature changes on the seal path); PR1 must
regenerate the snapshots and show the delta adds only new dependent edges, with zero removed lines.

## §2 — DataFusion coupling and version policy

### The coupling is unavoidable for the adapter, so confine it

A `TableProvider` *is* a DataFusion type (`datafusion-catalog-54.0.0/src/table.rs:51`). Any useful
DataFusion integration therefore puts DataFusion in the public API of the crate that offers it. §1
confines that crate to the adapter; the engine-neutral `tessera_io::scan` carries **arrow** types only.
Arrow is not a new coupling: the workspace already pins `arrow-array`/`-buffer`/`-schema` with `=58.3.0`
because Vortex resolves them (`tessera/Cargo.toml:82-92`).

### Options

| Option | Verdict |
|---|---|
| **Expose and re-export:** implement `TableProvider`, re-export `pub use datafusion;` | **recommended** |
| **Hide it** behind our own query API (`sql(&str) -> stream`), DataFusion private | rejected |
| **Expose without re-export** (consumer brings their own `datafusion`) | rejected |

**Recommended: expose and re-export.** A consumer that registers our table into *their* `SessionContext`
must share our DataFusion version, and `tessera_query::datafusion` hands them exactly that version without
a version-matching exercise. Hiding DataFusion behind a SQL-string API would make the provider useless to
the main consumer, who joins Tessera tables with their own tables and UDFs (#657 adds UDF versions to the
recipe). Exposing without a re-export makes every consumer guess the right major.

### The version policy

1. **One DataFusion major at a time**, declared caret-on-major (`datafusion = "54"`, the CLI's current
   requirement, `tessera-cli/Cargo.toml:58`). The lockfile pins the exact version. Features stay minimal:
   `default-features = false, features = ["sql", "recursive_protection"]`, as today.
2. **DataFusion's arrow major must equal Vortex's.** Two arrow majors cannot link in one graph
   (`RecordBatch` is a different type in each; `tessera-cli/Cargo.toml:53-57`). So a DataFusion major
   that moves arrow waits for a Vortex release on the same arrow major. That ordering is a constraint
   to state, not to work around.
3. **A DataFusion major is a breaking `tessera-query` change**, committed as `feat(query)!: …` with the
   DataFusion migration in the same PR. Under ADR-0052 §2's unified workspace version that bumps the whole
   workspace; pre-1.0 that is the normal pre-release increment.
4. **At 1.0 the unified version stops fitting.** DataFusion ships majors far more often than Tessera
   should. The recommendation is that `tessera-query` leaves the unified version at the 1.0 cut and keeps
   its own line. That is an exception to ADR-0052 §2, so it is filed for the owner as #673.
5. **MSRV.** No crate declares `rust-version` today and the toolchain is `stable` from the flake
   (`tessera/rust-toolchain.toml`). DataFusion 54 requires **1.88.0** (`datafusion-54.0.0/Cargo.toml`
   `rust-version`) and Vortex 0.75 requires **1.91.0** (Vortex workspace `rust-version`), so DataFusion adds
   no constraint today. A DataFusion bump that raises its floor above the flake's toolchain waits for the
   toolchain. Whether the workspace should declare an MSRV at all is #674.

**Honest cost:** every DataFusion major is a migration PR (their `TableProvider`/`ExecutionPlan` traits
change across majors) and a breaking release for query users. The engine-neutral layer limits the blast
radius to the adapter.

## §3 — The bounded-memory contract

### What is bounded

The scan bounds **rows in flight**, not bytes. A byte bound cannot be promised honestly: a `utf8` column
has no fixed width, and the probe showed decoded size has no useful relation to compressed size (one
`i64` sequence column of 1 049 353 rows read **1 112 bytes**; a random `i32` column of the same table read
1.31 MB).

The probe also showed that Vortex's default split does **not** follow our row groups. With
`SplitBy::Layout` (the default, `scan_builder.rs:87`) a full scan of a 1 049 353-row, 17-group table came
back as 131 072-row splits, and projections of one `i32` and one `i16` column as **262 144-row** and
**524 288-row** splits. The writer coalesces each column into segments of about 1 MiB of *uncompressed*
data before compressing them (`strategy.rs:250-264`, `canonicalize: true`), so a default split is roughly
1 MiB ÷ the column's width: ~1 M rows for an `i8`, a quarter of a block. With
**`SplitBy::RowCount(65 536)`** the same single scans came back as exactly 65 536-row splits (plus the
777-row tail), with identical bytes read. So the provider pins the split size to its window
(`with_split_by(SplitBy::RowCount(rows_per_window))`, `vortex-layout/src/scan/split_by.rs:18-24`): it
becomes a chosen constant instead of a consequence of the narrowest column's width.

One more Vortex default makes `ScanBuilder::with_concurrency` unusable as the bound: when the scan stream
starts, Vortex multiplies it by the **process's core count** (`scan_builder.rs:367-368`, `concurrency =
builder.concurrency * num_workers`, with `num_workers` from `get_available_parallelism()`), a value cached
once from `std::thread::available_parallelism` and not configurable (`vortex-utils/src/parallelism.rs:13-18`).
Spawned split tasks keep running on the runtime's pool while the consumer is blocked; only
`buffered(concurrency)` (`scan_builder.rs:389`) holds them back. On an 88-core ksb node one scan with
Vortex's default `concurrency = 4` may therefore have 352 splits in flight, and DataFusion runs one
partition per core by default. A bound that depends on the host's core count is no bound.

### Options

| Option | Verdict |
|---|---|
| **Explicit split windows:** one row-range scan per 65 536-row window, ≤ `windows_in_flight` open | **recommended** |
| One scan per unit, bounded by `with_concurrency`, with the core-count factor written into the formula | rejected |
| A byte budget (decoded bytes in flight) | rejected |
| No scan bound; rely on DataFusion's memory pool | rejected |

**Recommended: explicit split windows.** The scan thread opens each block's `VortexFile` once (`scan` takes
`&self`, `vortex-file/src/file.rs:184`) and issues one row-range scan per 65 536-row window, the same
windows tier 1 already reasons in (§4). With the split size pinned to the window, a window covers exactly
one split: a row-range scan is still split inside its range by the scan's `SplitBy`
(`vortex-layout/src/scan/scan_builder.rs:270-282`), and with `RowCount(rows_per_window)` that yields the one
range (`split_by.rs:49-53`). This rests on `RowCount`: under `SplitBy::Layout` a window would be re-split at
layout boundaries. So Vortex's internal multiplier
has nothing to multiply: the number of splits in flight is the number of window scans the thread has
started and not yet drained, which the thread controls. The probe confirmed that a row-range scan of one
window yields a single split, and measured the cost of the extra scan set-up: 17 window scans took
2.63 ms where one scan of the same block took 0.71 ms (full width), and 1.14 ms vs 0.19 ms for one
column. That is ~0.06–0.11 ms per window, measured *without* decoding the fields, so it overstates the
relative cost; PR1 measures it against a full decode and may set `rows_per_window` to a multiple of
65 536 rows. The window size is then the knob, and the bound stays explicit.

**Windows need Vortex's caches, or they re-read data.** A segment holds ~1 MiB of one column, so it spans
several 65 536-row windows (2 for an `i64`, 8 for an `i16`), and by default each window scan builds a fresh
layout-reader tree (`open.rs:82`, `cache_layout_reader: false`; `file.rs:143-149`), reloads the zone tables
and fetches whole segments with no segment cache (`open.rs:216-219`, `NoOpSegmentCache`). The probe read the
same block through 17 window scans:

| Configuration | full width | one `i32` column | one nullable `i16` column |
|---|---|---|---|
| one scan of the block (reference) | 8.79 MB | 1.31 MB | 0.39 MB |
| window scans, defaults | 22.57 MB (2.6×) | 5.25 MB (4.0×) | 3.15 MB (8.0×) |
| window scans, `with_layout_reader_cache()` only | 22.57 MB | 5.25 MB | 3.15 MB |
| window scans, layout-reader cache + 4 MiB `MokaSegmentCache` | **8.79 MB** | **1.31 MB** | **0.39 MB** |

So each block is opened through `open_read` (not `open_buffer`, which ignores a segment cache with only a
warning, `vortex-file/src/open.rs:187-188`) with `with_layout_reader_cache()` and **a fresh
`MokaSegmentCache` of its own** (`vortex-layout/src/segments/cache.rs:46-62`). The cache must never be
shared between blocks: its key is `SegmentId(u32)` (`vortex-layout/src/segments/mod.rs:26`), unique only
within one file, so a shared cache would serve one block's segment to another, i.e. wrong rows. A
partition keeps **at most two blocks open** (the current one, and the next while prefetching across a
block boundary; a key-mode zip's pre-pass, §5, uses the same two slots for the key column and finishes
before the row pass opens any block; on a spooled source each of them also holds a spool slot, which
`plan_scan` counts, §7 "Spool slots"), and the cache is sized to hold the segments the in-flight windows share:
`segment_cache_bytes = projected columns × 1 MiB × (windows_in_flight + 1)` per open block, which goes into
the memory formula below. 1 MiB is a sound bound for a fixed-width column's compressed segment (the writer
cuts at ≥ 1 MiB uncompressed in 8192-row steps, then compresses). It is **not** a bound for `utf8`: 8192
long strings make one segment far larger. Memory stays capped by the cache's capacity regardless; what a
too-small cache costs is re-reads, so PR1's byte-equality test must include a `utf8` column and a
dictionary-encoded one, not only the fixed-width probe table.

With the cache, window scans read exactly what one scan reads. The cache's eviction is TinyLFU,
which admits by frequency; the probe's 4 MiB cache was smaller than the 8.8 MB block and still matched, but
PR1 keeps "window bytes == single-scan bytes" as an acceptance test so a cache that stops admitting a
sequential scan shows up as a failure, not as silent extra I/O.

The rejected options: writing `× available_parallelism()` into the formula makes the bound depend on the
host (11.5 M rows per partition on 88 cores with Vortex's defaults) and can't be tuned down. A byte budget
fails for `utf8` and for compression (above). Relying on DataFusion's pool alone leaves the scan, the
largest consumer, unaccounted, and the pool is advisory (below).

The knobs, per DataFusion partition:

```rust
pub struct ScanBudget {
    /// Rows per split window; a multiple of 65 536. Default 65 536.
    pub rows_per_window: NonZeroUsize,
    /// Window scans started and not yet drained. Default 2:
    /// one decoding, one prefetching.
    pub windows_in_flight: NonZeroUsize,
    /// Vortex pool threads decoding this partition's windows.
    /// Default 1: parallelism comes from DataFusion partitions.
    pub workers: NonZeroUsize,
    /// Batches buffered for the consumer. Default 2.
    pub batches_buffered: NonZeroUsize,
    /// Most scan partitions one query may run (per query, not per
    /// partition). Default 8, whatever `target_partitions` says.
    pub max_partitions: NonZeroUsize,
}
```

**The contract is per query.** DataFusion asks for `target_partitions` partitions, and that defaults to the
core count (`datafusion-common-54.0.0/src/config.rs:514-515`): 88 partitions of ~29 MiB each would be ~2.5 GiB
on an 88-core ksb node and a fraction of that on a laptop, so a per-partition bound is not a bound on the
query. The scan therefore exposes `P = min(target_partitions, max_partitions, units)` partitions, and the
contract, for a projection of width `w(row)`, a key of width `w(key)` and `k` zip inputs (`k = 1` without
a zip), is:

```text
P              =  min(target_partitions, max_partitions, units)
rows(p)        =  windows_in_flight * rows_per_window
                  + batches_buffered * batch_rows
mem(p)         =  rows(p) * w(row)
                  + k * open_blocks * segment_cache_bytes
                                         (open_blocks <= 2)
                  + k * open_blocks * reader_state
                    (footer, layout tree, decoded zone
                     maps, decoded dictionary values)
                  + k * windows_in_flight * rows_per_window
                      * w(key)            (key-mode zip, §5:
                                         pre-pass or row pass)
                  + 64 KiB hash ring    (VerifyBlocks, §7)
                  + spool_chunk_bytes   (object-store
                                         spool download or
                                         local spool opt-in,
                                         §7)
mem(scan)      =  P * mem(p)
disk(spool)    =  spool_blocks * block_bytes
```

A partition waiting on another partition's verification of the same block (§7) holds nothing extra: it opens
the block only once the verify-once cell resolves, and its buffered batches are already in `rows(p)`.

With the defaults (`rows_per_window = 65 536`, two windows, two buffered 8192-row batches, `P ≤ 8`) and a
32-byte, four-column fixed-width list-mode projection, that is 147 456 rows ≈ 4.5 MiB of rows plus
2 × 4 × 1 MiB × 3 = 24 MiB of segment caches per partition, ≈ 28.5 MiB, so **≈ 228 MiB for the query's
scan** plus the reader state. Zone maps are one row of five statistics per 8192 rows per column, ~512 rows
per column per block, so ~20 KiB per column per open block; dictionary values depend on the data. PR1
measures `reader_state` on the test table, and the ksb job writes that number into its expected bound
before the run. All
of it is independent of the table's length **and of the host**, because `P` is capped by our knob and not by
the core count. It grows with the projection's width and with `max_partitions`, linearly and by a stated
amount.

A byte budget for the whole query, divided across its partitions, is rejected
for the reason a byte bound is rejected above: a `utf8` column has no width to divide by.

**Honest cost:** with `workers = 1` one partition decodes on one thread, and with `max_partitions = 8` a
query decodes on at most eight threads however many cores the host has. A caller raises `max_partitions` or
`workers` and accepts the larger bound the formula states. A query over one small block decodes on as many
partitions as it has units (§6).

### How it is enforced

Each partition runs its `TableScan` on **its own OS thread**. It drives the window scans with `block_on` on
a scan-local `CurrentThreadRuntime` with a pool of `workers` threads (ADR-0034's sync bridge), starts a new
window only while fewer than `windows_in_flight` are undrained, and pushes Arrow batches into a bounded
channel of `batches_buffered`. A full channel blocks the thread, and a blocked thread starts no window.
The DataFusion side is a `RecordBatchStreamAdapter` over the channel's receiver
(`datafusion-physical-plan-54.0.0/src/stream.rs:446`). This keeps the core sync (ADR-0034 §1) and needs no
assumption about polling a Vortex-runtime stream from a tokio executor.

Vortex arrays become Arrow with `ArrowSession::execute_arrow(array, Some(field), ctx)`
(`vortex-array/src/arrow/session.rs:410`), always with the **declared** field as target, so a `str` column
is `Utf8` (matching the in-memory path's `numpy_to_arrow`, `sql.rs:97-118`), never whatever Vortex prefers.

### What is *not* bounded by the provider

**Operators above the scan (sort, hash join, hash aggregate, and any `RepartitionExec` DataFusion inserts)
are bounded only by DataFusion's memory pool and its spilling; this ADR does not change that.** They hold
their own state. `tessera-query`
exposes a `memory_limit` that sets `RuntimeEnvBuilder::with_memory_limit`
(`datafusion-execution-54.0.0/src/runtime_env.rs:416`) and a spill directory. DataFusion's own
documentation on that method says it "does not yet respect this limit in all cases". So the honest
contract is: **the scan is bounded by construction; the query is bounded only as far as DataFusion's memory
pool is**, and a query that cannot spill fails with DataFusion's resources-exhausted error rather than an
OOM kill when the limit is set. Positional zip (§5) adds no operator state, which is why it matters.

### How a test proves it

1. **CI, deterministic:** the scan records a high-water mark of rows in flight (undrained windows +
   buffered batches). A test over a multi-block table asserts `high_water ≤ the formula above` for several
   budgets, and runs on a runner with more cores than `windows_in_flight`, so a regression to Vortex's
   core-scaled concurrency would show. No RSS measurement, so no flakiness.
2. **ksb job, the issue's acceptance:** "larger than RAM" means larger than the memory the process may
   use, and what must be bounded is the process's **anonymous** memory. cgroup v2's `memory.peak` also
   counts page cache, which the VerifyBlocks hash pass and the scan's file reads fill with the whole table,
   so a cap or peak on it would be met or missed for reasons unrelated to the scan. The job therefore:
   - pins its shape in the job spec: `ksb run --cpu 8 --exclusive-cpus` (eight dedicated cores, CPU limit
     equal to the request, so `$KSB_CPUS = 8` and the job cannot burst onto idle cores; `--cpu` alone sets
     only a request), `target_partitions = 8`, the default `ScanBudget`, and the table size, so the expected
     bound is a number before the run;
   - runs the query in a cgroup with swap off (`memory.swap.max = 0`) and `memory.max = 1 GiB`. Page cache
     is reclaimable, so under that cap only anonymous and unreclaimable memory can force an OOM kill;
   - samples `anon` from `memory.stat` every 100 ms and records the largest sample, and reads `oom_kill`
     from `memory.events`;
   - uses a synthetic list-mode table whose **decoded projected size is ≥ 8 GiB**.

   It passes when (a) the query completes with `oom_kill = 0`, (b) the sampled anonymous peak is within
   the §3 formula for the pinned shape plus a measured idle baseline, and (c) the anonymous peaks at N rows
   and at 4N rows differ by less than 10 %. A **negative control** runs the same query, in the same cgroup,
   through today's materialising path (`LogicalTableView::column` into a `MemTable`, the PR1 test oracle),
   which holds the whole projected table in anonymous memory and must be OOM-killed (`oom_kill ≥ 1`). If
   it completes, the cap or the measurement does not bind, the test cannot detect a broken bound, and the
   run counts as failed. (A control built from an oversized `ScanBudget` would not be deterministic: its
   windows fill only as fast as the consumer lags.) The
   queries are a projection + aggregate (`SELECT count(*), sum(e) … WHERE t BETWEEN …`) and, in PR2, the
   zip. The commands, the cgroup settings and every measured peak go into the performance book.

## §4 — Pushdown and pruning

### Projection

Always pushed. DataFusion's `projection` indices map to column names; `with_projection(select(names,
root()))` (`table.rs:1364`) reads only those columns' segments. The probe: a one-column projection read
1.31 MB of an 8.79 MB block.

### Two pruning tiers

| Tier | Where | Granularity | Columns | Reads before deciding |
|---|---|---|---|---|
| **1** | `tessera_io::scan`, `.cidx` | block, then 65 536-row group | the indexed column | the `.cidx` only |
| **2** | Vortex, zone maps | 8192-row zone | every column | footer and zone tables |

**Tier 1** is the prune-before-fetch tier: a block whose roll-up cannot match is never opened (zero bytes
fetched — the property cohort and object-store scans need), and surviving row groups become row ranges
(`with_row_range`, one scan per contiguous run; `VortexFile::scan` takes `&self`, so the file is opened once,
`vortex-file/src/file.rs:184`, confirmed by the probe). It reuses the reader's index-trust rules rather than
re-implementing them: today the sidecar must declare `indexes == <block>`, have one entry per row group,
and roll up without overflow (`multiblock.rs:271-316`), and its column is taken to be the block's
`row_index`, by convention (recording it is #675); once ADR-0059 lands, `indexed_digest` must equal the
block's digest (ADR-0059 §7). An untrusted index switches tier 1 off for that block. Unlike
`select_blocks_overlapping`, the scan does **not** fall back to decoding the column to learn its range:
tier 2 does that job inside the scan for free.

**Tier 2** is Vortex's own: `with_filter(expr)` prunes zones and then filters rows exactly. The probe, on
one block of 17 row groups: a range predicate on the monotone column selecting one group's worth returned
exactly 65 536 rows and read **328 KB instead of 1.31 MB** for the same projection; a predicate on a random
column returned the exact row count (9 507 of 1 049 353) and, as expected, could prune nothing.

### NULL semantics of tier 1

`ChunkStats` describes **present values only**: `count` is the number of non-NULL rows and `min`/`max`
exclude NULLs (`table.rs:276-289`, after #627). The row-group size is fixed, so the NULL count is exact
without a new field: `nulls = rows_in_group − count`. Under SQL's three-valued logic a comparison with a
NULL operand is NULL, which `WHERE` treats as false, so excluding NULLs from `min`/`max` is exact, not
conservative. The rules, where "may match" keeps the group:

| Predicate on the indexed column `c` | The group may match iff |
|---|---|
| `c = k`, `c < k`, `c <= k`, `c > k`, `c >= k` | `count > 0` and `[min, max]` meets the predicate's range |
| `c BETWEEN a AND b` | `count > 0` and `[min, max]` overlaps `[a, b]` |
| `c <> k` | `count > 0` and not `min = max = k` |
| `c IN (k₁, …, kₙ)` | `count > 0` and some `kᵢ ∈ [min, max]` |
| `c IS NULL` | `rows_in_group − count > 0` |
| `c IS NOT NULL` | `count > 0` |
| `p AND q` / `p OR q` | `may(p) ∧ may(q)` / `may(p) ∨ may(q)` |
| `NOT p`, any other shape | always (no pruning) |

Comparisons are done in `i128`, so a `u64` literal above `i64::MAX` against an `i64`-ranged index is
decided correctly rather than wrapped (`as_i64` already refuses to index a `u64` column holding such a
value, `table.rs:260-265`). An all-NULL group has the identity stats (`count = 0`) and is skipped by every
value predicate and kept by `IS NULL`.

Two in-flight changes touch this and neither changes the rules. **#632** moves the `.cidx` *digest*
pre-image under a NULL; pruning reads only the statistics. **ADR-0059 S2** adds a `masked` count; when a
table index carries it, the reader must cross-check `masked == rows_in_group − count` and treat a
disagreement as an untrusted index.

### The pushable class, and why every filter is `Inexact`

DataFusion asks per filter (`TableProvider::supports_filters_pushdown`,
`datafusion-catalog-54.0.0/src/table.rs:302`) and accepts `Unsupported`, `Inexact` or `Exact`
(`datafusion-expr-54.0.0/src/table_source.rs:37-51`). The translator accepts exactly:

- `column op literal` and `literal op column` with `op ∈ {=, <>, <, <=, >, >=}`, `BETWEEN`, `IN (list)`,
  `IS [NOT] NULL`, and `AND`/`OR` of those;
- on columns of dtype `i1…i8`, `u1…u8`, `b1`, `str`, with a literal of the column's own type after
  DataFusion's coercion. A predicate that still wraps the column in a `CAST` is `Unsupported`.
- The translator **narrows the literal to the column's exact type** (`Literal::Int(300)` against an `i16`
  column becomes an `i16` scalar). This is load-bearing, not cosmetic: Vortex refuses to compare scalars of
  different types (`primitive/scalar.rs:69-71`, the comment at `compare.rs:191`), so a mismatch is an error.
  A literal that does not fit the column's type is `Unsupported`.

Everything in the class is answered **`Inexact`**, so DataFusion keeps its own `FilterExec` above the scan.
Everything else is `Unsupported`, still correct, just not accelerated.

**Why not `Exact`.** Vortex evaluates a filter through per-encoding compare kernels (FoR, bit-packing,
FSST, run-end, sequence, sparse: `encodings/*/src/**/compute/compare.rs`) before falling back to arrow-ord
(`vortex-array/src/scalar_fn/fns/binary/compare.rs:110-182`). `Exact` would turn any divergence from
DataFusion's evaluation into a silently wrong answer. `Inexact` removes rows Vortex wrongly *keeps*, at
the cost of re-evaluating the predicate on the survivors, which tier 1 and tier 2 have already thinned.

**`Inexact` does not protect the other direction.** A row Vortex wrongly *drops* is lost either way. That
is why the class is closed and why PR1's gate is a **differential property test**: random tables (every
dtype in the class, NULLs, boundary values, data shaped so the deterministic compressor picks each integer
and string encoding) × random predicates in the class, asserting pushdown-on equals pushdown-off.

**Why floats are excluded.** Not because the two engines compare floats differently: arrow orders floats
by IEEE 754 totalOrder (NaN above +∞, −0.0 below +0.0, `arrow-ord-58.3.0/src/cmp.rs:72-75`), and so does
Vortex, both in its arrow fallback (`compare.rs:144-182`) and in its scalar path, whose `partial_cmp` uses
`total_compare` for `f16`/`f32`/`f64` (`vortex-array/src/scalar/typed_view/primitive/pvalue.rs:151-153`).
The risk is in **zone pruning**, which drops rows rather than keeping them, so `Inexact` can't repair it.
DataFusion's `x > 5.0` matches NaN rows. Vortex's pruning guards against NaN only when the zone carries a
`NaNCount` statistic: `with_nan_predicate` adds `nan_count = 0` if that stat exists and otherwise prunes on
`min`/`max` alone (`vortex-array/src/scalar_fn/fns/binary/mod.rs:187-204`). The default writer does persist
`NaNCount` (`PRUNING_STATS`, `vortex-array/src/stats/mod.rs:36-42`, used by `ZonedLayoutOptions::default`,
`vortex-layout/src/layouts/zoned/writer.rs:49-53`), but whether every zone carries it, and whether the zone
`min`/`max` order `−0.0` and `+0.0` the way totalOrder does, are unaudited. Float predicates are therefore
`Unsupported`: correct, decoded in full, filtered by DataFusion. The cost is no pruning on float
predicates; a list-mode energy window on an integer-quantised column is unaffected. Adding floats to the
class means amending this ADR with a differential test that covers NaN, ±0.0 and ±∞ in every zone shape.

### Instrumentation

`TableScan::metrics()` returns counters, and `tessera-query` surfaces the same values as DataFusion
execution metrics, so `EXPLAIN ANALYZE` shows them:

| Counter | Meaning |
|---|---|
| `blocks_total`, `blocks_pruned` | tier 1, whole blocks never opened |
| `row_groups_total`, `row_groups_pruned` | tier 1, row groups excluded from the row ranges |
| `bytes_fetched` | counted in our `VortexReadAt` adapter: what tier 2 and projection actually read |
| `bytes_verified` | bytes hashed by §7 |
| `rows_emitted`, `rows_in_flight_high_water` | output, and the §3 bound |

Vortex exposes no "zones pruned" counter (its layout metrics are segment-cache hits, misses and stores,
`vortex-layout/src/segments/cache.rs:97-103`), so tier 2 is measured by its effect, `bytes_fetched`, which
is also what an object store bills. Vortex does count bytes read (`InstrumentedReadAt`'s
`vortex.io.read.total_size`, `vortex-io/src/read_at.rs:216`); our adapter counts per scan unit instead,
so the number can be attributed to one block and one partition.

## §5 — Positional zip

### The operation

`zip(A, B, …)` presents several table blocks as one table whose row *i* is the concatenation of row *i* of
each input, with no hash join. Both inputs are scanned over the **same row ranges**; batches are cut to a
common boundary by zero-copy Arrow slicing (Vortex may split each input differently) and combined into one
`RecordBatch`. Memory is the sum of the inputs' scan budgets; there is no operator state.

### The invariant it relies on

Row *i* of A and row *i* of B describe the same event. Equal row counts are necessary and nowhere near
sufficient: two unrelated tables can have the same length. So the provider requires a positive **proof of
alignment**, in one of two forms:

1. **Declared.** The derived block's `TableSpec` gains an optional, sealed `aligned_to`:

   ```rust
   pub struct RowAlignment {
       /// `None`: a block of this product. `Some(role)`: the product
       /// of this manifest's `sources` edge with that role; the
       /// edge's `content_hash` pins that product's seal.
       source: Option<String>,
       /// The source table's block prefix (`events` = `events_NNNN`).
       block: String,
       /// blake3 over the source group's block digests, in manifest
       /// order: which bytes the rows were derived from.
       source_digest: String,
   }
   // Fields private, no public constructor (see "who may write it").
   // TableSpec gains, skipped when None:
   //   #[serde(default, skip_serializing_if = "Option::is_none")]
   //   pub aligned_to: Option<RowAlignment>,
   ```

   It is skipped when `None`, so every existing manifest serialises byte-identically: corpus-neutral, no
   `TESSERA_VERSION` bump.

   **The trust model.** `aligned_to` is the **writer's claim** that row *i* was derived from source row *i*.
   The seal makes the claim tamper-evident once written, and checks what it can: the row counts, and, for a
   same-product source, `source_digest`. It cannot check that the writer's transform really kept row order.
   So the claim is only as good as the code that writes it, and the engine restricts which code that is.

   **Who may write it.** `RowAlignment` has no public constructor. Exactly these engine paths create it:
   - `tessera_io::scan::derive_aligned` (new in PR2): streams a source logical table through a caller's
     per-batch transform, refuses a batch whose output row count differs from its input's, computes
     `source_digest` from the source it actually read, and seals the result. The transform can change values
     and add columns; it cannot reorder or drop rows, because the path, not the transform, assigns rows to
     output positions.
   - The #657 recipe materialiser, for recipes it classifies as row-preserving, through `derive_aligned`.
   - The Python binding's equivalent, when it exists, as a thin wrapper over `derive_aligned`.

   Generic `ingest`, `commit --add-block`, `Builder`/`table_block` and every other generic write path never
   write it, and refuse a new block whose `TableSpec` carries one. A commit that leaves a derived block
   untouched keeps it with its `aligned_to` (a `commit --set` metadata edit re-seals the manifest but not the
   block); a commit that removes or replaces the *source* block needs no special case, because the zip's
   `source_digest` check then refuses. A hand-crafted, re-sealed manifest can still carry
   the field, since the format can't tell who sealed it.

   **Signing: required across producers, not within one.** A forged claim that names the right source, with
   a correct `source_digest`, passes every check above and yields a **silently misaligned** zip; the digest
   only stops a claim from being applied to a different version of its source. Forging one requires writing
   and sealing the derived product outside the engine, and a party who can do that can equally forge the
   derived values themselves. So:
   - **PR2 (#667) ships same-producer zips, unsigned.** When the caller zips products it produced, the
     derived product's seal covers the claim exactly as it covers the values, and no signature is required.
   - **A zip across producers requires a signed alignment, and does not ship before it.** PR2 cannot tell
     producers apart (the manifest's `producer` is a self-reported tool stamp, not a verified identity), so
     in PR2 the same-producer scope is a documented usage scope, not a check: the docs state that zipping a
     derived product the caller did not produce is not supported. The signed-alignment work, tied to the
     identity-envelope claims of #656 and #566 (which need the same "signed claim, verified against the
     trust store" path), makes it a check: the zip accepts a declared alignment from a product the caller
     did not produce only when that product carries a valid signature from a trusted key (ADR-0037's trust
     store) binding `aligned_to`, and refuses an unsigned one with an `AlignmentError`. Cross-producer zips
     are offered only once that refusal exists.

   At query time the zip recomputes `source_digest` from the source manifest it was given and refuses a
   mismatch, so a derived table is never zipped with a different version of its source.
2. **Verified key (no format change).** The caller names a key column present in every input (an event
   id, or `row_index` when the derived product kept it). Matching keys row by row proves alignment only if
   the key is unique, and list-mode timestamps tie, so key mode requires the key to be **strictly
   increasing across the emitted rows, in table order**, in every input. Key mode never reads a row that
   tier 1 prunes, so the argument below has to account for pruned rows without seeing them.

   **Pruning under key mode.** Tier 1 may prune a row group of a key-mode zip only on the **key
   predicate**, the filter's conjuncts that reference the key column alone, and only when the key is the
   trusted indexed column of **every** input and **every** input's index rules the group out; the same rule
   governs whole-block pruning at planning (§6). Until #675 records the indexed column, "the key is the
   indexed column" means the key is `row_index`, by the convention of §4. Otherwise tier 1 is off for that
   zip. So every pruned row, in every input, fails the key predicate. The zip then evaluates the key
   predicate itself on each zipped row, on the decoded key and after the checks below, with tier 1's `i128`
   comparison and literal narrowing (§4) so the two can never disagree, and releases only rows that pass it
   (DataFusion's `FilterExec` still applies the whole filter above). Without
   that rule a pruned row could share a key with an emitted one: with an event-id key, A's ids
   `[1, 5] [5] [7]`, B the same with the two `5` rows swapped, and `WHERE row_index <> 2`, the middle
   group is pruned, the emitted ids `1, 5, 7` increase and match, and row 1 would be zipped with B's copy of
   row 2.

   **What pruning trusts.** "Every pruned row fails the key predicate" is true only if the `.cidx`
   statistics are. The trust rules of §4 bind the index to its block (and, after ADR-0059, to the block's
   digest), but nothing checks that a sealed `.cidx` reports the data's real `min`/`max`. A writer that seals
   a lying index (a group of `5`s recorded with `max = 4`, then `WHERE key >= 5`) gets the swap above past
   every check. So under pruning, key mode rests on the writer's index, a claim of the same kind as declared
   mode's `aligned_to`. Within one producer the seal covers it, as it covers `aligned_to` in PR2: whoever
   can seal a lying index can seal wrong values directly. This ADR extends the cross-producer rule above to
   the index on its own reasoning (the owner's decision names the alignment, not the index): a zip across
   producers that prunes on another producer's `.cidx` waits for a signed claim covering that index too. A
   caller who will not
   trust the index runs the zip without a key predicate tier 1 can use, which checks every row and trusts
   no writer claim.

   **Why that is enough.** Under a zip, tier 1 prunes the same row groups in every input and no row filter
   is pushed into any input (below), so every input emits the same positions, and a unit emits its whole
   row-group run; call these the checked rows. If at each checked position the inputs' keys are equal, and
   each input's checked keys strictly increase, then each key occurs exactly once among each input's checked
   rows. A released row also passes the key predicate, which every pruned row fails, so its key occurs in
   no pruned row either: it is unique in every input, and the rows paired at its position are the one row
   with that key in each input. Matching alone is not enough: let A have keys `[…, 5] [3] [5, …]` in three
   row groups, and B the same with the two `5` rows swapped. `WHERE key >= 5` prunes the middle group, the
   checked keys `…, 5, 5, …` match position by position, and only the tie between the two checked `5`s shows
   that the pairing is not determined.

   **How it is checked: a key-only pre-pass, then the rows.** A key-mode zip scans in two passes. The
   **pre-pass** decodes only the key column of every checked row group of every input and checks every key
   against the checked key before it: equal across inputs, NULL-free, strictly greater. The chain runs across
   row groups within a unit and, across units, on the **last checked key of the nearest preceding emitted
   unit** in table order, across pruned row groups and block edges. The **row pass** then decodes the
   projection, and only the row pass releases rows, behind the release gate below.

   §6 deals units to partitions as contiguous ranges, so a partition's units are one contiguous run of the
   emitted sequence: each partition's pre-pass checks its own run and carries the last checked key from one
   unit to the next. Across partitions, which run in parallel, the chain is enforced by a query-wide
   **boundary stage**. When the plan is first executed (the first `execute` of any partition), the query
   starts the pre-pass of **all P partitions**, regardless of which have been executed, on at most `P`
   threads, the scan's own bound. Each partition's pre-pass registers the first key of its run and the last,
   and the stage checks that *p − 1*'s last key is less than *p*'s first key for every adjacent pair. The
   pre-pass depends on no partition's consumer, so a consumer that drains one partition before executing the
   next cannot hang it. The first emitted unit has no predecessor and the last emitted unit no successor.

   **The release gate.** No partition releases a row until the pre-pass of every partition and every
   boundary check have passed, so a key-mode zip **refuses a pushed `LIMIT` or an early stop until the
   pre-pass has verified strict increase over the whole scanned range**; after that, a pushed `LIMIT` is
   honoured as in any scan. The gate sits inside the scan, so a `LIMIT` pushed into it (DataFusion's
   `fetch`), a `LIMIT` above it, an operator that stops pulling and a consumer that drops the stream can only
   ever see rows of a range the pre-pass has verified. The scan cannot know in advance whether a query will
   stop early (an operator above it or a dropped stream ends the query without telling it), so the gate
   applies to **every** key-mode zip, and there is no option to skip it. This is correct by construction,
   not a documented caveat, and checking neighbours while streaming is not enough: let A have keys
   `[5, 6] [3, 5]` in two row groups, and B the same with the two `5` rows swapped. A row-by-row check
   releases A's first `5`, paired with B's copy of the other `5`, once its neighbours have passed, before
   the inversion `6, 3` is reached; under `LIMIT 1` that misaligned pair is the whole result, returned with
   no error. Across two partitions, both copies of a key could likewise be released before either partition
   reached the other. The pre-pass meets the inversion before any row is released and fails the query.

   The row pass decodes the key again with the projection and runs the same checks on it; when a partition's
   row pass reaches its first and its last row, each key must equal the one the pre-pass registered. A
   failure there fails the query; it is how §7's local second-pass caveat can show in a key-mode zip.

   Keys are read through the same verify-once cell as the rows (§7): under `VerifyBlocks` from hashed bytes
   (locally with §7's second-pass caveat), and under `SealOnly` from unhashed bytes, like every row in that
   mode. When the zip reads from the spool (an object store under `VerifyBlocks`, or a local source with the
   spool opt-in of §7), every block the pre-pass reads is copied into the spool and hashed first. A
   partition's first and last block of each input stay spooled from its pre-pass until every partition that
   scans them has finished its row pass (§6 can split one block, so a block may be one partition's last and
   the next one's first); between them, two slots per input hold the block being scanned and the next one,
   prefetched across the block boundary as in every scan (§3). So a key-mode zip needs at most `4 · k · P`
   spool slots, and §7's slot rule applies: `plan_scan` computes the plan's need and refuses a `spool_blocks`
   below it before any row, so every partition can hold its maximum at once and a consumer that drains one
   partition first cannot starve another. A block between a partition's first and last is released after the
   pre-pass and copied and hashed again for the row pass, so a key-mode zip read from the spool fetches those
   blocks twice; it never reads a row or a key from bytes it did not hash. A pruned block is never opened for
   key mode.

   A tie, an inversion, a NULL key or a key that differs across inputs **fails the query** with an error
   naming the input and the row, in the pre-pass, before any row is released; the provider never skips the
   row. A key that is not unique is not a key for this purpose: the caller uses declared mode or writes a
   hash join explicitly.

   **What a guarantee covers.** Every row a key-mode zip releases lies in a range whose keys the pre-pass has
   verified, so every row it returns is paired by a unique key, whether the query runs to completion, stops
   early under a `LIMIT`, is stopped by an operator above the scan or has its stream dropped. On a local
   source read unspooled (the default, §7) this holds against corruption at rest only: a concurrent writer
   that swaps rows between the pre-pass and the row pass is not excluded, since the row pass checks keys row
   by row; a caller facing that threat opts into the spool. A query that fails in the pre-pass returns no row.
   A query that fails in the row pass (an integrity or I/O error, or a key the row pass reads differently) has
   no result: DataFusion's stream yields the error in place of the remaining batches, and `collect` returns
   only the error; batches streamed before the error were paired by verified keys. **Cost:** a read of the key
   column over the scanned range before the first row (time to first row is one key pass), and, read from the
   spool, the second fetch above. A consumer that needs the first rows sooner uses declared mode.

### Detection and refusal

Before the first row, in both modes, from manifests alone:

- every input's total row count is equal, and so are its **per-block** row counts. A conforming writer
  derives the block partition from the row count (`block_count`, `table.rs:831`), so equal totals imply
  equal partitions; a difference means a non-conforming writer and is refused rather than re-sliced;
- in declared mode: exactly one `sources` edge has the named role (`role` is free text, so zero or several
  matches are refused), that edge carries a `content_hash` (it is optional, `tessera-core/src/provenance.rs:
  20-23`; a missing one is refused), the source product's `manifest_hash` equals it, the named block
  prefix exists, the row counts agree, and `source_digest` equals the digest recomputed from the source
  manifest's block digests;
- column names do not collide across inputs (a collision is refused; the caller renames in SQL). A
  key-mode zip's key column is exempt: it is present in every input by definition, is checked equal across
  them, and is emitted once.

A failure is a typed `AlignmentError` naming the input, the check and both values, raised from
`plan_scan`, so **no row is emitted**. In key mode a tie, an inversion, a NULL or a mismatch is found by the
pre-pass and fails the query naming the input and the row, also before any row is released (the release
gate above).

Zipping across a Collection (each member zipped with its own aligned partner) is not in PR1–PR3; it is tracked
as #671.

### Filters under a zip

A filter must not drop rows from one input only. So for a zipped scan, tier 1 prunes row groups for **all**
inputs together (a group is skipped when the predicate, evaluated on the stats of whichever input owns the
column, cannot match), and **no row filter is pushed into any input's Vortex scan**: the predicates are
`Inexact` and DataFusion applies them after the zip. Projection is still pushed per input. A key-mode zip
narrows tier 1 further and evaluates its key predicate on the zipped rows itself (§5, "Pruning under key
mode"); that is a filter after the zip, not in any input's scan.

### Rejected

- **Row count alone.** Admits silent joins of unrelated tables, the failure the issue forbids.
- **Hash join on a key as a fallback.** Different performance (unbounded build side) and different
  semantics (duplicate keys multiply rows) behind the same name. A consumer who wants a join writes one.
- **Re-slicing inputs with different block partitions.** Possible, but a partition mismatch already
  proves the inputs were not written by a conforming writer from the same row count; refusing is the
  safer reading.
- **Encoding alignment in a provenance edge `role` string** (e.g. `"aligned:events"`). A string protocol
  inside a free-text field; the typed `RowAlignment` is the same information with a schema.
- **Sharing a predicate mask across inputs** (evaluate on the input that owns the column, apply the row
  selection to the others). Correct, but it couples the inputs' scans and decodes the predicate column
  twice; not needed for correctness.

## §6 — Multi-product scans

### Sources and membership

A multi-product table is built from an explicit list of products or from a `Collection`. Each member
resolves to a `ProductSource` (a local path or an object-store location) through a resolver; the default
follows ADR-0049 §4 (the member's `kind` decides `<ref>.tsra` vs `<ref>.collection.json`, siblings of the
collection file). The opened product's `manifest_hash` must equal the member's pinned `manifest_hash`;
otherwise the scan is refused before any row (`MemberMismatch`). Sub-collections are flattened depth-first
in member order, with ADR-0049 §3's `seen`-set cycle guard. A member that cannot be resolved is an error,
never a silently smaller cohort.

### Partitioning

The work unit is `(product, block, contiguous row-group run)`, at most one block. Planning reads manifests
and `.cidx` sidecars only and drops units tier 1 rules out. Units are dealt to the §3 cap,
`P = min(target_partitions, max_partitions, units)` DataFusion partitions, as contiguous ranges (locality;
`target_partitions`, `datafusion-execution-54.0.0/src/config.rs:216`), declared as `UnknownPartitioning`. Rows keep (product,
block) order within a partition; no global output ordering is claimed, so `ORDER BY` stays explicit.
The provider's `statistics()` (table level, no filters) reports `num_rows` as **Exact**: the sum of sealed
`TableSpec.rows`. The scan node reports it **Inexact** as soon as a filter or a `limit` is pushed, since
tier 1, tier 2 and the limit drop rows. Column statistics are **Inexact** from trusted indexes only,
otherwise absent, never Exact: an Exact min/max lets DataFusion answer `MIN(c)` without scanning, which
must not rest on a heuristic.

A single block also splits into several units, so one large product parallelises across partitions too.

### Schema across products

| Option | Verdict |
|---|---|
| **Refuse** unless every product has the identical schema (names, order, dtype, nullability) | **recommended default** |
| **Explicit common columns:** the caller names them; each identical in every product | **recommended opt-in** |
| Union with NULL-fill for absent columns (DataFusion `ListingTable`'s schema-adapter style) | rejected |
| Implicit widening (`i16` ∪ `i32` → `i32`) | rejected |

NULL-fill fabricates NULLs indistinguishable from recorded NULLs: "this product has no such column" and
"this event has no value" become the same answer. Widening is an opinion the sealed products did not
express, and `u64` ∪ `i64` has no lossless target at all. Both silently change what a cohort query means;
the refusal names the product and the first differing column, and the caller can choose the common columns
explicitly. An opt-in **product column** (a dictionary-encoded `manifest_hash` per row, name chosen by the
caller and refused if it collides with a real column) makes `GROUP BY product` possible without trusting
file names.

### Local first, then the object store

Local sources read blocks with positional reads on the file through a `VortexReadAt` over the member's byte
range (Context, point 4). Object-store sources open through `ObjectStoreReader`, whose tail prefetch makes
"open + manifest" one HEAD and one GET (`tessera-io/src/cloud.rs:47`, `TAIL_PREFETCH`), and read blocks
through a second `VortexReadAt` that issues `get_range` calls at the member's offset. Under `VerifyBlocks`
(§7; the object-store default, pending #672) each touched block is downloaded whole into the spool and scanned
locally, so this ranged reader serves the `SealOnly` path and the spool download.

**Two object_store versions, and the consumer's registry.** `tessera-io` uses object_store **0.14.0**;
DataFusion 54 uses **0.13.2** (both locked today, §1 D). So a consumer's DataFusion `ObjectStoreRegistry`
(`datafusion-execution-54.0.0/src/object_store.rs:151`), which holds 0.13 `ObjectStore`s, cannot be handed to a
reader written against 0.14: the traits are different types. The options:

| Option | Verdict |
|---|---|
| **A tessera-owned `RangeSource`**: `tessera-io` implements it for 0.14, `tessera-query` for 0.13 | **recommended** |
| Unify the versions (`tessera-io` to 0.13, or wait for DataFusion's 0.14) | not the plan; taken when it happens |
| Document the limit: object-store tables opened only by URL (`open_url`), never from the registry | rejected |

The seam is two methods, so each impl is a few lines, and it adds no dependency: 0.13 is already in every
`sql` build through DataFusion. A consumer configures credentials once, in their own registry, and a scan
of `s3://…` uses that store. When the versions unify, the 0.13 impl is deleted and nothing else moves.
Pinning `tessera-io` to 0.13 instead would tie the cloud reader to DataFusion's release cadence, which §2
keeps out of `tessera-io`; the documented limit would make every cohort-on-S3 consumer configure two sets of
credentials.

**Runtime.** The existing `ObjectStoreReader` owns a current-thread tokio runtime that makes progress only
inside its own `block_on` (`cloud.rs:18-21`, `:146`), so a request spawned onto it from the Vortex runtime
would never complete. The query backend instead runs **one I/O thread per object store** (per bucket and
credentials, shared by every product read from it, so a cohort of hundreds of products still has one or a
few) that sits in `block_on` on its current-thread tokio runtime for the life of the query. The
`VortexReadAt` spawns each `get_range` onto that runtime's handle and awaits the join handle, which is
runtime-agnostic. The Vortex runtime never polls a tokio-bound future, no multi-thread tokio runtime is
introduced, and tokio stays inside the object-store backend, where ADR-0034 §4 put it; the one change from
today is that its runtime is driven by a dedicated thread instead of by each caller's `block_on`. Pruned
blocks issue **zero** GETs, which PR3 proves with the request counter the `s3-range-read` check already
uses, a key-mode zip (§5) included: its pre-pass reads only blocks the scan touches.

## §7 — Integrity

### The question

A query must not return a row from bytes that do not match the seal. The format can verify at exactly one
granularity: the **block digest** (`BlockRef.digest`, checked by `read_block` and `stream_block`,
`container.rs:456-489`, `:527-565`). There is no per-segment digest, and the `.cidx` row-group digest covers
every column's decoded little-endian bytes of the group (`table.rs:1521-1525`), so it cannot verify a
projection, and it verifies decoded values, not stored bytes.

### Options

| Option | Verdict |
|---|---|
| **`VerifyBlocks`:** hash every block the scan touches before emitting any of its rows | **default** (object store: #672) |
| **`SealOnly`:** the manifest seal at open only (`container.rs:399-404`); payloads not hashed | explicit opt-out |
| Verify after emitting a block's rows | rejected |
| Verify per segment / per row group | rejected (no such digest in the format) |

### `VerifyBlocks`, and its honest cost

- **Local, the default: unspooled.** One streaming hash pass over the member's byte range with a 64 KiB
  ring (the `stream_block` pattern), then the scan reads the rows **in a second pass**, normally served by
  the page cache. So locally, "no row from unverified bytes" holds against **corruption at rest only**: a
  concurrent writer that changes the file between the two passes is not detected. This is the default for
  local sources because a `.tsra` is immutable once sealed and the threat the default defends against is
  corruption at rest; it costs one hash pass and no extra disk.
- **Local, opt-in: spooled.** A caller reading a local path it does not trust to stay unchanged for the
  query, such as a remote filesystem mounted locally or a shared, writable store, opts into the spool for
  that source (a `ScanOptions` setting whose name is PR1's to finalise): each touched block is copied into
  the spool while being hashed and its rows are read from the spool, exactly as on an object store below,
  which closes the concurrent-writer gap at the cost of the spool's disk and a copy of each touched
  block. It is never switched on implicitly.
- **Object store:** each touched block is downloaded once into a spool file while being hashed, verified,
  then scanned from the spool, so rows are read from exactly the bytes that were hashed. The download
  streams through the 64 KiB hash ring in GET bodies of at most `spool_chunk_bytes` (default 8 MiB), and the
  spool is bounded on disk by `spool_blocks × block size` (`spool_blocks` is a query-level option whose
  default and floor are the plan's need, "Spool slots" below), never in RAM.
- **Spool slots: every partition can hold its maximum at once.** A spooled scan (an object store under
  `VerifyBlocks`, or a local source with the opt-in) opens a block only from the spool, so every block a
  partition holds takes a slot. Per input, a partition holds the block it is scanning and the next one it
  prefetches across the block boundary (§3), and a key-mode zip also pins its first and last block (§5). So a
  plan needs at most `2 · k · P` slots, `4 · k · P` for a key-mode zip. `plan_scan` computes the plan's actual
  need from its units (fewer when a partition's run has fewer blocks; a boundary block that both partitions
  pin in key mode counted once; outside key mode a block two partitions share counts in each partition's need,
  since one partition may still hold it while the other holds two other blocks), makes that the default, and
  **refuses** a configured `spool_blocks` below it with a typed error naming both numbers (the name is PR3's
  to finalise), before any row. Since every partition can hold its maximum at the same time, no partition ever
  waits for a slot another partition holds, so a consumer that drains the partitions in any order cannot hang
  the query. The rule replaces a default of `P`, which could hang: a declared-mode zip with `k = 2` and `P =
  1` must hold a block of each input to emit a zipped row, and with one slot it spools A's block and waits
  forever for B's; even with `k = 1`, partitions that prefetch hold two slots each, so partitions blocked on
  full channels could take every slot while the partition the consumer drains waits for one. **Why count
  prefetch rather than disable it:** the measured 2.6–8× re-read (§3) happens inside one block, between window
  scans, and is fixed by the per-block caches whichever way this is decided. Cross-block prefetch is what
  overlaps the next block's whole-block download and hash with decoding the current one, the largest single
  wait on a spooled scan; without it every partition would stall for one block fetch at every block boundary.
  Counting its slot costs one block of spool **disk** per input per partition (`disk(spool)`, §3) and no RAM;
  a caller short on disk lowers `max_partitions`, which lowers the need.
- **The `.cidx` is always verified.** Tier 1 reads the sidecar with `read_block`, which checks its digest
  (`container.rs:456-489`), in **every** mode, `SealOnly` included, so pruning never acts on unverified
  statistics. `read_block` checks only when the manifest records a digest for the member (`if let
  Some(expected)`, `container.rs:467-473`). A sidecar without one is not trusted unverified: tier-1 pruning
  is switched off for that block, which is then scanned as if it had no `.cidx`.
- **Once per block, not once per unit.** §6 can split one block into several units on different
  partitions. Verification is a query-wide, verify-once cell per (product, block): the first unit to touch
  the block hashes it (and fills the spool), the others wait for that result. A block is hashed once per
  query however many partitions read it, with one exception: read from the spool, a key-mode zip fetches
  and hashes a block between a partition's first and last again for the row pass (§5), since only the first
  and last stay pinned between the passes.
- **Cost:** a touched block's I/O is the whole block, whatever the projection and whatever tier 2 would have
  skipped; CPU is blake3 at the measured 2930 MB/s single-thread (ADR-0059 §9), about 0.37 s per GiB per
  thread, parallel across partitions. Tier 1 is untouched: a pruned block is neither fetched nor hashed,
  and no row from it is served, a key-mode zip's pre-pass (§5) included. So the price falls
  on full scans of narrow projections and on object storage; selective queries on the indexed column pay
  little.
- **"Verify offline forever" holds:** verification needs nothing but the file.

### `SealOnly`

Reads only what projection and tier 2 need (the probe's 328 KB instead of 8.79 MB). Payload bytes are then
**not** tamper-evident: a payload changed without changing the manifest goes undetected. It must be asked
for by name (`Integrity::SealOnly`), it is shown in the plan's `EXPLAIN` output and in the metrics, and
`tessera sql` exposes it only behind an explicit flag. Whether object-store sources should default to it
is #672.

### Rejected

- **Verify after emitting.** Rows from an unverified block have already reached the consumer when the
  mismatch is found: the same hole `stream_block` documents for its own writer (`container.rs:521-526`).
- **Per-segment verification.** It would need a new sealed digest per Vortex segment, which is a format
  event of its own and not something this provider should invent. Tracked as #670.

## §8 — Phasing

**Why this order, and not zip first.** Positional zip is the first consumer's main need, so starting with
it was considered. It is rejected because a zip *is* two single-product scans run over the same windows:
it needs PR1's windows, budget, verification and metrics, and building it first would mean building them
inside it. PR2 is small once PR1 exists. The multi-product PR comes last because the object-store path
needs the I/O-thread design of §6, which nothing earlier depends on.

Three PRs, in order, filed as sub-issues of #654 with these acceptance criteria: **#666** (PR1), **#667**
(PR2), **#668** (PR3). Each carries the issue's three acceptance criteria (bounded RSS, instrumented chunk
skipping, equality with the in-memory path) for its own scope. Deferred work, each filed: float pushdown
(#669), per-segment digests (#670), zip over a Collection (#671).

### PR1 — single-product streaming + pruning (#666)

`tessera_io::scan` (plan, units, `TableScan`, `ScanBudget`, `Integrity`, metrics, local `VortexReadAt`),
the `tessera-query` crate with a `TableProvider` over one product's logical table, and `tessera sql`
migrated onto it. The current materialising path survives only as the test oracle.

- **Equality:** for a query corpus (projections, every operator in the class, NULLs, aggregates, `LIMIT`)
  on single- and multi-block (`events_NNNN`) fixtures, results equal the `MemTable` path's.
- **Pushdown safety:** the differential property test of §4 (pushdown on vs off) passes.
- **Instrumented pruning:** on a table whose indexed column selects *k* of *N* row groups, `row_groups_pruned
  = N − k`; blocks that cannot match have `bytes_fetched = 0`; a tier-2 predicate reads fewer bytes than the
  same projection without it.
- **Window bytes:** window scans read the same bytes as one scan of the block, on a table with a `utf8`
  and a dictionary-encoded column.
- **Bounded memory:** the CI high-water test of §3, and the ksb run of §3 (anonymous-memory peak, pinned
  `target_partitions` and `--exclusive-cpus`, scale invariance, and the negative control: the materialising
  oracle path in the same cgroup must be OOM-killed), with its numbers recorded.
- **Integrity:** a flipped payload byte in a scanned block fails the query with `Error::Integrity` and no
  row of that block is emitted; the same flip in a tier-1-pruned block does not fail it, and the docs say so.
- **Gates:** Gate B snapshots regenerated with the inert-delta proof (§1).

### PR2 — positional zip (#667)

`zip` in `tessera_io::scan` and in `tessera-query`; the `aligned_to` field, `derive_aligned` (its only
writer) and the seal-time checks; key mode with the key-only pre-pass, the release gate and the
strictly-increasing check. Same-producer zips only: no signature check, and the docs say that a zip across
producers is not offered until a signed alignment is (§5).

- **Equality:** a zip equals the hash join on the key column for aligned inputs, with and without filters.
- **Refusal before any row:** different total rows, different per-block rows, a missing or ambiguous
  `sources` role, a missing `content_hash`, a wrong source `manifest_hash`, a `source_digest` that differs
  from the source's, a missing block prefix and a column collision (other than a key-mode key) each produce
  `AlignmentError`.
- **Key mode:** a tie, an inversion, a NULL key, a key altered in row group *g*, a tie on a unit boundary
  inside one partition and a tie on a partition boundary each fail the query with an error naming the row.
  §5's counterexample (A with keys `[…, 5] [3] [5, …]`, B the same with the two `5` rows swapped, queried with
  `WHERE key >= 5` so the middle group is pruned) is refused with the gap inside one block and with the gap a
  whole pruned block, with both `5`s in one partition and in two; asserted on the released batches, no swapped
  pair is ever zipped. An inversion across a pruned gap and across a partition edge is refused the same way.
  Key mode reads no byte of a pruned block (its `bytes_fetched` is 0). The key column is exempt from the
  collision refusal and emitted once. `WHERE key >= 5 AND e > 3` prunes on the key conjunct only. An event-id
  key under `WHERE row_index <> 2` (the key is not the indexed column) switches tier 1 off for the zip, so
  §5's `[1, 5] [5] [7]` swap is checked and refused. A scanned boundary key that differs from the registered
  one fails the query. A consumer that executes the partitions one after another completes without hanging.
  The docs say that pruning in key mode trusts the `.cidx` statistics. (The spool lands in PR3, so the
  key-mode spool tests are PR3's.)
- **Release gate:** a key-mode zip with a pushed `LIMIT` or an early stop is refused until the key-only
  pre-pass has verified the scanned range: §5's `[5, 6] [3, 5]` case (B the same with the two `5` rows
  swapped) under `LIMIT 1` fails the query and returns no row, a test that fails on a neighbour-check-only
  implementation, which returns the misaligned pair. The same case with the two row groups in two
  partitions, and with no `LIMIT` but a consumer that drops the stream after its first batch, returns no
  row either. On aligned inputs, the pre-pass has completed before the first batch is released, and
  `LIMIT n` then returns exactly `n` correctly paired rows.
- **Writers:** generic `ingest` and `commit --add-block` refuse a spec carrying `aligned_to`;
  `derive_aligned` refuses a transform that changes a batch's row count.
- **Bounded memory:** the ksb run repeated on a zip of two blocks; the anonymous peak stays within the §3
  formula with `k = 2`, key-mode buffers included.

### PR3 — multi-product and collections, local then object store (#668)

Member resolution, partitioning, schema policy, the product column, then the object-store `VortexReadAt`,
the spool (with the local spool opt-in of §7) and the `RangeSource` seam for a consumer's DataFusion
`ObjectStoreRegistry`.

- **Equality:** a collection scan equals the `UNION ALL` of its products' single-product scans.
- **Refusal:** a member `manifest_hash` mismatch, an unresolvable member and a schema mismatch are refused
  before any row.
- **Object store:** against versitygw (extending the `s3-range-read` flake check), results equal the local
  scan; pruned blocks issue zero GETs, in a key-mode zip too; the spool stays within its bound; a store
  taken from a DataFusion `ObjectStoreRegistry` reads the same rows as one opened by `open_url`.
- **Local spool opt-in:** with the opt-in, a byte changed in the local file after its block was spooled and
  hashed reaches no row; without it, a local scan creates no spool file.
- **Spool slots (§7), on an object store and on a local source with the opt-in:** at the default
  `spool_blocks`, a declared-mode zip with `k = 2` and `P = 1`, a prefetching single-product scan and a
  key-mode zip each complete when the consumer drains one partition before executing the others; a
  configured `spool_blocks` one below the plan's need is refused by `plan_scan` with the typed error,
  before any row; a block shared by two partitions is spooled once (but counted per partition outside key
  mode); the spool's high-water mark never exceeds the computed need (`2 · k · P`, `4 · k · P` in key mode,
  at most), including a single-product scan with `k = 1`, `P = 2` and runs `{1, 2, 3}` / `{3, 4, 5}`
  whose first partition is blocked on a full channel holding blocks 1 and 2 while the second holds 4 and 5
  (need 4, not 3).
- **Bounded memory:** the ksb run over a many-product collection.

## Alternatives considered and rejected (summary)

| Alternative | Why not | § |
|---|---|---|
| DataFusion as a feature on `tessera-io` | its semver in the seal-path crate's API; mixed responsibility | §1 |
| Everything in `tessera-query` | second owner of Vortex sessions; direct `vortex-*` deps; no engine-neutral read | §1 |
| `vortex-datafusion` 0.75 | unpatched umbrella crate, multi-thread tokio; no seal, zip or `.cidx` | §1 |
| A private query API hiding DataFusion | useless to consumers who join with their own tables and UDFs | §2 |
| Byte-bounded memory | `utf8` has no width; decoded size is unrelated to compressed size (measured) | §3 |
| Vortex's default `SplitBy::Layout` | split rows ≈ 1 MiB ÷ column width (524 288 rows measured) | §3 |
| Window scans without a segment cache | re-read shared segments: 2.6–8× the bytes (measured) | §3 |
| Bounding by `with_concurrency` | Vortex multiplies it by the host's core count | §3 |
| `Exact` filter pushdown | turns any Vortex/DataFusion divergence into a silent wrong answer | §4 |
| Float predicate pushdown | zone pruning's NaN and ±0.0 handling unaudited; drops can't be undone | §4 |
| Zip on row count alone / hash-join fallback | silent joins / different semantics under one name | §5 |
| NULL-fill union, implicit widening | fabricated NULLs; unexpressed type opinions | §6 |
| Verify after emitting; per-segment verify | rows escape before the check; no such digest in the format | §7 |

## Consequences

- A 7th workspace member, `tessera-query`, carrying the project's only DataFusion dependency. The CLI's
  `sql` feature depends on it; nothing else does.
- `tessera-io` gains a public, engine-neutral, synchronous streaming scan returning Arrow batches, and
  `arrow-array`/`arrow-schema` become direct dependencies (already pinned, already compiled).
- `tessera sql` stops materialising tables and gains `EXPLAIN ANALYZE` metrics; its output is unchanged.
- A bounded query becomes possible on tables far larger than RAM; the bound is in rows and is a function
  of the budget, not of the table.
- Every local query verifies the blocks it reads by default (object-store default pending #672), which costs a
  full read of each touched block.
- `TableSpec` gains one optional sealed field, `aligned_to`, corpus-neutral, written only by
  `derive_aligned`. Zips ship for products of one producer; a zip across producers waits for a signed
  alignment (#656, #566).
- A key-mode zip reads its key column once more than the query needs and releases its first row only after
  that pre-pass; in exchange every row it returns is correctly paired, however the query ends.
- DataFusion majors become a recurring migration and a breaking `tessera-query` release.

## Open questions (do not block acceptance)

Each is filed with a recommendation, and none changes a decision this ADR states as settled. Each gates the
work named beside it, not the acceptance.

| Issue | Question | Gates |
|---|---|---|
| #672 | Integrity default for object-store sources: `VerifyBlocks` (spooled) or `SealOnly` (§7) | PR3, #668 |
| #673 | `tessera-query` keeps its own version line at 1.0, an exception to ADR-0052 §2 (§2) | 1.0 / API freeze |
| #674 | A declared workspace MSRV now that DataFusion is public API (§2) | 1.0 / API freeze |
| #675 | The indexed column recorded in the table `.cidx` spec at the ADR-0059 event (§4, §5) | `.cidx` spec change |

#672 decides only which mode an object-store source gets when the caller names none. Under either answer
the spool, its slot rule and the key-mode pre-pass stay as stated, defined for `VerifyBlocks`, and
`SealOnly` stays a labelled mode with its own costs (§7); if #672 picks `SealOnly`, the object-store
statements of this ADR that call `VerifyBlocks` the object-store default change to name it an opt-in,
and no other rule moves. Until #675 lands,
key-mode pruning uses the `row_index` convention as §5 states.

## References

Issue #654, the architect's triage comment of 2026-10-25 and the owner's decisions of 2026-10-26
([comment](https://github.com/vig-os/tessera/issues/654#issuecomment-6423805993)) · #251 · #225 · #627 ·
#632 · #657 ·
ADR-0026 · ADR-0034 · ADR-0049 · ADR-0052 · ADR-0057 · ADR-0059 · `tessera-cli/src/sql.rs` ·
`tessera-io/src/{table,multiblock,container,cloud}.rs` · `tessera-core/src/chunk_index.rs`.
