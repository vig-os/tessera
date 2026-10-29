# Spike — a deadlock-free bound on the Vortex writer's pending segments (#681)

> **Status:** design, for the implementation round of #681 (PR 3 of #677). No product code changes here.
> Every Vortex claim cites `file:line` at the pinned fork `gerchowl/vortex@608d8a1` (tessera's
> `[patch.crates-io]`, #480) unless it names upstream `develop`. The reproduction and the prototype ran
> in a scratch clone at that SHA (`/mnt/HDD/larsgerchow/vortex-681`, branch `spike-681`, never pushed);
> the prototype diff and the harness are kept there as `job/prototype-681.patch`.

## TL;DR

- **Two causes, not one.** The encoded segments that pile up in RAM while one table is written have
  two separate causes:
  1. **Sink starvation (grows with the input).** Nothing slows the producers down, and on tessera's
     single-threaded runtime the loop that drains the segment sink runs only between batches of 200
     other tasks. The sink falls behind and the backlog grows with the table. Measured on a single
     random `u64` column: the live heap reaches 0.94 × the encoded size, and only 39 MB of 537 MB had
     reached the sink at the peak.
  2. **Order debt (bounded but large).** Segments go to the file in one global `SequenceId` order. A
     column that coalesces slowly (a `bool` column needs 8 M rows for one 1 MiB block) holds the
     lowest live id while it waits for its own input, and every other column's later segments have to
     wait behind it. This is a fixed cost of the byte order: about 100 MB for the consumer's 12-column
     listmode shape, whatever the table's size.
- **Why `buffered(n)` deadlocks** (reproduced, deterministically): the lowest live id is held at a
  *peek* inside one column's pipeline, which needs more input. The input comes through the table
  fan-out, which is stuck feeding a *different* column whose capped chunk tasks are parked waiting for
  that lowest id. That is a cycle.
- **The #681 candidate as stated also deadlocks.** "Always admit the chunk holding the lowest pending
  id" never fires. The lowest live id is almost never held by a chunk at the fan-out; it is held at an
  upstream peek. Measured: it deadlocks at a budget of 1 byte.
- **Decision: a budget on pending segment bytes, applied at the writer's input, with a
  starvation override.** The writer pulls the next input chunk only while fewer than `B` encoded bytes
  are pending in the sink, **or** while the sink is *starved*, i.e. it has nothing it could write
  because the lowest live `SequenceId` is not parked in it. This is deadlock-free for every `B`
  (argument below; measured down to `B = 1`). It never changes a byte, because it changes only *when*
  the input is pulled, never *what* is pulled or which ids it gets. Measured: byte-identical across
  budgets, shapes and executor threads. The peak no longer grows with the table: the live heap stays
  about `B + D + ε + K`, where `D` is the order debt, `ε` a few MB of overshoot and `K` the pipeline's
  own working set.
- **Consumer shape, 268 M rows (2.05 GB encoded): stock 834 MB peak heap → 134 MB with the gate**,
  flat from the first 512 row-groups to the last. Same bytes, same wall time.

## 1. Root cause, at `608d8a1`

### 1.1 The data flow

The write path tessera takes (`tessera-io/src/table.rs::encode_streaming_to` →
`VortexWriteOptions::blocking(&rt).write`, on a `CurrentThreadRuntime`, i.e. a single-threaded smol
executor driven by `block_on`, `vortex-io/src/runtime/current.rs:98-102`):

1. **Sequence ids.** `write_internal` splits a root universe into a data pointer and an `eof` pointer
   (`vortex-file/src/writer.rs:157`) and tags input chunk `r` with `[0, r]`
   (`vortex-file/src/writer.rs:163`). Ids are ordered lexicographically; every later id is created by
   `advance` (next sibling, `vortex-layout/src/sequence.rs:183`) or `descend` (first child,
   `sequence.rs:101`), always from a live id, so it can never be lower than that live id.
2. **The sink orders by id.** `BufferedSegmentSink::write` awaits `sequence_id.collapse()`
   (`vortex-file/src/segments/writer.rs:51`), which completes only once every lower id in the universe
   has been *dropped* (`sequence.rs:132`, `:215-222`, `:238`). Then it sends the segment's buffers,
   one at a time, into a `kanal::bounded_async(1)` channel (`segments/writer.rs:93,96`;
   `writer.rs:178`), which the writer's own future drains into the output
   (`writer.rs:204-209`). So the file holds the segments in one global id order across all columns.
   That order *is* the byte layout: it is what tessera's `content_hash` covers.
3. **The table fan-out.** `TableStrategy` descends `[0, r]` into one id per column, `[0, r, c]`
   (`vortex-layout/src/layouts/table.rs:238,255`). A detached task sends each row-group's columns, in
   column order, into one `kanal::bounded_async(1)` channel per column
   (`table.rs:267`, `:279`). While it waits on column `c`'s channel, it still holds the ids of the
   columns after `c`.
4. **The per-column pipeline** (built in `vortex-file/src/strategy.rs:229-311`, the configuration
   tessera uses with its deterministic compressor): `Repartition(8192 rows)` → `Zoned` (per-chunk
   stats, `buffered(P)`, `zoned/writer.rs:123`) → `Dict` or fallback → `Repartition(≥ 1 MiB,
   canonical)` (`strategy.rs:259`) → `Compressing` (`buffered(P)`, `compressed.rs:118`) →
   `Buffered(2 MiB)` (`strategy.rs:232`) → `Chunked` → `Flat` (`flat/writer.rs:174` calls the sink).
   `P` is `available_parallelism` (`compressed.rs:71`, `zoned/writer.rs:55`), a queue depth even on a
   single thread.
5. **The unbounded fan-out.** `ChunkedLayoutStrategy::write_stream` spawns one task per chunk
   (`chunked/writer.rs:69`) and polls them with `buffered(usize::MAX)` (`chunked/writer.rs:89`). It
   always pulls the next chunk, so nothing upstream ever waits for the sink.

### 1.2 Where ids are held while waiting for input (the order debt)

Several stages keep an id alive across an `await` on their *own input*:

- `RepartitionStrategy` peeks at the next input while its `sequence_pointer` (descended from the
  current input's id) is alive (`repartition.rs:141,163`).
- `BufferedStrategy` peeks at the next input while it holds the current chunk's `sequence_id`
  (`buffered.rs:64,71`). Its input is the *coalesced* 1 MiB block, so the next one is a whole block
  away: 16 row-groups of 65,536 rows for a `u8` column, 128 for a `bool` column.
- `dict_encode_stream` peeks at the next chunk while it holds the current `sequence_id`
  (`dict/writer.rs:275,280`).

The peeks are needed: each stage must know whether the current chunk is the last, to flush its
remainder under that chunk's id. They cannot be removed without changing which id a segment gets,
and so the bytes.

So at any moment the lowest live id is typically held by the slowest-coalescing column's
`BufferedStrategy` peek. Every segment the other columns produce in the meantime has a higher id and
must wait in the sink. This order debt is what an in-memory writer must hold to write those bytes in
that order:

```text
D ≳ max_c span_rows(c) × Σ_c' encoded_bytes_per_row(c')
span_rows(c) ≈ 1 MiB / canonical_bytes_per_row(c)  (strategy.rs:259)
```

This is a lower estimate: it leaves out the segments that sit in the stages' own queues
(`Buffered`, `buffered(P)`) while the span is open.

For the consumer's shape (12 columns: `pkt` u64 sequential, nullable `ms` u32, 2 × `bool`, 7 masked
`u8`, `tof` i16), the `bool` columns give `span ≈ 8.4 M rows`, and the formula gives ≈ 64 MB
(7.6 encoded bytes per row, 2,048.7 MB / 268 M rows). The measured `D`, the peak pending bytes at
`B = 1` (§4), is 102 MB. It does not depend on the table's length. It does depend on the schema and
on how well the data compresses. A column whose canonical form has **zero** bytes per row
(`DType::Null`) never fills a 1 MiB block, so its span is the whole table: no in-memory scheme can bound
that case (§2, alternative E). Tessera never writes such a column; its dtypes are numeric, `bool` and
`utf8`.

### 1.3 Why the sink falls behind (the growth)

The order debt is bounded, yet the stock pile grows with the table: 225 MB at 67 M rows and 834 MB at
268 M rows for the consumer's shape (§4.2). The second cause is scheduling. `CurrentThreadRuntime::block_on` runs
`executor.run(write_future)` (`current.rs:102`). The executor (async-executor 1.14.0,
`src/lib.rs:740-756`) runs ready tasks in batches of 200 and polls the caller's future (here the write
loop, `writer.rs:204-209`, the sink's only consumer) only between batches, or when no task is ready.
With `buffered(usize::MAX)` some task is always ready, and the sink's `bounded_async(1)` channel lets
one buffer through per poll of the write loop. So the producers out-run the sink at any table size.
Measured on a single random `u64` column, which has no cross-column order debt at all: the live heap
reaches 0.94 × the encoded size, with 39 MB of 537 MB written at the peak (§4.2). A single sequential `u64`
column compresses to a few bytes per 1 MiB block. `BufferedStrategy` holds those blocks until it has
4 MiB (`buffered.rs:80`), so nothing is written before the end. That is bounded (≤ 4 MiB) and harmless:
10 MB live for 67 M rows.

### 1.4 The `buffered(n)` deadlock: the wait-for cycle

With `ChunkedLayoutStrategy`'s fan-out capped at `n`, a column whose `n` chunk tasks are all parked in
`collapse` stops pulling its input, its channel fills, and the table fan-out blocks on it. Reproduced
at `n = 2` and `n = 8` (§4.1). The deadlocked state at `n = 2` (consumer shape, ids printed by the
harness in the local reproduction; the ksb log cuts each line at 700 characters, `job/sweep.sh:8`):

```text
lowest live id  [0, 63, 3, 7, 0]
    column 3 (u8 `ax`), held by its BufferedStrategy peek (buffered.rs:71);
    its next 1 MiB block needs row-group 79
fan-out         [0, 74, 10], [0, 74, 11]
    held by the fan-out task, stuck sending row-group 74 to column 10
    (table.rs:279): columns 10 and 11 of row-group 74 are unsent
parked          [0, 63, 10, 7, 0, 0], [0, 63, 10, 7, 0, 1]
    column 10's two capped chunk tasks, in collapse, waiting for every
    lower id, including [0, 63, 3, 7, 0]
```

The cycle: column 3's peek needs row-group 79 → the fan-out must deliver row-groups 74..79 → it is
blocked on column 10's full channel → column 10 drains only when a chunk task finishes → its tasks wait
for column 3's id. The cap that deadlocks depends on the data: it must be larger than the number of
chunks a column produces during the longest coalescing span. `n = 64` happens to be enough for this
shape (§4.1); nothing guarantees it in general.

## 2. Invariant and mechanism

**The invariant (progress rule).** A bound may stop the producers only while the sink can make
progress without them, that is, while the lowest live `SequenceId` is parked in (or granted by) the
sink's `collapse`. Whenever the lowest live id is held anywhere else, it may be waiting for input, and
input must flow.

This is tessera's `StreamWindow` rule from the ADR-0026 amendment ("the window must never block the job
the committer waits on") applied to a writer where the job the committer waits on is not a unit we can
schedule. It may be a peek in another column, three stages upstream.

### Alternatives

- **A. Fixed fan-out cap** (`buffered(n)` in `ChunkedLayoutStrategy`). Not deadlock-free: it
  deadlocks at `n = 2` and `n = 8` (§4.1), and a safe `n` depends on the data. Same bytes; `C × n`
  chunks when it completes. **Rejected.**
- **B. The #681 candidate as stated** (a per-chunk byte budget at the fan-out that exempts the chunk
  with the lowest pending id). Not deadlock-free: it deadlocks at `B = 1` (§4.1), because the lowest
  live id is held at an upstream peek, not by a chunk at the fan-out. Same bytes. **Rejected as
  stated**; its corrected rule is the decision (F).
