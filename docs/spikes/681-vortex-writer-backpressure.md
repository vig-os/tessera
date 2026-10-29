# Spike — a deadlock-free bound on the Vortex writer's pending segments (#681)

> **Status:** design, for the implementation round of #681 (PR 3 of #677). No product code changes here.
> Revision 2 answered an independent pre-mortem of revision 1 (§8): its memory claims were wrong, and
> were replaced by measured ones. Revision 3 answers a re-review of revision 2's recommendation (§8): it
> adds P-logical (§5.1), the input side and the `min(W, n)` model of the per-block bound (§4.7), and the
> writer-output change since alpha.4. Every Vortex claim cites `file:line` at the pinned fork
> `gerchowl/vortex@608d8a1` (tessera's `[patch.crates-io]`, #480) unless it names upstream `develop`
> (`vortex-data/vortex@40e0b1ccd`, 2026-10-28). The reproduction and the prototype ran in a scratch clone
> at that SHA (`/mnt/HDD/larsgerchow/vortex-681`, branch `spike-681`, never pushed). The prototype diffs,
> the harness, the sweep scripts and every log are kept in its `job/` directory (§4).

## Decision needed

**The acceptance of #677 is a bounded RSS that does not depend on the input size, for one scan product
with "single named `pairs`, `markers`, `ms_stats` table blocks".** #677 explicitly rejects
`TableMultiBlockSink`, because it "changes the requested logical block into suffixed physical blocks". The
triage made that a rule, recorded in ADR-0026: "One logical block stays **one canonical block**: no
suffixed physical blocks" (`docs/adr/0026-streaming-table-ingest.md:190-191`). **So no partitioning
option meets #677 as written.** Partitioning meets the memory half. The owner decides whether "one
logical table" may replace "one canonical block".

**What does not bound it.** The writer fix F (§2) does not bound it on its own. Per encode worker it leaves
two terms:

- **The order debt grows with width.** The order debt `D` grows with the width and no budget pushes it
  down. It is 73 MB → 4,566 MB for 1 → 64 random `u64` columns (§4.3).
- **The heap grows with rows.** Under F the heap still grows by 0.058 B/row for the consumer shape. About
  half of that is the zone-map statistics (§1.4).

A single canonical block of unbounded rows cannot have a bounded RSS with the same bytes. Something must
bound the rows one encode holds.

**The three options that bound it** (§5.2):

| | Bounds RSS | Keeps `pairs` as one name | One canonical block | Change |
| --- | --- | --- | --- | --- |
| **P**: `BLOCK_ROWS` blocks `pairs_0000`, … | yes | via the `_NNNN` match | no | writer output |
| **P-logical**: `pairs/0000`, … (ADR-0063) | yes, as P | yes, a name-tree node | no | SPEC §5b + ADR |
| **R**: Vortex row groups in one block | no: +0.014 B/row | yes | yes | Vortex patch + canonical bytes |

Names follow the caller's prefix (`tessera-io/src/table.rs:890-896`), so P names the consumer's blocks
`pairs_NNNN`, not `events_NNNN`.

**Measured bound of P and P-logical** (ksb `vortex-681-sweep-3`, exit 0, §4.7). The consumer shape uses
4 Mi-row blocks, a reader thread, input staged to disk per block and `W` encode workers:

```text
peak heap  ≤  min(W, n_blocks) × p_block  +  s
disk       ≤  (W + 1) × BLOCK_ROWS × record bytes
```

- These are heap numbers from the counting allocator. RSS adds allocator retention, thread stacks and
  the binary, so the RSS proof (§6.4) measures its own `p_block` as RSS.
- `p_block` is one block's encode peak: 46.1 MB at 8 CPUs.
- `s` is the staging slack: one row-group of records plus the reader's buffers, measured at 0.0 MB above
  `p_block` with `W = 1`.
- With `W = 4` the peaks are 128.0, 142.0 and 143.1 MB at 67 M, 268 M and 1.07 G rows (16, 64 and 256
  blocks). They are under `4 × 46.1 = 184.4`. They rise by 11% from 16 to 64 blocks and by 0.8% from
  64 to 256, as more of the 4 workers overlap.
- Disk is 402.7 MB, which is 4 staged blocks.
- **The input side is a real term.** Building a block in memory before dispatch raises the peak to
  391.3 MB. That is the term §6.4 of revision 2 left out.
- **Disk staging is chosen on memory and structure, not on measured speed.** In this harness it was
  slower than a bounded channel: 14.9 against 12.1 s at 16 blocks, and 45.4 against 26.1 s at 268 M rows.
  The cause is the harness's single-threaded record writer.
  - A channel serializes the encoders by construction: `W = 4` was 12.1 s against 14.8 s at `W = 1`.
  - Decoupling the reader from the encoders is what lets them run in parallel. `fill` shows that
    speed-up at 3.0×, but holds the blocks in RAM.
  - The consumer reproducer's throughput run decides (§6.4). The bounded channel is the fallback if
    disk staging does not beat it there.

**F is separate.** It is the upstream writer fix, and it lowers `p_block` up to 4.2× (`wide:16`
527.0 → 126.5 MB). For #677's shape it changes nothing inside a block (46.0 MB either way). It goes
upstream after the #10046 conversation (§6.3), and #677 does not wait for it.

**Writer output changes; readers don't.** alpha.4 (`v0.1.0-alpha.4`, `2e9f530`) shipped
`table_job_streaming` as "ONE canonical table block of **any size**" (`tessera-io/src/stream.rs:121`).
Under P or P-logical, a streamed table above `BLOCK_ROWS` is written as several blocks. That is a
different layout, different block digests and a different `content_hash` from what alpha.4 wrote for the
same rows.

- Every product alpha.4 wrote still reads: no reader changes for P, and P-logical's readers will accept both
  names.
- Products of at most `BLOCK_ROWS` rows don't move.
- P matches the partition SPEC §5b already requires of every writer (`tessera/docs/SPEC.md:156-170`).
  alpha.4's single oversize block was the outlier.

**Recommendation: P-logical.**

- It keeps the consumer's one name with measured, bounded memory.
- It makes the grouping a property of the name tree that ADR-0063 already enforces, rather than a
  `_NNNN` convention a user's own `pairs_0003` block can join.
- It costs a SPEC §5b wording change and one ADR.

If the owner holds to one canonical block, R is the only option. It does not fully bound the RSS, and
it needs a Vortex change the maintainers have not agreed to.

**The implementation rounds, if P-logical is taken.**

1. **ADR** (the first PR): "logical tables over bounded physical blocks".
   - It retires ADR-0026's canonical-block rule for tables above `BLOCK_ROWS`.
   - It amends ADR-0063 (a node with numbered leaves is a logical table) and SPEC §5b (names and the
     logical-table definition).
   - It records the writer-output change since alpha.4 and the bound above.
2. **Tessera, track 1** (§6.4):
   - `block_group` accepts `prefix/NNNN`;
   - the partitioning job API with disk-staged blocks, dispatched as each block fills;
   - the read paths in §5.1's table;
   - per-block tests modelled as `min(W, n) × p_block` (§6.2);
   - the ksb RSS proof with the consumer reproducer at 268 M and 1.6 G rows.
3. **Track 2** is F upstream, independent of the rest (§6.1-§6.3).

## TL;DR

- **Two causes of the pile, as in revision 1.**
  - *Sink starvation.* Nothing slows the producers down, and the single-threaded executor drains the sink
    only between batches of 200 tasks. The pile grows with the input: a single random `u64` column holds
    0.93 × its encoded size.
  - *Order debt `D`.* Segments go to the file in one global `SequenceId` order, so the slowest-coalescing
    column holds every other column's later segments.
- **Revision 1 got the size of `D` wrong.** It is not a fixed ~100 MB: `D ≈ span × Σ encoded bytes per
  row`. The span is 8.4 M rows for a table with a `bool` column, so `D` grows with the table's width
  (measured within 6–8% of the formula for `N` random `u64` columns, §4.3).
- **A third term grows with the row count:** `c · rows` with `c` ≈ 0.058 B/row for the consumer shape.
  - About half is the zone-map accumulator, which keeps every zone's statistics until the stream ends
    (`zoned/writer.rs:102,137,158`). It was confirmed with a counting allocator and an A/B with zone
    stats off, where the gap equals the zone tables' size to 0.1 MB (§4.2).
  - The other half persists with zone stats off and is not attributed.
- **F is still the right fix for what it fixes.**
  - It is a budget on pending sink bytes at the writer's input, with a starvation override.
  - It is deadlock-free in all 88 gated runs (of 127 completed runs in total): every shape, at `B = 0`
    and `B = 1`, with 0–3 extra executor threads and 1–4 concurrent writers with per-writer counters.
  - Every input gave the same bytes in every byte-preserving mode: 17 inputs, one SHA-256 each.
  - It removes the starvation growth: the consumer shape at 268 M rows peaks at 834.3 MB stock and
    134.2 MB with F (both from sweep 1).
  - It cannot remove `D` or `c · rows`.
- **The per-worker bound, every term measured:**

  ```text
  peak heap  ≲  B + D(width) + ε + K(width) + c · rows
  ```

  With bounded blocks, `rows ≤ BLOCK_ROWS` and `D ≤ BLOCK_ROWS × Σ bytes/row`, so the bound is
  independent of the input size. With `W` encode workers, up to `min(W, n_blocks)` blocks are
  encoded at once. The input must be staged to disk, or it adds up to `W + 1` blocks of rows (§4.7).

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

### 1.2 Where ids are held while waiting for input: the order debt `D(width)`

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

So once the slowest-coalescing column has emitted a block, its `BufferedStrategy` peek holds the lowest
live id until its next block, a whole span later. Every segment the other columns produce in the
meantime has a higher id and must wait in the sink. This order debt is what an in-memory writer must
hold to write those bytes in that order:

```text
D ≈ max_c span_rows(c) × Σ_c' encoded_bytes_per_row(c')
span_rows(c) ≈ 1 MiB / canonical_bytes_per_row(c)  (strategy.rs:259)
```

The sum runs over **all** columns, so `D` grows linearly with the table's width. A `bool` column gives
`span ≈ 8.4 M rows`.

- **Random `u64` columns** (incompressible, ~8 B/row each): the formula predicts 68, 270, 1,076 and
  4,298 MB for 1, 4, 16 and 64 columns. The measured pending peaks are 73.4, 287.4, 1,143.1 and
  4,566.3 MB, 6–8% above, from the segments in the stages' own queues (§4.3).
- **The consumer shape** (12 columns: `pkt` u64 sequential, nullable `ms` u32, 2 × `bool`, 7 masked
  `u8`, `tof` i16): the formula gives ≈ 64 MB and 102 MB was measured. Its `u8` columns' `Dict` and
  `Buffered` stages hold more than the formula counts.

`D` does not depend on the table's length; it is reached once the slow column has emitted its first
block. A column whose canonical form has **zero** bytes per row (`DType::Null`) never fills a block,
so its span is the whole table. Tessera never writes such a column: its dtypes are numeric, `bool` and
`utf8`.

### 1.3 Why the sink falls behind (the growth F removes)

The stock pile grows with the table: 225 MB at 67 M rows and 834 MB at 268 M rows for the consumer's
shape (§4.2). The cause is scheduling. `CurrentThreadRuntime::block_on` runs
`executor.run(write_future)` (`current.rs:102`). The executor (async-executor 1.14.0,
`src/lib.rs:740-756`) runs ready tasks in batches of 200 and polls the caller's future (here the write
loop, `writer.rs:204-209`, the sink's only consumer) only between batches, or when no task is ready.
With `buffered(usize::MAX)` some task is always ready, and the sink's `bounded_async(1)` channel lets
one buffer through per poll of the write loop. So the producers out-run the sink at any table size.
Measured on a single random `u64` column, which has no cross-column order debt: the live heap reaches
0.93–0.94 × the encoded size (§4.2).

### 1.4 What grows with the row count: `c · rows`

The pre-mortem measured the consumer shape under F growing with the row count. Its peak comes at the
end of the write. Confirmed (§4.2, ksb `vortex-681-sweep-2a`):

- **The heap grows, pending doesn't.** Peak heap is 122.6, 133.1 and 179.6 MB at 67 M, 268 M and
  1.07 G rows. Pending stays flat at 102.7, 102.1 and 103.5 MB. At 1.07 G rows the peak comes at
  row-group 16,384 of 16,384.
- **Half is the zone statistics (confirmed).** `ZonedStrategy` creates one `StatsAccumulator` per
  column (`zoned/writer.rs:102`) and pushes one entry per 8,192-row zone into columnar builders
  (`:137`, `zoned/builder.rs:77`). It finishes them into one stats table only when the column's
  stream ends (`zoned/writer.rs:158`). That table is written under the column's `eof` id
  (`:169,172`), after all data. With zone stats switched off (`VORTEX_681_ZONES=off`, a diagnostic
  that changes bytes), the growth from 268 M to 1.07 G rows drops from 46.5 MB to 23.4 MB. The
  23.1 MB difference equals the growth of the finished zone tables' `nbytes`, 7.68 → 30.74 MB.
- **The other half is not attributed.** It is 0.029 B/row and persists with zone stats off. With
  top-level row groups (§5) the growth falls to 0.014 B/row, because zone stats are then flushed per
  group. The remaining candidates are per-chunk state held until the footer: the layout tree's child
  layouts (`chunked/writer.rs:89`) and the segment specs (`segments/writer.rs:23,75`). They were not
  measured one by one, and the implementation round must attribute them.

The zone table is one segment per column, compressed in one piece at the end. So no byte-identical
writer can stream it. The term disappears only if the rows one encode holds are bounded.

### 1.5 The `buffered(n)` deadlock: the wait-for cycle

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
for column 3's id. A safe cap depends on the data; `n = 64` happens to be enough for this shape.

## 2. Invariant, mechanism and bound

**The invariant (progress rule).** A bound may stop the producers only while the sink can make
progress without them, that is, while the lowest live `SequenceId` is parked in (or granted by) the
sink's `collapse`. Whenever the lowest live id is held anywhere else, it may be waiting for input, and
input must flow.

### Alternatives for the back-pressure itself

- **A. Fixed fan-out cap** (`buffered(n)` in `ChunkedLayoutStrategy`). It deadlocks at `n = 2` and
  `n = 8` (§4.1). **Rejected.**
- **B. The #681 candidate as stated** (a byte budget at the fan-out that exempts the chunk with the
  lowest pending id). It deadlocks at `B = 1` (§4.1): the lowest live id is held at an upstream peek,
  not by a chunk at the fan-out. **Rejected as stated**; its corrected rule is F.
- **C. Per-column budgets plus a global lowest-id exemption.** Deadlock-free only with the starvation
  override, and then F with `C` knobs. **Rejected.**
- **E. Change the segment order** (per-column runs, completion order). It **changes the bytes** of every
  table at every size. **Rejected.**
- **F. A budget on pending sink bytes at the writer's input, with a starvation override.**
  Deadlock-free, same bytes. **Chosen**, as the back-pressure. What bounds `D` and `c · rows` is a
  separate question (§5).

(Spilling, revision 1's alternative D, bounds memory rather than providing back-pressure. It is now
measured in §5.)

### The mechanism (F)

- **Accounting.** `BufferedSegmentSink::write` adds a segment's byte length to the **writer's own**
  pending counter on entry, *before* `collapse`, and subtracts it once the buffers have been handed to
  the output channel, before its `SequenceId` drops. Revision 2's prototype keeps the counter in the
  writer's `SequenceUniverse`, one per write (`SequenceId::root`, `writer.rs:157`), so W concurrent
  writers never share it (§4.5).
- **Starvation.** `SequenceUniverse` records which id `collapse` has granted (set when its
  `WaitSequenceFuture` completes, and also at the moment `remove` wakes the next waiter), and answers
  `starved() = lowest live id ∉ parked ∪ {granted}`.
- **The gate.** `write_internal` wraps the input `ArrayStream` before it tags ids
  (`writer.rs:159-165`). The wrapper polls the input only while `pending < B || starved()`. Otherwise
  it registers its waker as an observer of *its* universe, re-checks (so no wake-up is lost), and
  returns `Pending`.
- **Wake-ups.** `SequenceId::drop` wakes the observers (`sequence.rs:147`). The gate condition can only
  turn true on a removal: `pending` drops right before the writing segment's id drops, and the lowest
  id changes only on a removal, because new ids are always created above a live id (§1.1). A cancelled
  `WaitSequenceFuture` removes its waker without a removal, so the patch wakes the observers there too
  (error and teardown paths only).
- **Push API.** `VortexWriteOptions::writer()` (`writer.rs:255`) feeds the same `write_internal`, so
  `Writer::push` gets the back-pressure for free.

### Deadlock freedom

Premises:

- Stock (no gate) is deadlock-free under every fair schedule, including schedules that delay polling the
  input for arbitrarily long. The gate only delays.
- `BufferedSegmentSink::write` is the only caller of `collapse` (`segments/writer.rs:51`, at `608d8a1`
  and on upstream `develop`), so a granted lowest id always belongs to a sink write about to send.

Take any state in which nothing can progress.

- **If the gate is open**, the gated stream behaves exactly like its input: a stock state (under a
  schedule that delayed the input) with nothing able to progress. Impossible by the first premise.