- **C. Per-column budgets plus a global lowest-id exemption.** Deadlock-free only with the starvation
  override, and then equivalent to F with `C` knobs instead of one. A stalled column still blocks the
  shared fan-out. Same bytes; bound `C × B_col + D + K`. **Rejected:** more knobs, no gain.
- **D. Spill pending segments above `B` to a scratch file** and read them back in order. Trivially
  deadlock-free, same bytes, and the only option bounded below `D` (`B + K`). Costs: an extra write and
  read of the overflow, and a scratch-dir knob in every writer. The producers still out-run the sink,
  so it spills at production rate, and `D` is only ~100 MB here. **Not pursued** (open question 3).
- **E. Change the segment order** (per-column runs, or completion order). Deadlock-free with a small
  bound, but it **changes the bytes** and every `content_hash`. **Rejected.**
- **F. A budget on pending sink bytes at the writer's input, with a starvation override.**
  Deadlock-free (argument below; measured at `B = 1`), same bytes, bound `B + D + ε + K`. **Chosen.**

A sink-side fix for §1.3 alone (writing segments from `BufferedSegmentSink::write` directly, or a deeper
output channel) would help the throughput on a single thread. It would not bound anything: any sink
slower than the encoder (a slow disk) still lets the backlog grow. It is not a substitute for F.

### The mechanism (F)

- **Accounting.** `BufferedSegmentSink::write` adds a segment's byte length to a write-scoped
  `pending` counter on entry, *before* `collapse`, and subtracts it once the buffers have been handed to
  the output channel, before its `SequenceId` drops.
- **Starvation.** `SequenceUniverse` records which id `collapse` has granted (set when its
  `WaitSequenceFuture` completes, and also at the moment `remove` wakes the next waiter, so a woken
  waiter never looks unparked), and answers `starved() = lowest live id ∉ parked ∪ {granted}`.
- **The gate.** `write_internal` wraps the input `ArrayStream` before it tags ids
  (`writer.rs:159-165`). The wrapper polls the input only while `pending < B || starved()`. Otherwise
  it registers its waker as a universe *observer*, re-checks (so no wake-up is lost), and returns
  `Pending`.
- **Wake-ups.** `SequenceId::drop` wakes all observers (`sequence.rs:147`). A cancelled (dropped)
  `WaitSequenceFuture` removes its waker without a removal, so the patch wakes the observers there
  too; that path is only reached on an error or teardown. The gate condition can only
  turn true on a removal: `pending` drops right before the writing segment's id drops; starvation can
  only start when the lowest id changes; and the lowest id changes only on a removal, because new ids
  are always created above a live id (§1.1). So waking on removals is complete. When no observer is
  registered it costs one `is_empty` check under the lock the removal already takes.