- **If the gate is closed**, `pending ≥ B` and the sink is not starved, so the lowest live id `L` is
  parked in or granted by `collapse`. Its holder is granted or is woken by the `remove` that made `L`
  lowest (`sequence.rs:215-222`); it then sends to the write loop, which `block_on` keeps polling.
  That is progress. Contradiction.

The gate cannot miss its own opening: it re-checks after registering, and everything that can open it
ends in a removal. So the write completes for every `B ≥ 0`. Measured: 88 gated runs, none
deadlocked, including `B = 0` and `B = 1` on every shape and up to 4 concurrent writers (§4).

### The memory bound, per encode worker

```text
pending   ≤  B + D(width) + ε
heap      ≲  B + D(width) + ε + K(width) + c · rows
```

| Term | Meaning | Measured (§4) |
| --- | --- | --- |
| `B` | the budget | knob; binds only above `D` |
| `D(width)` | order debt, §1.2: `span × Σ` encoded bytes/row | 73 MB → 4,566 MB, `N` = 1 → 64 `u64` |
| `ε` | overshoot of one admission (4 MiB flush burst + queues) | 4–7 MB |
| `K(width)` | per-column working set (Buffered, queues) | ≈ 8 MB per `u64` column |
| `c · rows` | zone stats + footer-bound per-chunk state | 0.058 B/row, consumer shape |

- **`B` cannot push the total below `D`.** Below `D` the starvation override admits exactly the input
  the slow column needs. `gate:0`, `gate:1` and `gate:64 MiB` measure the same pending peak on every
  `wide:N` and `mixed` input, except `wide:1` at 64 MiB (74.5 vs 73.4 MB).
- **`K` grows with the width too.** It was measured as the heap left when spilling holds in-RAM pending
  at 16.8 MB: 56.1, 149.2 and 540.9 MB for 4, 16 and 64 `u64` columns. The increase between those widths
  is 7.8–8.2 MB per added column, on top of a fixed part.
- **With W concurrent encode workers, the heap is about W × the per-worker bound.** The consumer shape at
  67 M rows: 122.6, 237.5 and 469.4 MB for W = 1, 2, 4.
- **Bounded blocks bound the rows.** With them, `rows ≤ BLOCK_ROWS` and `D ≤ BLOCK_ROWS × Σ bytes/row`.
  The process peak is then `min(W, n_blocks) ×` one block's peak plus the input side (§4.7). Measured
  per 4 Mi-row block, stock / F at 4 CPUs: 46.0 / 46.0 MB (consumer shape), 106.3 / 41.8 MB
  (`mixed`), 135.0 / 37.2 MB (`wide:4`) and 527.0 / 126.5 MB (`wide:16`).

## 3. Determinism

F decides only **when** the writer polls its input:

- It never reorders, drops or splits input chunks.
- Ids are allocated in input order by `ptr.advance()` (`writer.rs:163`), whatever the timing.
- Every later id comes from the data and the strategy's thresholds (row counts, `nbytes`, dictionary
  limits), never from timing.
- Segment order is id order (`collapse`), and the layout tree collects children in input order.

So the file bytes, and tessera's `content_hash` and `manifest_hash`, are the same for every `B`, every
executor thread count, every concurrent-writer count and every schedule. Measured across sweeps 1 and 2
(§4.6): 17 inputs, each with exactly one SHA-256. That covers every byte-preserving mode: stock, `gate`
at `B` ∈ {0, 1, 16 MiB, 64 MiB, 256 MiB}, spilling, 0/1/3 extra threads, and W = 1/2/4 writers.

Two byte facts the implementation round must respect:

- **Input chunking already moves bytes.** For `utf8` shapes, stock bytes depend on how the input is
  chunked: `mixed` written as one 2 Mi-row chunk differs from 32 × 65,536-row chunks (§4.6). Tessera
  fixes `ROWS_PER_GROUP`, so its bytes are deterministic, but any option that re-chunks the input moves
  bytes.
- **The bytes-changing options in §5** (smaller block, row groups) are format changes for tessera.

**How the implementation round proves it:**

1. A byte-identity matrix test in Vortex, upstream as a `vortex-file` test.
   - Shapes: multi-column with `bool`/`u8`/`u64`/nullable, `utf8` + `f64` + nullable, single random
     `u64`, single sequential `u64`.
   - Budgets: `B` ∈ {0, 1, 64 KiB, 16 MiB, unbounded}.
   - Executors: current-thread, + 1 and 3 pool workers, tokio multi-thread, and 2 concurrent writers.
   - Every output must equal the unbounded current-thread output.
2. Tessera's conformance corpus goldens, unchanged across the pin bump, and
   `stream::tests::staged_table_jobs_seal_the_in_memory_product_for_every_worker_count_and_window`.

## 4. Prototype measurement

**Harness.** `vortex-file/examples/bp681.rs` in the scratch clone writes tables through the default
strategy, configured exactly like tessera's (`deterministic_table_compressor`: ALP/ALPRD excluded, Pco
added).