- **Push API.** `VortexWriteOptions::writer()` (`writer.rs:255`) feeds the same `write_internal`, so
  `Writer::push` gets the back-pressure for free: it awaits its `bounded_async(1)` send while polling
  the writer.

### Deadlock freedom

Premises:

- Stock (no gate) is deadlock-free under every fair schedule, including schedules that delay polling the
  input for arbitrarily long. The open-gate case below rests on exactly that: the gate only delays.
- `BufferedSegmentSink::write` is the only caller of `collapse` (`segments/writer.rs:51`, at `608d8a1`
  and on upstream `develop`), so a granted lowest id always belongs to a sink write that is about to
  send its buffers.

Take any state in which nothing can progress.

- **If the gate is open**, the gated stream behaves exactly like its input, so this is a stock state
  (under a schedule that delayed the input) with nothing able to progress. By the first premise,
  impossible.
- **If the gate is closed**, then `pending ≥ B` and the sink is not starved, so the lowest live id `L`
  is parked in or granted by `collapse`. Its holder either is already granted, or is woken by the
  `remove` that made `L` lowest (`sequence.rs:215-222`). It then sends its buffers to the write loop,
  the writer's own future, which `block_on` keeps polling. That is progress. Contradiction.

And the gate cannot miss its own opening: it re-checks after registering, and everything that can open
it ends in a removal, which wakes it. So the write always completes, for every `B ≥ 0`. `B = 0` and
`B = 1` turn the writer into "admit input only when the sink is starved", the tightest setting, and
still complete (measured).

### The memory bound

With `B` the budget, `D` the order debt (§1.2), `K` the pipeline's working set and `ε` the overshoot:

```text
peak pending segment bytes  ≤  B + D + ε
peak encode heap            ≲  B + D + ε + K
K ≈ C × (2 × 2 MiB Buffered + ~2 MiB Repartition
         + 2P × ~1 MiB Compressing/Zoned queues)
```

- `ε` is the overshoot of an admission made just under `B`. It is not one input chunk's segments (a
  65,536-row random `u64` chunk is only 0.5 MB): `BufferedStrategy` releases its blocks in bursts of up
  to 4 MiB (`buffered.rs:80`), and the `buffered(P)` queues add up to `P × 1 MiB` blocks in flight. So
  `ε ≲ 4 MiB + P × 1 MiB` per column. Measured: 4–7 MB in the random runs (§4.2).
- The `+ D` covers rows that are already inside the per-column pipelines when pending crosses `B`;
  they park afterwards. In practice the peak lies between `max(B, D)` and `B + D` (pairs:
  `B = 256 MiB` → 301.6 MB pending, with `D` ≈ 102 MB).
- `K`'s terms are upper bounds. The measured `K` (peak heap minus peak pending) is 8–43 MB
  single-threaded and up to 45 MB in the gated runs with 3 extra executor threads (51 MB stock).

Neither `D` nor `K` grows with the table's length. Below `D` the budget does not bind, because the
starvation override admits exactly the input the slow column needs. Getting under `D` takes alternative
D (spilling).

**What `D` means for tessera:** with `W` encode workers, a staged table job's encode adds at most about
`W × (B + D + ε + K)` on top of the `StreamWindow` bound, instead of today's `≈ 0.4 × S` per job.

## 3. Determinism

The gate decides only **when** the writer polls its input:

- It never reorders, drops or splits input chunks.
- Ids are allocated in input order by `ptr.advance()` (`writer.rs:163`), whatever the timing.
- Every later id comes from the data and the strategy's thresholds (row counts, `nbytes`, dictionary
  limits), never from timing.
- Segment order is id order (`collapse`), and the layout tree collects children in input order
  (`buffered` preserves order).

So the file bytes, and tessera's `content_hash` and `manifest_hash`, are the same for every `B`, every
executor thread count and every schedule. The executor schedules nondeterministically (async-executor's
`run` picks tasks with an RNG, `src/lib.rs:742`; stock peaks vary run to run, the bytes don't).

Measured (§4): one SHA-256 per shape and size across stock, `B ∈ {1, 16 MiB, 64 MiB, 256 MiB}`, the
literal-candidate and capped runs that completed, and 0, 1 or 3 extra executor worker threads.

**How the implementation round proves it:**

1. A byte-identity matrix test in the fork, upstream as a `vortex-file` test: shapes {multi-column
   with `bool`/`u8`/`u64`/nullable, single random `u64`, single sequential `u64`} × `B` ∈ {0, 1,
   64 KiB, 16 MiB, unbounded} × executor {current-thread, + 1 and 3 pool workers, tokio
   multi-thread}. Every output must equal the unbounded current-thread output.
2. Tessera's conformance corpus goldens, unchanged across the pin bump (the flake's conformance checks),
   and `stream::tests::staged_table_jobs_seal_the_in_memory_product_for_every_worker_count_and_window`.

## 4. Prototype measurement

**Harness.** `vortex-file/examples/bp681.rs` in the scratch clone writes one table through the default
strategy, configured exactly like tessera's (`deterministic_table_compressor`: ALP/ALPRD excluded, Pco
added). The input is a lazy source of 65,536-row chunks (tessera's `ROWS_PER_GROUP`), going into a sink
that only counts and hashes. A counting global allocator reports the peak live heap and the bytes
written at that peak. The future is polled by hand on a single-threaded smol executor. That gives a
**deterministic deadlock detector**: the write is `Pending`, no task is runnable, no wake-up is
outstanding, and nothing outside the thread can wake it. On a deadlock the harness prints the live and
the parked ids. Modes are selected by `VORTEX_681`: unset = stock, `fixed:N` = alternative A,
`lowest:B` = alternative B, `gate:B` = the decision. The shapes are the consumer's `pairs` (same
generator as MorePET/visionx-rs `stream_memory.rs`), a single random `u64` (SplitMix64) and a single
sequential `u64`.

**Runs.** ksb job `vortex-681-sweep-1`: `ksb run --cpu 4 --exclusive-cpus --mem 8Gi --time 60`, flake at
`job/` in the scratch clone, which runs `job/sweep.sh` over the release-built harness.