- **Input and sink.** The input is a lazy source of 65,536-row chunks (tessera's `ROWS_PER_GROUP`),
  going into a sink that only counts and hashes.
- **Counting allocator.** A counting global allocator reports the peak live heap, the bytes written at
  that peak, and the input row-group at the peak (`peak_group`).
- **Deadlock detector.** Each write is polled by hand on its own single-threaded smol executor. A
  deadlock (`Pending`, nothing runnable, no wake-up outstanding) exits 3 and prints the live and the
  parked ids.
- **Knobs** (all environment variables):
  - `VORTEX_681` = `fixed:N` | `lowest:B` | `gate:B` selects the mode;
  - `VORTEX_681_SPILL` and `_ZONES=off` add spilling and the zone-stats A/B;
  - `_BLOCK` and `_ROWGROUP` are the bytes-changing options;
  - `BP_WRITERS=W` runs W concurrent writers, one thread and executor each, like tessera's encode
    workers.
  - `BP_BLOCKS=64` with `BP_FEED` = `lazy` | `fill` | `channel:K` | `disk` runs the per-block
    partition (P): a reader thread feeds 4 Mi-row blocks, each its own Vortex file, to at most `W`
    encode workers (§4.7).
- **Shapes:**
  - `pairs`: the consumer's 12 columns, the same generator as MorePET/visionx-rs `stream_memory.rs`;
  - `random`: one random `u64` column;
  - `seq`: one sequential `u64` column;
  - `wide:N`: 1 `bool` + `N` random `u64`;
  - `mixed`: `utf8` + `f64` + nullable `i64` + nullable `utf8` + `bool`.

**Runs.** All the heavy runs went through `ksb run --exclusive-cpus --time 60`. The flake is `job/` in
the scratch clone, and its `sweep*.sh` scripts drive the release-built harness.

| Run | Command | Exit | Result |
| --- | --- | ---: | --- |
| `vortex-681-sweep-1` | `ksb run --cpu 4 --mem 8Gi … job#default` | 0 | 64 runs: 61 OK, 3 DEADLOCK (exit 3) |
| `vortex-681-sweep-2a` | `ksb run --cpu 4 --mem 8Gi … job#sweep2a` | 0 | 12 runs, all exit 0 |
| `vortex-681-sweep-2b` | `ksb run --cpu 8 --mem 16Gi … job#sweep2b` | 0 | 54 runs, all exit 0 |
| `vortex-681-sweep-3` | `ksb run --cpu 8 --mem 16Gi … job#sweep3` | 0 | 63 runs, all exit 0, 669 s |
| local | `bp681 <shape> 64` / `32`, on the seat | 0 | `job/local-*.log`, each exit 0 |

- **Prototype revisions.**
  - Sweep 1 used `job/prototype-681.patch`.
  - Sweep 2 used `job/prototype-681-r2.patch`. It adds the per-writer counters, the spilling sink, the
    zone A/B and top-level row groups with a group-local `eof`. It also drops the compressed chunk before
    the sink awaits (`flat/writer.rs:174`): the segment buffers are views of it, so spilling could not
    free them otherwise.
  - The local identity runs used `job/prototype-681-r2b.patch` (a raw row-group cut and an
    input-chunking knob).
  - Sweep 3 used `job/prototype-681-r3.patch`: the same Vortex prototype, plus the harness's per-block
    partition (`job/bp681-r3.rs`).
- **Logs and tables.**
  - Logs: `job/sweep-1.log`, `job/sweep-2a.log`, `job/sweep-2b.log`, `job/sweep-3.log`.
  - Tables: `job/sweep-1.table`, `job/sweep-2.table`.
- **Units.** All numbers are MB (10⁶ bytes).
  - *heap* is the counting allocator's peak above the baseline.
  - *pending* is the writer's peak bytes inside `BufferedSegmentSink::write`.

### 4.1 Deadlocks (sweep 1, consumer shape, 67 M rows)

| Mode | Result | Live heap / written at the deadlock |
| --- | --- | --- |
| `fixed:2` (`buffered(2)`) | **DEADLOCK**, exit 3, after 1.0 s | 42.7 / 0.0 |
| `fixed:8` | **DEADLOCK**, exit 3, after 2.5 s | 72.2 / 30.3 |
| `fixed:64` | completes, same bytes as stock | peak 225.0 |
| `lowest:1` (#681 candidate as stated) | **DEADLOCK**, exit 3, after 1.9 s | 52.3 / 30.3 |
| `gate:1` (F) | completes, same bytes | peak 122.6 |

No run of F deadlocked: 88 gated runs on ksb, on all five shapes, `B` from 0 to 256 MiB, 0–3 extra
threads, 1–4 writers.

### 4.2 Growth with the row count (sweeps 1 and 2a)

Consumer shape, single-threaded:

| Mode | 67 M rows | 268 M rows | 1.07 G rows | Growth |
| --- | ---: | ---: | ---: | ---: |
| stock | 225.0 | 834.3 | — | ≈ 3.0 B/row (67 M → 268 M) |
| `gate:1`, heap | 122.6 | 133.1 | **179.6** (peak at row-group 16,384 of 16,384) | 0.058 B/row |
| `gate:1`, pending | 102.7 | 102.1 | 103.5 | flat |
| `gate:1`, zone stats off, heap | — | 125.7 | 149.1 | 0.029 B/row |
| zone tables' `nbytes` (with zone stats on) | 1.92 | 7.68 | 30.74 | 0.029 B/row |
| row groups of 4 Mi rows, heap | 134.0 | 136.8 | 147.7 | 0.014 B/row |

Growth is per row between 268 M and 1.07 G rows.

- **Revision 1's "independent of size" was wrong.** It compared 67 M with 268 M rows only; over that
  range the growth is 10.5 MB, which revision 1 read as noise.
- **The 0.058 B/row splits in two.** Half is the zone statistics (§1.4). The other half is not
  attributed: it survives zone stats off, and row groups cut it to 0.014 B/row.
- **A single random `u64` grows too.** Under `gate:1` it is 14.3, 15.7 and 21.1 MB at 67 M, 268 M and
  1.07 G rows (0.007 B/row; zone tables 3.15 MB at 1.07 G).

### 4.3 Growth with the width (sweep 2b, 16.8 M rows)

`wide:N` = 1 `bool` + `N` random `u64`. The table gives pending / heap:

| `N` | encoded | stock | `gate:0` | `gate:1` | `gate:64 MiB` | spill 16 MiB |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 136.4 | 121.7 / 130.2 | 73.4 / 81.9 | 73.4 / 81.9 | 74.5 / 83.9 | 16.8 / 33.0 |
| 4 | 539.2 | 500.3 / 511.9 | 287.4 / 298.5 | 287.4 / 298.5 | 287.4 / 298.5 | 16.8 / 56.1 |
| 16 | 2,150.5 | 2,039.8 / 2,065.1 | 1,143.1 / 1,165.2 | 1,143.1 / 1,165.2 | 1,143.1 / 1,165.2 | 16.8 / 149.2 |
| 64 | 8,595.7 | — | 4,566.3 / 4,631.9 | 4,566.3 / 4,631.9 | 4,566.3 / 4,631.9 | 16.8 / 540.9 |

(The spill column gives the in-RAM pending.)

- **`D` grows linearly in `N`.** It is 6–8% above `8.39 M rows × encoded bytes per row`.
- **The budget changes nothing.** `B = 0`, `1` and 64 MiB give identical peaks, except `N = 1` at
  64 MiB (+1.1 MB).
- **The pre-mortem's numbers reproduce exactly:** 73/74, 287/287 and 1,143/1,143 MB.

`mixed` (`utf8`, `f64`, nullable `i64` and `utf8`, `bool`), pending / heap:

| Rows | encoded | stock | `gate:0` = `gate:1` = `gate:64 MiB` | spill 16 MiB |
| ---: | ---: | ---: | ---: | ---: |
| 16.8 M | 354.1 | 340.7 / 394.9 | 188.8 / 224.7 | 17.8 / 54.8 |
| 67 M | 1,416.5 | 1,367.4 / 1,543.9 (1.09×) | 189.6 / 230.5 | 18.8 / 59.6 |

### 4.4 Options that bound `D` or `c · rows` (sweeps 2a and 2b)

- **Spill 16 MiB** (same bytes). Consumer shape, 268 M rows: heap 133.1 → 97.6, 1,493.7 MB spilled
  (73% of the file), wall 56.4 → 74.6 s (+32%). `wide:64`: heap 4,631.9 → 540.9, 8,310.3 MB spilled
  (97%), wall 73.2 → 161.7 s (+121%).
- **128 KiB coalescing block** (bytes change). Consumer shape, 268 M rows: pending 102.1 → 44.9, heap
  133.1 → 111.6, encoded +0.35%, wall +3%. `wide:64`: pending 808.5, heap 987.7, encoded +0.15%.
- **Row groups, 4 Mi rows** (bytes change; one group in flight). Consumer shape: heap 134.0 / 136.8 /
  147.7 at 67 M / 268 M / 1.07 G rows, pending 32.0 throughout, encoded +0.02–0.05%, wall +25–27%.
  - `wide:64` needs 4,319.5 MB at 4 Mi-row groups and 1,091.8 MB at 1 Mi-row groups: the group size
    must shrink as the width grows.
  - `mixed`: 307.7 / 308.0 MB at 16.8 M / 67 M rows.
- **Per-block encode at `BLOCK_ROWS`**: one 4 Mi-row table, stock / `gate:1` heap. The runs were local,
  pinned to 4 CPUs like sweep 2a (`taskset -c 0-3`, so `P` = 4), with each command and its exit 0 in
  `job/local-block-rows-p4.log`. Each block is its own Vortex file, so this is the per-worker peak
  whatever the total rows:

  | Shape | stock | `gate:1` |
  | --- | ---: | ---: |
  | consumer shape | 46.0 | 46.0 |
  | `mixed` | 106.3 | 41.8 |
  | `wide:4` | 135.0 | 37.2 |
  | `wide:16` | 527.0 | 126.5 |

  An unpinned run (`P` = 32, `job/local-block-rows.log`) gives 46.7, 52.2, 45.6 and 152.5 MB for F:
  `P` sets the queue depths in `K` and `ε`.

### 4.5 Concurrent writers (sweep 2b, per-writer counters)

Consumer shape at 67 M rows under `gate:1`, one thread and executor per writer:

| W | Heap | Peak pending per writer | Summed pending | Digest |
| ---: | ---: | ---: | ---: | --- |
| 1 | 122.6 | 102.7 | 102.7 | `2d3481d6…` |
| 2 | 237.5 | 102.7 | 165.8 | `2d3481d6…` |
| 4 | 469.4 | 102.7 | 270.0 | `2d3481d6…` |

- **Stock with W = 2:** 489.3 MB.
- **`wide:4` with W = 4** under `gate:1`: 1,182.4 MB, ≈ 4 × 298.5.
- **`mixed` with W = 4** under `gate:0`: 820.5 MB.
- **The per-writer counters are independent.** Each writer's peak equals the single-writer peak, and
  the gates do not see each other. Revision 1's process-wide counter could not show this.

### 4.6 Byte identity

Every input produced **exactly one SHA-256** across its byte-preserving runs in sweeps 1 and 2: stock,
`gate:{0, 1, 16 MiB, 64 MiB, 256 MiB}`, spill, `fixed:64`, `lowest:64 MiB`, 0/1/3 threads and W =
1/2/4.

| Input | Runs | Digest |
| --- | ---: | --- |
| pairs 16.8 M / 67 M / 268 M / 1.07 G | 10 / 12 / 10 / 1 | `150dc259…` / `2d3481d6…` / `e6c81bdc…` / `aaf98231…` |
| random 16.8 M / 67 M / 268 M / 1.07 G | 10 / 5 / 5 / 1 | `bef4f31f…` / `bb0380d1…` / `b03482be…` / `3be09197…` |
| seq 16.8 M / 67 M / 268 M | 10 / 5 / 5 | `a3f4980d…` / `8475d984…` / `137d9571…` |
| `wide:1` / `:4` / `:16` / `:64`, 16.8 M | 5 / 6 / 5 / 4 | `3c09d5bc…` / `7e7ff198…` / `9ad763d9…` / `d4fc1400…` |
| mixed 16.8 M / 67 M | 8 / 5 | `cb3dc3ae…` / `01cd792d…` |

The bytes-changing options produce other digests, as expected.

- **Row groups on a table of at most one group** (`job/local-identity.log`) keep the consumer shape's
  bytes at 2 Mi and 4 Mi rows. They change `mixed`'s.
- **`mixed` stock depends on the input chunking.** At 2 Mi rows, 65,536-row chunks give `5da6f5c0…`
  and one 2 Mi-row chunk gives `150418f9…`.

### 4.7 The per-block partition with a real input side (sweep 3)

Each block is 64 row-groups (4 Mi rows, `BLOCK_ROWS`) and its own Vortex file. A reader thread produces
the row-groups in order and hands each block to one of at most `W` encode workers, each a single-threaded
executor as in tessera. `BP_FEED` selects how a block's input reaches its encoder:

- **`lazy`**: the encoder generates its own rows. There is no reader, so this is the encode-only
  baseline, `p_block`.
- **`fill`**: the reader builds the whole block in memory, then waits for a free worker.
- **`channel:2`**: the reader takes a free worker first, then pushes the block's row-groups into a
  bounded channel of 2 row-groups, which the encoder pulls as a stream.
- **`disk`**: the reader writes the block's raw records to an unlinked temp file (24 B/row for the
  consumer shape, so 100.7 MB per block), then waits for a free worker. The worker reads the file back
  one row-group at a time.

Consumer shape, stock Vortex, 8 CPUs. Peak heap in MB; the parentheses give the time in seconds:

| Feed | `W` | 1 block | 2 blocks | 4 blocks | 16 blocks (67 M rows) |
| --- | ---: | ---: | ---: | ---: | ---: |
| `lazy` | 1 | 46.1 | 46.1 | 46.1 | 46.1 (18.0) |
| `lazy` | 2 | 46.1 | 90.8 | 91.9 | 91.3 (9.4) |
| `lazy` | 4 | 46.1 | 88.9 | 162.2 | 159.0 (5.1) |
| `fill` | 1 | 98.7 | 180.3 | 180.3 | 180.3 (14.5) |
| `fill` | 4 | 98.7 | 176.3 | 324.7 | 391.3 (4.8) |
| `channel:2` | 1 | 47.3 | 47.3 | 47.3 | 47.3 (14.8) |
| `channel:2` | 4 | 47.3 | 63.8 | 65.3 | 68.5 (12.1) |
| `disk` | 1 | 46.1 | 46.1 | 46.1 | 46.1 (16.9) |
| `disk` | 2 | 46.1 | 78.0 | 86.2 | 86.5 (9.5) |
| `disk` | 4 | 46.1 | 85.8 | 133.9 | 128.0 (14.9) |

- **The `min(W, n)` model holds as an upper bound.** Every `lazy` and `disk` peak is ≤
  `min(W, n) × 46.1`. The measured ratio is 0.69–1.00 (the highest is 91.9 against 92.2). One block is
  one block's peak whatever `W` is, and `W` workers reach up to `W` blocks.
- **`fill` adds the input side.** At `W = 1` the peak is 180.3 MB against 46.1: one block is being
  encoded and the next is built in memory. A lone filled block measures 98.7 − 46.1 = 52.6 MB above the
  encode alone. At `W = 4` it is 391.3 MB.
- **`disk` adds nothing to RAM.** At `W = 1` the peak is 46.1 MB, the same as `lazy`. The staged bytes
  on disk peak at 201.3, 302.0 and 402.7 MB for `W` = 1, 2, 4: at most `W + 1` blocks, freed once a
  worker has read its block.
- **`channel:2` is the leanest in RAM, but it serializes the encoders.** The reader cannot start block
  `b + 1` until encoder `b` has taken all its rows, so `W = 4` is barely faster than `W = 1` (12.1 vs
  14.8 s). `fill` gets 3.0× from the same workers.
- **The harness's disk reader is the bottleneck.** In `disk` mode one thread generates and serializes
  every record, so `W = 4` gains little time (16.9 → 14.9 s). The memory numbers are valid, but this
  harness doesn't measure disk staging's throughput. The consumer reproducer must (§6.4).

**Size independence with the input side** (`disk`, `W = 4`): 128.0, 142.0 and 143.1 MB at 67 M,
268 M and 1.07 G rows (16, 64 and 256 blocks; 14.9, 45.4 and 205.4 s). Staged disk is 402.7 MB in each
case.

**Other shapes**, `W = 4`, 16 blocks (stock; `lazy` under F):

| Shape | `lazy` | `fill` | `channel:2` | `lazy`, F (`gate:1`) | `4 × p_block` (§4.4, stock / F) |
| --- | ---: | ---: | ---: | ---: | --- |
| `mixed` | 408.5 | 516.3 | 134.8 | 159.5 | 425.2 / 167.2 |
| `wide:4` | 538.7 | 708.1 | 170.7 | 153.2 | 540.0 / 148.8 |

`wide:4` under F is 3% above `4 × 37.2`, because §4.4's `p_block` was measured at `P` = 4 and this run
has `P` = 8.

Under F, the consumer shape with `disk` staging is 48.8 MB at `W = 1` and 138.3 MB at `W = 4` (16 blocks).

**Bytes.** Every input gave one ordered list of block digests across all feeds, `W` values and F. Block 0
of the consumer shape is `6f624a77…` in every run, the same as a plain single-block write of its 4 Mi
rows. The block-list digests are 8 inputs, one each; they are in `job/sweep-3.log`.

## 5. Options that bound `D` and `c · rows`

| Option | Same bytes? | Bounds | Measured cost | Where |
| --- | --- | --- | --- | --- |
| S. Spill pending above `S` | yes | `D` → `S` | +32% wall (pairs), +121% (`wide:64`) | Vortex |
| G. Smaller coalescing block | no | `D` ∝ block | +0.15–0.35% size, more segments | Vortex |
| Z. Incremental zone flush | no | half of `c` | not prototyped | Vortex |
| R. Top-level row groups | no | `D`, zones | +25–27% wall, +0.05% size | Vortex |
| P. `BLOCK_ROWS` blocks | per block, yes | `D`, all of `c` | `N` blocks, disk staging; below | tessera |
| P-logical (§5.1) | as P | as P | as P, plus a SPEC/ADR naming change | tessera |

- **S. Spill** (revision 1's alternative D). It keeps the bytes and takes the in-RAM pending to `S`
  whatever the width.
  - It does not touch `K(width)` (541 MB at `wide:64`) or `c · rows`, so it does not meet #677 alone.
  - It writes most of the file twice: 73–97% of it was spilled at `S = 16 MiB`.
  - It needs a scratch-dir knob in every writer, and the flat writer must drop the compressed chunk
    before parking.
  - It is the right tool for one thing: a very wide schema on a small-RAM worker.
- **G. A smaller coalescing block.** It moves bytes (the 1 MiB block was chosen for S3 reads,
  `strategy.rs:253-259`), and the read cost wasn't measured. It shrinks `D` 2.3–5.6×, but leaves `c` and
  still grows with the width. **Not recommended.**
- **Z. Incremental zone flush.** The zone table is one segment per column under the column's `eof` id
  (`zoned/writer.rs:166-172`).
  - Flushing it in parts helps only if the parts get ids between the data segments, which changes the
    bytes and the zone layout.
  - The A/B bounds what it can remove: 0.029 B/row, half of `c`. The other half stays.
  - **Not recommended alone.**
- **R. Top-level row groups** (Parquet-style).
  - **How.** The table is cut into groups of `R` rows (a chunked layout of tables), each group writes its
    own zone tables right after its data (a group-local `eof`), and one group is in flight. This needs
    no F: a group needs no later input, so a cap of 1 cannot deadlock.
  - **What it bounds.** `D ≤ R × Σ bytes/row`, the zone stats, and the input staging (`R` rows). It
    leaves 0.014 B/row of footer state.
  - **Gotchas found in the prototype.**
    - With the stock shared `eof` it deadlocks (a group's zone tables wait for all later data).
    - With an unbounded fan-out it pulls the whole input before encoding (`buffered` fills its queue
      from an always-ready stream).
    - It changes bytes, except for some tables of at most one group (§4.6).
  - **Fit.** It is the right upstream answer to #10046, where the maintainer points to Parquet's
    `ROW_GROUP_SIZE`, which Vortex lacks. For tessera it is a format change: a new canonical encoding
    of large table blocks.
- **P. Tessera's own bounded blocks.** `BLOCK_ROWS` = 4 Mi rows is already the format law
  (`table.rs:845-852`). It was chosen so that "one block fits comfortably in a worker's RAM", and is
  written by `TableMultiBlockSink` (`accumulate.rs:382`). Small tables stay single-block, byte-identical
  (`accumulate.rs:371-375`). Each block is its own Vortex file, so `c · rows` is capped at one block and
  the footer state goes with it.
  - **Cost: the product's shape.** A product larger than `BLOCK_ROWS` is `N` blocks named
    `pairs_0000`, … (the caller's prefix, `table.rs:890-896`), not one. #677 rejects exactly this
    ("suffixed physical blocks"), and the consumer chose `table_job_streaming` (`stream.rs:121`) to
    avoid it.
  - **Cost: writer output.** alpha.4 shipped `table_job_streaming` as one block of any size. Under P a
    streamed table above `BLOCK_ROWS` gets other bytes and another `content_hash`. Readers don't change.
  - **Cost: read before encode.** `TableMultiBlockSink` names its blocks only once it knows the total,
    so its workers start encoding after the whole input has been read and staged
    (`accumulate.rs:376-381`). Reading and encoding then don't overlap, and the whole input sits on
    disk first. `table_job_streaming` is told `spec.rows` up front, so a streaming partition knows its
    block count from the start and can dispatch each block as it fills. That is the variant track 1
    builds (§6.4). Its input is still staged to disk per block. Filling a block in memory instead was
    measured at 391.3 MB for `W = 4` (§4.7).
  - **Width.** Width stays a factor: the per-block heap is ≤ the block's encoded size + `K(width)`. It is
    documented as a limit, with S as the escape hatch if a consumer ever needs it.

### 5.1 P-logical: one logical table over bounded physical blocks (ADR-0063 names)

**The option.** Partition exactly as P does, but name the physical blocks with ADR-0063's hierarchical
grammar: `pairs/0000`, `pairs/0001`, … Under ADR-0063's leaf rule (`docs/adr/0063-block-name-grammar.md:45-51`,
checked by `check_leaf_rule`, `tessera-core/src/block/name.rs:166-181`), `pairs` is then a node of the
name tree and not a block. Readers resolve `pairs` to one logical table through the existing
`LogicalTableView`. A product holding `pairs/0000` and `pairs/0001` and no `pairs` block is already
valid for writers and readers today: `ProductBuilder::seal` (`tessera-core/src/product.rs:173`) and
the write session (`tessera-io/src/write.rs:783,1239-1257`) accept it. The leaf-rule test accepts the
same shape for `volume/0`, `volume/1` (`name.rs:406`).

Code references are at `origin/dev` `4b3ac42`, under `tessera/crates/`.

**Does it meet #677's literal need?** #677 asks for "one scan product with single named `pairs`,
`markers`, `ms_stats` table blocks and a configurable buffer bound independent of input size". It
rejects `TableMultiBlockSink` because it "changes the requested logical block into suffixed physical
blocks". The triage comment and ADR-0026 make it a design rule: "One logical block stays **one
canonical block**: no suffixed physical blocks" (`docs/adr/0026-streaming-table-ingest.md:190-191`).

- **The name:** yes. The consumer addresses `pairs`, and every reader that resolves logical tables
  answers `pairs`. The physical blocks are leaves under it, not renamed siblings.
- **The memory:** yes. It has exactly P's bounded memory (§6.2, §6.4): the partition is the same and
  only the names differ.
- **"One canonical block":** no, by construction. Taking P-logical means the owner replaces that design
  rule with "one **logical** table, any number of bounded physical blocks". Only R keeps one block.
- **What the consumer (MorePET/visionx-rs) would change.**
  - *Writer.* Its reproducer builds `pairs` by hand: `table::encode_streaming` plus
    `BlockRef { name: "pairs" }` (`crates/visionx-tsra/examples/stream_memory.rs` ~`:248-266`, branch
    `chore/39-tessera-677-repro`). ADR-0026 records that it moved to `table_job_streaming`
    (`0026:254-256`). Under P-logical it calls the new partitioning job API with the name `pairs` and
    lets tessera name the leaves. `markers` and `ms_stats` are small, so they stay single blocks.
  - *Readers.* They change only if they read `pairs` through a single-block API (below). The
    consumer's ADR-0001 needs a row-aligned `pair_group` (`:489`), DataFusion views over
    `pairs ⋈ pair_group` (`:552`, waiting on tessera's streaming `TableProvider`, `:561-563`), and
    chunk-stats subset reads (`:453`). All three are defined over a logical table in ADR-0062.
    No consumer read code calls `logical_table` yet.

**Read paths that resolve a name.** One function decides grouping: `Reader::block_group`
(`tessera-io/src/container.rs:438-461`). It matches the bare `prefix` or `prefix_` + four digits. It
does **not** match `pairs/0000`, so today `read pairs`, `sql … pairs` and `logical_column("pairs")` would
fail with "no blocks for prefix" (`tessera-io/src/multiblock.rs:378-381`).

| Path | Today | Under P-logical |
| --- | --- | --- |
| `Reader::block_group` | `prefix`, `prefix_NNNN` | also `prefix/NNNN` (the one core change) |
| `LogicalTableView` (`multiblock.rs:376-416`) | via `block_group`; checks columns, dtypes | works with it |
| CLI `read` (`tessera-cli/src/nav.rs:1160,1196`) | `logical_table(block)` | works |
| CLI `sql` (`main.rs:2015`, `sql.rs:131,161`) | `logical_table` → DataFusion `MemTable` | works |
| Py `read_logical_table_column` (`tessera-py/src/lib.rs:159-165`) | logical | works |
| CLI `ls <node>` (`nav.rs:1048`) | exact block name | **needs work** (list the node) |
| Py `read_block`/`read_table`/`read_table_column` (`lib.rs:71,116,139`) | single block | **needs work** |
| Py `column`, `table*` (`tessera-py/python/tessera/__init__.py:106-127`) | single block | **needs work** |
| CLI `tree` (`nav.rs:467,514`) | lists blocks | shows `pairs` as a node, no change |
| ADR-0062 scan, `aligned_to`, positional zip | **not built** (ADR Proposed) | designed on `LogicalTableView` |
| WASM reader | no logical reads | **needs work** if used |

**`.cidx`, pruning and zip.**

- **`.cidx`** is per block (`<block>.cidx`, `tessera-io/src/chunk_index.rs:21-23`). `pairs/0000.cidx` is a
  sibling leaf, so it is grammar-valid and leaf-rule-clean.
- **Pruning across blocks** already works for `prefix_NNNN`. `select_blocks_overlapping`
  (`multiblock.rs:225-240`) iterates the group, and `block_stats` (`:271-323`) uses each block's
  `.cidx` (`:290-297`). It needs nothing beyond `block_group`.
- **Zip** (ADR-0062 `aligned_to`, `docs/adr/0062-streaming-table-provider.md:579-611`) is defined over a
  logical prefix group: `RowAlignment.block` is "the source table's block prefix" and `source_digest`
  covers the group's block digests. A `pairs` group under either naming fits it. It is unbuilt either
  way.

**Is it a format change, and which ADR?** Yes.

- SPEC §5b defines the partition names as `<prefix>_0000` … (`tessera/docs/SPEC.md:165-166`), and the
  logical table as "the ordered sequence of `prefix` / `prefix_NNNN` blocks" (`:191-198`). P-logical
  rewords both to "a name prefix with numbered leaves", keeping the small-stays-single rule (one block
  keeps the bare name, `table.rs:890-896`).
- No ADR defines the `_NNNN` string today. It lives only in SPEC §5b and `table.rs:885-896`, and
  `multiblock.rs:1` cites "ADR-0026 §4", which is about `row_index`.
- So it needs a **new ADR**: "logical tables over bounded physical blocks". It amends:
  - ADR-0026's "one canonical block" rule (`:190-191`);
  - ADR-0063, to say what a numbered-leaf node means (§2 says only that blocks are leaves);
  - SPEC §5b.
- **Readers must keep accepting `prefix_NNNN`** for every product already written (GE ingest's
  `events_NNNN`, `tessera-ingest/src/ge_hdf5.rs:976,1007`). Writers switch.
- **Multi-block products written after the switch get new block names**, so a new `content_hash` for the
  same rows. Single-block products don't move. The corpus's multi-block goldens move, or are pinned to
  the old writer.

### 5.2 P, P-logical and R side by side

| | P (`pairs_NNNN`) | P-logical (`pairs/NNNN`) | R (Vortex row groups) |
| --- | --- | --- | --- |
| Bounded RSS, every term | yes (§6.2: `min(W, n) ×` block + input) | same as P | no: `0.014 B/row` footer term remains |
| Consumer addresses `pairs` | via `block_group`'s `_NNNN` match | via a node of the name tree | one real block |
| "One canonical block" (#677 triage) | no | no | yes |
| Unambiguous grouping | no: a user block `pairs_0003` joins the group | yes: leaf rule, a subtree | n/a |
| Exploded dir / S3 layout | flat siblings | `blocks/pairs/` directory | one object |
| Parallel encode | `W` blocks at once | same | one writer |
| Format change | names unchanged; streamed output moves | SPEC §5b + ADR; names move | large blocks' payload |
| Vortex change | none | none | fork patch, then upstream |
| Read-path work | `ls`, Python single-block calls, WASM | the same, plus `block_group` | none |

**Recommendation: P-logical, decided by an ADR that is the first implementation PR.**

- **It meets the consumer's two needs: one name, `pairs`, and a buffer bound independent of input
  size.** Its memory is P's (§4.7, §6.2), with every term measured. It does not meet the triage's
  "one canonical block", which is why the owner decides (next bullet).
- **It fixes P's naming weakness.** `_NNNN` is a convention inside a flat name space, so a user's own
  `pairs_0003` block silently joins the logical table. ADR-0063 has already given tessera a name tree
  in which a numbered-leaf node *is* the grouping. The pyramid levels `volume/0`, `volume/1`, …
  (ADR-0063 §3) are the same shape.
- **It needs the owner to retire one design rule.** #677's triage and ADR-0026 say "one logical block
  stays one canonical block". P-logical keeps the logical block and gives up the canonical-block half.
  If that half is a hard requirement, the answer is R. R changes Vortex and tessera's canonical bytes
  for large tables, needs a Vortex patch the maintainers have not agreed to (§6.3), encodes on one
  writer, and still grows by 0.014 B/row (34 MB at the consumer's 2.4 G rows).
- **F is a separate fix.** It lowers the per-block constant up to 4.2× (`wide:16`: 527.0 → 126.5 MB)
  and fixes the writer for everyone who can't partition (upstream). It is deadlock-free and
  byte-identical. For #677's own shape it changes nothing inside a block, so #677 does not wait for it.

**R remains the right mechanism upstream** for single-file writers that can't partition. It is proposed in
the maintainer conversation (§6.3), not built for tessera.

## 6. Plan for the implementation round

### 6.1 The Vortex patch (upstream first)

Upstream `develop` already has a writer-scoped `BufferedBytesTracker` (#8961,
`vortex-layout/src/strategy.rs:20-67`), carried by `LayoutWriterContext` (`:69-79`). It is exposed as
`VortexWriteOptions::buffered_bytes_tracker()` (`vortex-file/src/writer.rs:147`). `BufferedStrategy`
(`layouts/buffered.rs:63`) and the indexed layout writer (`layouts/indexed/writer.rs:357`) reserve into
it today. **The patch extends that tracker
rather than adding a parallel counter:**

- `vortex-layout/src/sequence.rs`:
  - `SequenceUniverse` gains the `granted` id, the observers and `starved()`.
  - A `SequenceProgress` handle from a `SequencePointer` (`is_starved()`, `observe(&Waker)`) is the
    documented back-pressure hook.
  - `remove` and a cancelled `WaitSequenceFuture` wake the observers.
  - The lock-contention change #10867 (2026-10-27) must keep its fast path: observers are touched only
    when the list is non-empty.
- `vortex-file/src/segments/writer.rs`: `BufferedSegmentSink::write` takes a
  `BufferedBytesReservation` on the write's tracker for the segment's bytes, before `collapse`. It is an
  RAII value declared after the `SequenceId`, so it drops before the id on every path. A second
  tracker in the same context keeps "pending in the sink" apart from "buffered in strategies", so the
  gate reads only the sink's share.
- `vortex-file/src/writer.rs`:
  - `VortexWriteOptions::with_max_pending_bytes(u64)` and the gate in `write_internal`.
  - `WriteSummary::peak_pending_bytes()`.
- **Not on the strategy tree.** At `608d8a1` `CollectStrategy::buffered_bytes` is `todo!()`
  (`collect.rs:75-76`). The root `TableStrategy` doesn't override the trait's default `0`
  (`strategy.rs:63`), so `Writer::buffered_bytes` (`writer.rs:363-364`) returns 0 today rather than
  panicking. Anything that made the tree recurse into the validity child would panic for nullable
  structs. So the fork backport counts in the sink, as upstream's tracker does, and never sums the tree.
  `develop` removed the method from the trait (#8961).
- **The flat writer.** It drops the compressed chunk before `segment_sink.write` (`flat/writer.rs:174`),
  so the sink owns the only reference to its buffers. This is needed for any spilling, harmless
  otherwise, and changes no bytes.
- **Docs.** Rustdoc on the knob states the rule, the full bound `B + D(width) + ε + K(width) + c · rows`
  with each term, that bytes never change, and that `B` cannot go below `D`.

### 6.2 Tests

In Vortex, upstream:

1. **Deadlock regression.**
   - Runs `B ∈ {0, 1}` on the multi-column shape with a `bool` column, and on the `mixed` shape.
   - Driven by a hand-polled single-thread executor that reports idle-while-pending as a failure
     (no timeout).
   - Shown to fail once without the starvation override (mutation: drop `|| starved()`).
2. **Byte-identity matrix** (§3), including 2 concurrent writers.
3. **Bound assertions**, each modelling its own term:
   - **Accounting:** `peak_pending_bytes ≤ B + ε` with `ε = 4 MiB + P × 1 MiB` on a single random
     column, where `D = 0`.
   - **Order debt:** on the multi-column shape, `peak_pending_bytes ≤ B + D̂ + ε`, with `D̂` computed by
     the §1.2 formula times a per-shape allowance, stated with the shape: 1.25× for `wide` (measured
     1.06–1.08×) and 2× for the consumer shape (measured 1.60×: 102.7 MB against the formula's 64 MB,
     because its `Dict` and `Buffered` stages hold more than the formula counts). The implementation
     round should replace the consumer allowance with a model of those stages if it can.
   - **Rows:** the real heap from a counting allocator in its own test binary, at `N` and `4N` rows. The
     test asserts `peak(4N) − peak(N) ≤ c_max × 3N + slack`, with `c_max` = 0.1 B/row and the slack
     stated. Stock grows by 3.0 B/row, 30× that limit.

In tessera:

1. `tests/staged_job_memory.rs` gets a **per-block** assertion (track 1). With `W` encode workers up to
   `W` blocks encode at once, so the peak is not one block's. The test asserts the measured model (§4.7):

   ```text
   peak(n blocks, W)  ≤  min(W, n) × p_block + s
   ```

   - `p_block` is the peak of a one-block job in the same test binary.
   - `s` is the staging slack, one row-group of records plus the reader's buffers, stated in the test.
   - The model is an upper bound, not a prediction. Fewer than `W` blocks may overlap, so the peak
     still rises with `n` until `n` is a few times `W`.
   - The test runs `W = 2` at `n = 2W = 4` and `n = 8W = 16` blocks. It asserts the bound at both, and
     `peak(16) ≤ peak(4) + s`.
   - Measured with `disk` staging: 86.2 and 86.5 MB at `n` = 4 and 16, against `2 × 46.1 = 92.2`. At
     `n = 2` the peak is 78.0 MB, which is why the test doesn't compare against `n = W`.
   - A `W = 1` case pins the single-block peak: 46.1 MB at 1, 2, 4 and 16 blocks.
   - That is the size independence #677 requires. It holds because the partition removes the linear
     term and the staging is on disk.
2. A single canonical block (`table_job_streaming`, if it remains) asserts the modelled linear bound
   `c_max × rows`, not "4× within fixed slack".
3. The corpus goldens. Under P none move. Under P-logical the multi-block goldens get the new names
   (§5.1), or stay pinned to the old writer as reader-compatibility fixtures. Single-block goldens
   never move.

### 6.3 Upstream: a maintainer conversation first

No code goes upstream before a maintainer agrees on the shape. The **owner** posts this on
vortex-data/vortex#10046, where a maintainer said Vortex has no mechanism to bound write-path memory:

```markdown
We have been measuring where write-path memory goes in the default strategy
(single-threaded executor, deterministic compressor). Three terms, each
confirmed with a counting allocator:

1. The segment sink is starved. `ChunkedLayoutStrategy` polls chunk writes with
   `buffered(usize::MAX)` and nothing applies back-pressure, so encoded segments
   pile up ahead of the sink: a single random u64 column holds 0.93x its encoded
   size at the peak.
2. Order debt. Segments are written in one global `SequenceId` order, so a
   slow-coalescing column (a bool needs ~8.4M rows per 1 MiB block) holds every
   other column's later segments: about 8.4M rows x the table's encoded bytes
   per row (1.1 GB for 1 bool + 16 u64 columns), whatever the budget.
3. Zone statistics and per-chunk state grow with the row count until the end of
   the write (~0.06 B/row for a 12-column table).

A fixed cap on the chunk fan-out deadlocks (`buffered(2)` and `buffered(8)`
reproduce it), and so does exempting "the chunk with the lowest pending id".
What works: gate the writer's input on pending sink bytes, with an override that
admits input whenever the lowest live `SequenceId` is not parked in `collapse`.
It is deadlock-free for every budget (88 runs, including B=0), never changes a
byte, and removes term 1. We'd extend the existing `BufferedBytesTracker` for it
rather than add a counter. Terms 2 and 3 need bounded row groups (a chunked
layout of tables with group-local eof, one group in flight). That is the
`ROW_GROUP_SIZE` you mentioned, and it changes bytes, so it would be opt-in.

Would you take (a) the input gate with a `with_max_pending_bytes` option, and
(b) an opt-in row-group writer option? We have a prototype and measurements, and
would open (a) first.
```

Upstream needs DCO with author `Lars Gerchow <lars.gerchow@gmail.com>` and an AI-use disclosure. The
implementation round prepares the branch and the PR text; **the owner signs and submits**.

The relation to #9423 must be stated in the PR. That is gatesn's push-API rework, closed unmerged by the
stale bot on 2026-09-13. It kept capacity-one mailboxes ("backpressure matches the previous writer's
memory model") and the same `SequenceId` order, so it addressed neither the order debt nor the
starvation. F does not conflict with it, and a revived push API would take F's gate at its root input.

### 6.4 Tessera

**Track 1 (the memory half of #677, no Vortex change; the naming half is open question 1):**

1. **Bounded blocks for streamed tables** (if open question 1 goes that way).
   - `table_job_streaming` already gets `spec.rows` (`stream.rs:128-136`), so it knows the block count
     up front. It splits its lazy row-groups into `BLOCK_ROWS` blocks, with the same partition and bytes
     as `TableMultiBlockSink`. The names follow open question 1: `pairs/NNNN` under P-logical,
     `pairs_NNNN` under P.
   - **The input side is staged to disk, per block.** The reader writes each block's row-groups as it
     fills them. When the block is complete, it dispatches the block as a staged job that reads its
     input back one row-group at a time.
     - This is the mechanism `TableMultiBlockSink` already uses: fragment files, `accumulate.rs:485-528`.
       The difference is that a block is dispatched as soon as it is full, not at `finish`
       (`accumulate.rs:550-585`). That works because the block count is known up front.
     - Measured: no RAM term (46.1 MB at `W = 1`, the same as the encode alone), and at most `W + 1`
       blocks of raw input on disk (§4.7).
   - **Rejected input mechanisms.**
     - *Filling a block in memory* holds up to `W + 1` blocks of arrays: 391.3 MB at `W = 4` for the
       consumer shape.
     - *A bounded channel per block* is lean (68.5 MB at `W = 4`), but it serializes the encoders: the
       reader can't move on until the current block's encoder has taken every row.
   - Small tables (≤ `BLOCK_ROWS`) stay one block, byte-identical.
   - Its "any size" contract (`stream.rs:121`, shipped in alpha.4) is replaced. Streamed tables above
     `BLOCK_ROWS` get other bytes and another `content_hash` than alpha.4 wrote; readers are unchanged.
   - **New disk cost:** the input is written once more, at most `(W + 1) × BLOCK_ROWS × record bytes`
     at a time. That is 402.7 MB at `W = 4` for the consumer's 24 B/row, under the session's
     `staging/` directory.
2. **ADR-0026.** The "single-table output sink" amendment's *Known limit* paragraph is replaced by the
   per-worker bound `B + D(width) + ε + K(width) + c · rows`, with each term and its measured size. It
   states that `rows ≤ BLOCK_ROWS` under the per-block partition, that `D` and `K` grow with width, and
   that `B` exists only once track 2 lands. `encode_streaming_to`'s rustdoc changes with it.
3. **ksb RSS proof** with the consumer reproducer (MorePET/visionx-rs PR #41, branch
   `chore/39-tessera-677-repro`), built against track 1's tessera at the current pin, at 268 M and
   1.6 G rows.
   - Job shape: `--cpu 4 --exclusive-cpus --mem 12Gi`, with peak `RssAnon` sampled every 10 ms.
   - Acceptance: the two `RssAnon` peaks agree within a stated slack, and both are within
     `min(W, n) × p_block + s`, with `p_block` measured as RSS by a one-block run of the same binary.
   - A throughput run compares disk staging with the bounded channel. Disk staging stays only if it is
     faster.
   - With open question 1 decided, that closes #677. The reproducer also measures the throughput of
     disk staging, which the harness could not (§4.7).

**Track 2 (F, after the #10046 conversation):**

1. **Pin bump.** The fork backport is a commit on top of `608d8a1` on `gerchowl/vortex`. All 27 vortex
   `rev =` lines in `tessera/Cargo.toml` move together, plus `Cargo.lock`. The #480 checklist gains a
   third patch to wait for upstream.
2. **The knob.** `encode_streaming_to` sets `with_max_pending_bytes` from
   `WriteConfig::encode_pending_bytes` (default 64 MiB). It does not belong in `StreamWindow`, which
   counts jobs. `StreamStats` gains the encode's peak pending bytes.
3. **The proof.** The per-block test (§6.2) tightens to F's per-block numbers. The corpus goldens must
   not move.

## 7. Open questions for the owner

1. **Retire "one logical block stays one canonical block" (#677 triage, ADR-0026 `:190-191`)?**
   *Recommendation:* yes, in favour of one **logical** table over bounded physical blocks (P-logical,
   §5.1).
   - The consumer's `pairs` stays one name. Its physical blocks become `pairs/0000`, `pairs/0001`, …
     (under P they would be `pairs_0000`, …, since the name is the caller's prefix,
     `table.rs:890-896`).
   - It needs no Vortex change, and its memory bound is measured term by term (§6.2).
   - Keeping one canonical block needs R: a change to tessera's canonical bytes for large tables, a
     Vortex patch, and still 0.014 B/row of growth.
2. **Tessera's default budget.** *Recommendation:* `WriteConfig::encode_pending_bytes`, default 64 MiB,
   per encode worker. Below `D` it doesn't bind, and above ~64 MiB it only buys peak RAM.
3. **Upstream default: on or opt-in?** *Recommendation:* propose a 64 MiB default for the gate. It never
   changes a byte, and the unbounded default is the defect. Fall back to opt-in if upstream asks. Row
   groups are opt-in, since they change bytes.
4. **Width.** *Recommendation:* document the per-worker bound with `D(width)` and `K(width)` (§2) as a
   limit, and don't build spilling now. With bounded blocks a block's encode is at most its encoded
   size plus `K(width)` (527.0 MB stock and 126.5 MB with F for `wide:16`, §4.4). Revisit if
   a consumer's schema makes one block exceed its budget.
5. **Carry F on the fork before upstream merges?** *Recommendation:* yes, as a third patch tracked
   in #480, after the #10046 conversation shows upstream agrees with the shape, so the fork does not
   diverge from what upstream will take. #677 doesn't depend on it (track 1).

## 8. Pre-mortem findings and where they are answered

| Finding (pre-mortem of revision 1 at `b40b708`) | Answer |
| --- | --- |
| 1. `D` scales with width; the budget can't push it down | Confirmed: §1.2, §4.3; bound in §2 |
| 2. The peak grows with rows, at the end; zone stats? | Confirmed: §1.4, §4.2; zone stats = half |
| 3. Sweep soundness: narrow shapes, process-wide counter | Fixed: wide/`mixed`/`B = 0`; W = 1–4 (§4) |
| 4. Upstream: #10046, #9423, the tracker, `todo!()` | §6.1 (tracker, `todo!()`), §6.3 (#10046, #9423) |

Re-review of revision 2 at `1f0e3ff` (blockers on the recommendation):

| Blocker | Answer |
| --- | --- |
| 1. "P meets #677" contradicts its rejection of suffixed blocks; `pairs_NNNN` | Decision needed, §5.1, Q1 |
| 2. P's bound leaves out the input side | Disk staging chosen and measured, §6.4; the bound in §6.2 |
| 3. With W ≥ 2 the per-block test is mis-modelled | `min(W, n) ×` per block, measured, §6.2 |
| 4. alpha.4 shipped "one block of any size"; "no format change" is readers-only | Decision needed, §6.4 |
| P-logical (ADR-0063 names) | §5.1, §5.2, with `file:line` evidence |

Withdrawn claims from revision 2:

- "P meets #677". It meets the bounded-memory half and not #677's "no suffixed physical blocks". P-logical
  keeps the name and needs the owner to retire the canonical-block rule.
- "No staged input" for the per-block stream. A block that fills in memory holds its raw rows; the
  measured choice is disk staging (§6.4).
- "No format change". True for readers only. Writers of streamed tables above `BLOCK_ROWS` produce other
  bytes than alpha.4 did.

Withdrawn claims from revision 1:

- "The peak no longer grows with the table". It does, by `c · rows`.
- "≈ 100 MB, accept it". That holds for one 12-column shape only; `D` grows with width.
- "#677 can close on the ksb proof of F". It cannot. Bounded rows close it, with or without F: §5,
  open question 1.