The job's exit code was 0: 64 runs, 61 completed and 3 deadlocked as predicted (each exit code 3).
Log: `job/sweep-1.log`, table: `job/sweep-1.table`. All numbers are MB (10⁶ bytes):

- *peak heap* is the counting allocator's high-water mark above the baseline;
- *peak pending* is the high-water mark of bytes inside `BufferedSegmentSink::write`;
- *written* is what had reached the sink at the heap peak.

The sweep is single-threaded (tessera's encode runtime) unless a thread count is given.

### 4.1 Deadlocks (consumer shape, 67 M rows)

| Mode | Result | Live heap / written at the deadlock |
| --- | --- | --- |
| `fixed:2` (`buffered(2)`) | **DEADLOCK**, exit 3, after 1.0 s | 42.7 / 0.0 |
| `fixed:8` | **DEADLOCK**, exit 3, after 2.5 s | 72.2 / 30.3 |
| `fixed:64` | completes, same bytes as stock | peak 225.0 (as stock: the cap never binds) |
| `lowest:1` (#681 candidate as stated) | **DEADLOCK**, exit 3, after 1.9 s | 52.3 / 30.3 |
| `lowest:64 MiB` | completes, same bytes | peak 139.6 |
| `gate:1` (decision) | completes, same bytes | peak 122.6 |

The `lowest:1` deadlock has the same shape as §1.4. Its parked ids are columns 5–9's chunks at
row-group 127, waiting behind `[0, 127, 2, 7, 0]`: the `bool` column `prompt`'s `BufferedStrategy` peek.
The exemption never fires because no parked chunk is the lowest id. A local reproduction of the same
three deadlocks, with the id dumps, gave the same result.

### 4.2 Memory against size and budget

| Shape | Rows | Encoded | Stock peak heap | `gate:1` | `gate:16 MiB` | `gate:64 MiB` | `gate:256 MiB` |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| pairs (12 col) | 67 M | 512.3 | 225.0 (0.44×) | 122.6 | 122.6 | 130.5 | 225.0 |
| pairs (12 col) | 268 M | 2,048.7 | **834.3 (0.41×)** | **134.2** | 134.1 | 140.1 | 344.8 |
| random `u64` | 67 M | 537.1 | 507.1 (0.94×) | 14.3 | 30.1 | 80.6 | 282.6 |
| random `u64` | 268 M | 2,148.4 | **2,007.4 (0.93×)** | **15.7** | 31.5 | 82.0 | 283.9 |
| sequential `u64` | 67 M | 0.1 | 10.3 | 10.3 | 10.3 | 10.3 | 10.3 |
| sequential `u64` | 268 M | 0.4 | 12.2 | 12.2 | 12.2 | 12.2 | 12.2 |

Peak pending bytes (the part the budget governs):

| Shape | Rows | Stock | `gate:1` | `gate:16 MiB` | `gate:64 MiB` | `gate:256 MiB` |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| pairs | 67 M | 204.5 | 102.7 | 102.5 | 102.7 | 204.5 |
| pairs | 268 M | 799.3 | 102.1 | 102.1 | 102.1 | 301.6 |
| random | 67 M | 498.2 | 6.3 | 22.0 | 72.4 | 273.7 |
| random | 268 M | 1,992.6 | 6.3 | 22.0 | 72.4 | 273.7 |

What the tables show:

- **The bound does not depend on the input size.** With the gate, every shape's peak is the same at
  67 M and 268 M rows, within 12 MB, except pairs at 256 MiB: at 67 M rows that budget never binds
  (the pile stays below it, 225.0 MB as stock), at 268 M it does (344.8 MB). Stock grows 3.7× (pairs)
  and 4.0× (random). The time series
  (pairs, 268 M rows, every 512 row-groups) shows stock rising steadily (143 → 216 → 309 → 449 → 521 →
  593 → 751 MB live), while `gate:1` is flat from the first sample to the last (108, 114, 113, 112,
  118, 120, 117 MB).
- **The order debt `D`.** At `B = 1` pairs still holds 102 MB pending: that is `D` (§1.2). Random has no
  cross-column order, so `D ≈ 0` and it holds 6.3 MB, which is `ε`.
- **The budget binds above `D`.** Random: pending = `B` + 5–7 MB (16.8 → 22.0, 67.1 → 72.4, 268.4 →
  273.7; 4.2 MB with 1 extra thread, 7.3 MB at `gate:1` with 3). Pairs at 256 MiB: 301.6, which is
  between `max(B, D)` and `B + D`. The rows already inside the
  per-column pipelines when pending crosses `B` park afterwards, so the overshoot above `B` can reach
  `D`.
- **`K`, the encode's working set beyond the pending bytes**, is 8–43 MB single-threaded (peak heap
  minus peak pending; 35.0 stock, 38.0 at 64 MiB and 43.2 at 256 MiB for pairs at 268 M rows), and up
  to 45 MB in the gated runs with 3 extra executor threads (more compressions in flight).
- **No throughput cost.** Wall time is the same within run-to-run noise: pairs at 268 M rows took 83.9 s
  stock vs 79.3 s (`gate:1`), 85.2 s (16 MiB), 79.6 s (64 MiB) and 82.7 s (256 MiB); the time-series
  pair took 63.0 s vs 64.1 s. The gate reorders the same single-threaded work; it doesn't add work.

### 4.3 Byte identity

Each of the 9 inputs (3 shapes × 16.8 M / 67 M / 268 M rows) produced **exactly one SHA-256** across
every completed run:

- at 67 M and 268 M rows: stock, `gate:{1, 16 MiB, 64 MiB, 256 MiB}`, and for pairs at 67 M also
  `fixed:64` and `lowest:64 MiB`;
- at 16.8 M rows: {stock, `gate:1`, `gate:64 MiB`} × {0, 1, 3} extra executor worker threads.

| Input | Runs | Digest |
| --- | ---: | --- |
| pairs, 16.8 M | 9 | `150dc2599bcaf55a…` |
| pairs, 67 M | 7 | `2d3481d671025b56…` |
| pairs, 268 M | 7 | `e6c81bdc9347327d…` |
| random, 16.8 M | 9 | `bef4f31f919e1576…` |
| random, 67 M | 5 | `bb0380d1570fa91b…` |
| random, 268 M | 5 | `b03482be1e71667a…` |
| sequential, 16.8 M | 9 | `a3f4980d1a997620…` |
| sequential, 67 M | 5 | `8475d9848b562ad2…` |
| sequential, 268 M | 5 | `137d957176b7cc48…` |

The multi-threaded runs also confirm §1.3. With 3 extra executor threads, the write loop no longer
waits behind 200-task batches, and stock random drops from 0.99× to 0.39× of the encoded size at
16.8 M rows. The gate still takes it to 0.26×.

## 5. Plan for the implementation round

### 5.1 The Vortex patch

Target upstream `develop` first, then backport onto the fork's 0.75 line. Upstream has since threaded a
write-scoped `LayoutWriterContext` with a `BufferedBytesTracker` through every strategy
(`vortex-layout/src/strategy.rs` on `develop`, #8961), and reduced the universe's lock traffic
(#10867). The patch must keep #10867's fast path: observers are only touched when non-empty.

- `vortex-layout/src/sequence.rs`: `SequenceUniverse` gains the `granted` id, the `observers` and
  `starved()`. A public `SequenceProgress` handle from a `SequencePointer` (`is_starved()`,
  `observe(&Waker)`) is documented as the back-pressure hook. `remove` wakes the observers.
- `vortex-file/src/segments/writer.rs`: `BufferedSegmentSink` gains `pending` and `peak_pending`
  counters around `collapse` and the send. The decrement is an RAII guard declared after the
  `SequenceId`, so it runs before the id drops on every path, including the early `?` returns
  (the prototype's plain `fetch_sub` would leak on them).
- `vortex-file/src/writer.rs`: `VortexWriteOptions::with_max_pending_bytes(u64)`, the input gate in
  `write_internal`, and `WriteSummary::peak_pending_bytes()` / `Writer::pending_bytes()` for
  instrumentation.
- Docs: rustdoc on the knob with the rule, the `B + D + ε + K` bound, the guarantee that bytes never
  change, and the `DType::Null` limit.

Public knob: `with_max_pending_bytes`. The default is open question 2: the recommendation is
64 MiB. The prototype's `fixed:`/`lowest:` modes and the `debug681` statics are spike-only and
do not ship.

### 5.2 Tests

All in Vortex, upstream:

1. **Deadlock regression:** `B ∈ {0, 1}` on the multi-column shape with a `bool` column, driven by the
   hand-polled single-thread executor that reports idle-while-pending as a failure (no timeout). Shown
   to fail without the starvation override once (mutation: drop `|| starved()`).
2. **Byte-identity matrix** (§3).
3. **Bound assertion:** the exact accounting (`peak_pending_bytes ≤ B + ε`, with
   `ε = 4 MiB + P × 1 MiB`, the Buffered flush burst plus the compress queue) on the single random
   column, where `D = 0` (measured overshoot: 4–7 MB). Plus size-independence on the real heap: a counting
   allocator in its own test binary, peak at 4× rows within a fixed slack of the peak at 1× rows. Fails
   on stock, which grows linearly.

### 5.3 Upstream PR (vortex-data/vortex)

The implementation round prepares the branch and the PR text: the root cause from §1, the decision and
the measurements. Upstream requires DCO with author `Lars Gerchow <lars.gerchow@gmail.com>` and an
AI-use disclosure. **The owner signs and submits.**

### 5.4 Tessera

1. **Pin bump.** The fork backport is a commit on top of `608d8a1` on `gerchowl/vortex`. Every one of
   the 27 vortex `rev =` lines in `tessera/Cargo.toml` moves together, plus `Cargo.lock`. The #480
   checklist gains a third patch to wait for upstream. No new dependencies, so no feature-snapshot churn.
2. **The knob.** `encode_streaming_to` sets `with_max_pending_bytes`. Recommended: a `WriteConfig` field
   (`encode_pending_bytes`, default 64 MiB), passed to staged table jobs. It does not belong in
   `StreamWindow`: the window counts jobs and the bytes they hand over, while this budget lives inside
   one job's encode. `StreamStats` gains the encode's peak pending bytes, so a consumer can assert the
   whole bound.
3. **Tests.** `tests/staged_job_memory.rs` moves from "no worse than the encoder itself" to an absolute
   bound (peak at 4× rows within a fixed slack of 1× rows). The corpus goldens must not move.
4. **ADR-0026.** The "single-table output sink" amendment's *Known limit* paragraph is replaced by the
   bound: per encode worker `B + D + ε + K` (with `D` and its meaning), instead of `≈ 0.4 × S`.
   `encode_streaming_to`'s rustdoc changes with it.
5. **ksb RSS proof** with the consumer reproducer (MorePET/visionx-rs PR #41, branch
   `chore/39-tessera-677-repro`), built against the bumped tessera, at 268 M and 1.6 G rows. The same
   job shape as the amendment's table: `--cpu 4 --exclusive-cpus --mem 12Gi`, peak `RssAnon` sampled
   every 10 ms. Expected: flat in the row count. That closes #677's "independent of input size"
   requirement.

## 6. Open questions for the owner

1. **Tessera's default budget and where it lives.** *Recommendation:* `WriteConfig::encode_pending_bytes`,
   default 64 MiB. It is per encode worker, so the product's bound is `W ×` that. Below `D` the budget
   doesn't bind, and above ~64 MiB it only buys peak RAM (§4).
2. **Upstream default: on or opt-in?** *Recommendation:* propose a 64 MiB default. The unbounded
   default is the defect itself, and §4.2 measured no throughput cost and no byte change. Fall back to
   opt-in only if upstream asks. Tessera sets it explicitly either way.
3. **Is `D` acceptable?** About 100 MB for the consumer's shape, independent of the input size, and set
   by the format's own 1 MiB block threshold. *Recommendation:* accept it, and document it with the
   formula. Going below it means spilling (alternative D), which isn't worth a scratch-dir knob in the
   writer at this size. Revisit only if a consumer's budget can't absorb `W × D`.
4. **Carry it on the fork before upstream merges?** *Recommendation:* yes, as a third patch on the
   pinned fork, tracked in #480 like the two determinism fixes. #677 can close on the ksb proof, without
   waiting on upstream's review queue.
