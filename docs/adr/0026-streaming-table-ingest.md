# ADR-0026 — Streaming, chunked table writes for >RAM ingest

Status: **Proposed** (2026-06-26) · Extends ADR-0024 (table payload), ADR-0025 (ingest), relates to
S17 (`WriteSession`). Blocks the >RAM half of `#208` (GE listmode `events_2p` ≈ 10⁸ rows ≈ 5–7 GB). Amended by
ADR-0064 (2026-10-30): tables above `BLOCK_ROWS` are numbered leaves of one logical table.

## Context
`tessera_io::table::encode` builds **one** Vortex `StructArray` from full in-memory column `Vec`s and
writes it as a single block payload. `WriteSession::append_block` streams at *block* granularity (it
appends whole encoded blocks with hash-on-write + journal), but the table block itself is still
materialised whole. So ingesting a real GE listmode acquisition — `read_events_2p` reads the entire
compound dataset into RAM (`read_raw::<Rec2p>()`), then encodes one blob — needs ~2× the file in RAM
(7 GB file → ~14 GB peak). That is the one functional ceiling left in the ingest path; everything
smaller works today.

Probe (vortex-file 0.75): the writer already consumes a **stream** of array chunks —
`VortexWriteOptions::default().write(&mut buf, array.to_array_stream())` — and emits a
`ChunkedLayout` (Parquet-style row groups: outer chunked layout = row groups, inner = pages,
`src/lib.rs:83`). A `ChunkedLayoutStrategy` (`strategy.rs:230`) is the default. So feeding an
`ArrayStream` of fixed-size row-batches instead of one giant `StructArray` is the supported path, and
HDF5 hyperslab reads (`dataset.read_slice_1d(start..end)`) supply the batches without loading the
whole dataset.

## The load-bearing problem: determinism
Content-addressing requires **byte-identical** output for identical input (the writer-determinism
release gate; ADR-0020). A chunked, multi-row-group layout does **not** produce the same bytes as the
current single-`StructArray` encode — chunk boundaries become part of the physical layout, and
per-chunk compression decisions (the BtrBlocks scheme search, already a source of the ALP
nondeterminism excluded in ADR-0024) are taken per chunk. Naively adding a streaming path would mean
*two* encoders that disagree on bytes for the same table, silently forking identity.

## Decision (proposed — requires the validation below before Accepted)
1. **One encoder, always chunked.** Make `table::encode` *always* write through a fixed-size
   row-group `ArrayStream` (proposed group size: **2¹⁶ = 65 536 rows**, a power-of-two constant in
   the spec), so the batch path and the streaming path are the *same* code producing the *same*
   bytes. A table smaller than one group is a single-group file (≈ today's output, but via the
   chunked strategy).
2. **Deterministic strategy, ALP still excluded.** Pin the layout strategy explicitly (fixed
   row-group size, `exclude_schemes([ALP, ALPRD])` per ADR-0024, buffering/segment knobs fixed) so
   the per-chunk scheme search can't reintroduce build-profile float nondeterminism.
3. **Streaming ingest reader.** `ge_hdf5::stream_events_2p/3p(path, dataset, sink)` reads the compound
   dataset in row-slabs (one row-group at a time), transposes each slab to columns, and feeds the
   sink — bounded RAM (one row-group, not the file). The sink is the chunked `table::encode` stream.
4. **`row_index` unchanged.** `ms` stays the O(1)-take index; the chunked layout's row-group offsets
   (`src/lib.rs:18`, "finding the chunks containing a row range") make range-reads *cheaper*, not
   harder.

## Consequences / validation (the gate to Accept)
- **The golden corpus changes.** Every committed `.tsra` table fixture re-encodes to new bytes →
  regenerate `corpus/files/*` + `corpus/corpus.json` hashes, and re-confirm the **independent
  Python reader** still reproduces them from SPEC.md (update SPEC §5b with the row-group framing).
- **Cross-env determinism must be re-validated**, not assumed: dev == release == hermetic for the
  chunked output, ideally x86 == ARM (the ADR-0024 caveat). This is the real risk and the reason this
  is an ADR, not an inline change — the per-chunk scheme search is exactly where determinism broke
  before.
- **SPEC bump.** Row-group size + framing become part of the format contract → a SPEC version note
  (readers must tolerate N row groups; they already do via Vortex's layout, but the spec must say so).
- **Net once validated:** a single deterministic table encoder, >RAM ingest at bounded memory, and
  cheaper row-range reads — closing the last functional gap in the ingest path. Until validated, the
  whole-file path stays correct for moderate files; `read_events_2p` documents the RAM ceiling.

## Revision (SSoT/DRY — ingest rides the DAQ write engine, not a second encoder)
The >RAM ingest problem is **the same surface as the streaming DAQ write** (`#203`: bounded RAM ring →
A/B/C fragment registry → rayon encode pool → compaction → seal). So the right design is NOT a separate
chunked table encoder bolted onto ingest — it is: **ingest is a throttled *producer* into the one
`WriteSession`.** Reading the HDF5 in row-slabs and pushing them through the streaming engine with
backpressure means we never hold the whole file, and DAQ capture + file ingest + batch authoring all go
through a **single write path** (SOLID: ingest depends on the `WriteSession` abstraction; DRY: one
encoder; SSoT: one place decides bytes).

Consequence: the "always-chunked encoder" becomes a property of the streaming engine's compaction, not
an ingest-specific fork. Batch encode = streaming with one fragment; >RAM ingest = streaming with N. The
determinism re-validation + golden regen still gate it — but there is now exactly **one** encoder to
validate, which is the whole point.

**Sequencing:** finish `#203` (streaming write engine — `WriteSession` currently appends whole blocks;
sub-block row-group compaction is the missing piece) FIRST; then ingest is a thin producer into it. Do
NOT build a standalone chunked table encoder.

## Status note
**As-built (2026-06-27):** §1 **always-chunked encoder DONE** — `tessera_io::table::encode` always slices
into fixed `ROWS_PER_GROUP = 2¹⁶` row-groups (`table.rs`), so the batch and streaming paths are the *same*
code producing the *same* bytes (`encode_streaming_matches_batch_encode`, `accumulator_equals_batch_over_
odd_batches`). §2 deterministic strategy (ALP excluded, fixed knobs) + §4 `row_index` are in that encoder.
The bounded-memory streaming engine + journal/recover/seal (`WriteSession`, `TableStreamWriter`) is built
- tested, incl. the ADR-0028 §5 live fold (`with_live_index`).

**§3 streaming HDF5 ingest reader — DONE (2026-06-27):** `ge_hdf5::stream_events_2p` reads the compound
dataset in HDF5 row-slabs (hyperslab `read_slice_1d(start..end)`) → `transpose_2p` → `TableStreamWriter`
→ seal; `stream_to_listmode_product_2p` is the bounded-memory >RAM ingest. Test
`streamed_2p_ingest_is_byte_identical_to_whole_file_read` proves a 5000-row synthetic `.h5` streamed in
non-aligned 999-row slabs seals to the **same `content_hash`** as the whole-file read (slab size never
changes the canonical bytes). So §1 always-chunked + §2 deterministic + §3 streaming reader + §4
`row_index` are all as-built.

**Real-scale measurement (2026-06-29, `tessera bench write --input` on real DUPLET GE `.h5`, no PHI):**
streaming ingest of `singles` (294 M rows), `coin_2p` (37.4 M), `events_2p` (9.4 M) seals without error
at 0.9–4.9 M events/s (single-thread encode). **The memory behaviour is now precisely characterised:
input-bounded, output-materialised.** HDF5 slab reads + row-group spill keep the *source file* off-RAM,
but `table::encode_streaming` returns the whole compressed Vortex block as one in-RAM `Vec<u8>`, so peak
RSS ≈ ~600 MiB baseline + the compressed block (flat 605→647 MiB for events_2p/coin_2p, **3.63 GiB for
singles**). So the "RAM ceiling" this ADR named is the **compressed output block**, not the input — a
file far larger than RAM ingests fine *iff its compressed product fits in RAM*. Evidence + table:
`tessera/docs/SPIKE-RESULTS.md` (#203 → DUPLET scale run).

**Constant-memory >RAM — DONE (2026-06-29, `c5a48a2`, multi-block decomposition / ADR-0034 §3).** A large
listmode acquisition is now a deterministic SEQUENCE of `BLOCK_ROWS = 2²²` table blocks, parallel-encoded
across `workers` and **streamed to disk** (the `WriteSession::seal` MUST-FIX: `pack_streaming` copies each
block fragment into the `.tsra` via `std::io::copy` instead of `fs::read`-ing all blocks into one
`Vec<BlockPayload>`). Re-measured DUPLET singles (294 M): **peak RAM 147 MiB @ 1 worker (was 3.63 GiB),
bounded by workers not dataset; throughput 4.9 M→14.9 M events/s (~3×), saturating ~8 workers**
(SPIKE-RESULTS #203 → "Multi-block result"). Determinism preserved: `content_hash` worker-count-independent
(ordered commit), small products (≤ BLOCK_ROWS) stay single-block byte-identical (no corpus regen).

**Remaining before an Accepted flip:** **cross-env / cross-arch determinism re-validation**
(dev==release==hermetic, x86==ARM — the ADR-0024 caveat; the x86+aarch64-linux CI matrix is wired, needs a
green ARM run). The functional + memory + parallelism gates are now met.

## Amendment (#677, 2026-10-25) — StreamWriter memory contract

**Defect.** The `StreamWriter` bounded its *input* channel, but its committer drained the output channel
into an unbounded `BTreeMap` reorder buffer. So the channel capacity did not bound the encoded payloads it
held: if job 0 was slow while jobs 1..N finished, the committer kept all N payloads while the producer kept
pushing. The memory bound this ADR depends on did not hold under reordering. The fix doesn't change any
bytes or hashes, so it is not a format change and needs no new ADR.

**Contract: a credit window** (`tessera_io::StreamWindow`). A job takes a credit when it is **pushed** and
gives it back only when it is **committed in order**. So one window covers every job the pipeline holds:
queued, encoding, and finished but waiting in the reorder buffer. `push` blocks while the window is full.

The knobs:

- `max_in_flight` (`J`): jobs pushed but not yet committed. Defaults: `StreamWriter::new(_, W, capacity)`
  uses `capacity + W`; `StreamWriter::with_config` uses `ring_depth(unit_bytes) + W`.
- `max_in_flight_bytes` (`B`): encoded payload bytes (data + companions) held in RAM. Defaults: `new` uses
  `DEFAULT_RAM_BUDGET` (1 GiB); `with_config` uses the config's `ram_budget`.

With `W` encode workers and `P` the largest single job's encoded payload:

- jobs in flight `≤ J`, always;
- encoded bytes held `< B + min(W, J) × P` (and trivially `≤ J × P`).

Through `with_config`, `ram_budget` is spent on both sides: the job window holds about `ram_budget +
W × unit_bytes` of raw input and the byte budget about `ram_budget + W × P` of encoded payloads, so the
worst case is about `2 × ram_budget + W × (unit_bytes + P)` plus the encoders' working sets.

The `+ W` in the default job window is the queue capacity plus one job per worker. A window of only the
queue capacity would leave workers idle whenever `W > capacity`, which loses today's throughput.

**Enforcement.** Two gates share one mutex and condvar. (1) *Submit*: a job is admitted while fewer than
`J` jobs are in flight **and** fewer than `B` bytes are held. (2) *Encode start*: a worker starts a job only
while fewer than `B` bytes are held, **except** for the job the committer waits on, which always runs. Gate
2 is what makes `B` bind: once `B` is reached, only the encodes already in progress can add bytes.

**Deadlock rule.** The window must never block the job the committer waits on (`next`). `next` has been
pushed, the queue is FIFO so it is dequeued first, and gate 2 exempts it. A job whose payload alone
exceeds `B` is admitted into an empty window (no bytes are held then) and runs alone once it is `next`. A
failed encode, a failed commit or a panicking worker *closes* the window: `push` returns an error and the
workers waiting at gate 2 exit (idle workers exit when `finish` closes the job queue), so nothing hangs.
The committer also errors if any pushed job is missing at the end, instead of sealing a truncated product.

**Determinism.** Commit order is still `seq`. Bytes and `content_hash` are identical across worker counts
and window sizes (`stream::tests::content_hash_is_identical_across_workers_and_windows`).

**Instrumentation.** `StreamWriter::stats()` (live) and `StreamWriter::finish_with_stats()` (final) return
`StreamStats` with `peak_in_flight_jobs`, `peak_in_flight_bytes`, `max_job_bytes` (`P`), `jobs_pushed`,
`jobs_committed` and `producer_stalls`, so a consumer can assert the bound.

**Scope.** `P` is one job's encoded payload in RAM. The second half of #677 (below) moves a table
block's encoded output to disk, so a single block larger than RAM is bounded too.

## Amendment (#677, 2026-10-26) — single-table output sink

**Defect.** `table::encode_streaming` pulled its input lazily but wrote the encoded table into a
`ByteBufferMut` and returned `freeze().to_vec()`, so the encoded output grew with the table (2 GiB of
input → 4.76 GB anonymous RSS at `9424f90`, the consumer reproducer on #677). `TableStreamWriter::finish`
and `table_job_from_fragments` reached that path, so neither the DAQ writer nor the multi-block sink could
produce one canonical block larger than RAM. The fix doesn't change any bytes or hashes: not a format
change, no new ADR.

**Contract.**

1. **File-backed encode.** `table::encode_streaming_to(spec, groups, sink)` writes the Vortex file to any
   `std::io::Write`. At the pinned Vortex (0.75, rev `608d8a1`) that is
   `VortexWriteOptions::blocking(&runtime).write<W: std::io::Write + Unpin>(sink, array_iterator)`
   (`vortex-file/src/writer.rs`), which writes each segment buffer as soon as it is available and has no
   buffering of its own. `encode_streaming` keeps its signature as `encode_streaming_to` into a `Vec`.
2. **Staged jobs.** A `StagedEncodeJob` (`StreamWriter::push_staged`) receives the session's
   `BlockStaging` and writes its data block into a `StagedBlock`, a file in the session's `staging/`
   directory. `table_job_streaming(name, spec, groups)` (a lazy row-group iterator) and
   `table_job_from_fragments` (staged fragments; used by `TableMultiBlockSink`) are staged jobs, and so is
   `TableStreamWriter::into_job`. One logical block stays **one canonical block**: no suffixed physical
   blocks. *(Retired for tables above `BLOCK_ROWS` by ADR-0064, 2026-10-30: see the amendment at the end.)*
3. **Commit by rename, verified.** A `StagedBlock` hashes its bytes as the encoder writes them, so the
   block's digest is born with the bytes (ADR-0002 first-moment integrity). `WriteSession::append_staged`
   takes that digest (a set `block_ref.digest` must match it, a `None` one is filled in), fsyncs the file,
   **renames** it into `blocks/` (no copy: `staging/` is inside the session directory, so on the same
   filesystem; if it is on another one anyway, the rename fails with `CrossesDevices` and the file is
   copied instead), fsyncs the directory, then **re-reads the fragment and checks it against the born
   digest** before the same journal line + Merkle leaf as `append_block`. Scratch corruption between the
   encode and the commit, or a bad copy, is refused (`Error::Integrity`, `what = "staged_block"`)
   instead of being sealed under the digest of the corrupted bytes. `append_block_from(block_ref,
   reader)` copies a reader into a `StagedBlock` and commits it the same way.
4. **RAII.** A staged file is deleted on a failed commit and on drop (an encode error, a panic, a
   pipeline shut down early); a committed one has become the fragment. Any append that fails before its
   journal write leaves no staged file, no fragment, no journal line and no Merkle leaf.
   `WriteSession::create` and `recover` clear `staging/`, since nothing in it was committed.
   `StreamWriter::finish` joins every thread before it reports an error, so no staged encode is still
   running when it returns (#680 item 3).
5. **A failed journal write poisons the session** (every append, through the one commit they share).
   Once the journal write has begun, a failure (the write, or its fsync) leaves the journal's state
   unknown: a torn line, or a complete line that may or may not be durable. The failed block's fragment
   is deleted, so `recover` stops at such a line, and any block appended behind it would be dropped. So
   the session refuses every later append and the seal, with an error that says to `recover`; `recover`
   rereads the journal and keeps exactly the blocks committed before the failure. Truncating the journal
   back instead was rejected: it would rest on the fsync that just failed (after an fsync error the kernel
   may drop the dirty pages and clear the error, so a later fsync can succeed with the data lost). A
   block name already committed is refused too, so a failed append can't delete a committed fragment.

**The rule: table streaming jobs are always staged.** The encoded size isn't known until the encode ends,
so a threshold would need a sink that starts in RAM and spills to disk; always staging keeps the RAM a
table job holds independent of its size, for one sequential read of the encoded block on local disk (the
commit's verify pass; the write is the fragment's own, since the staged file becomes the fragment).
`table_job` (in-memory input) and `EncodeJob`s stay in RAM, as before.

**The bound, extended.** With the window's `J`, `B`, `W` and `P` as above, `P` now counts only what a job
holds **in RAM**: a staged job's companions, i.e. `0` for the table jobs. So for a product written with
staged table jobs, the encoded bytes held in RAM are `< B` (`StreamStats::peak_in_flight_bytes`; test
`stream::tests::single_table_block_many_times_the_budget_stays_within_the_window`). What the window does
**not** count, per encode worker: one row-group of input, the compressor's buffers, a `BufWriter` (8 KiB),
and the encoded segments the Vortex writer holds before it writes them (below). The committer's verify
pass reads through a fixed 64 KiB buffer. That a staged job holds nothing beyond the encoder's own working
set is checked on the real heap, not only the window's accounting: `tests/staged_job_memory.rs` (a counting
allocator; each staged table job's peak heap is that of the same encode into `io::sink()`, within 1 MiB).

**Disk is not bounded by the RAM budget.** Scratch needed in the session directory, with `S` the largest
staged block (`StreamStats::max_staged_job_bytes`) and `T` the product's total size:

- staged files of finished-but-uncommitted jobs `≤ J × S` (`StreamStats::peak_staged_bytes`), plus the
  files of the up to `min(W, J)` encodes in progress;
- committed fragments (`blocks/`, up to `T`): a commit renames the staged file into place, so a block is
  never on disk twice;
- at seal, the output `.tsra` (another `T`, written to a sibling `.part` and renamed).

So plan for about `2 × T + J × S` of scratch when the output `.tsra` is on the same filesystem (`T + J × S`
until the seal). For a product that is one large block (`S ≈ T`) that is `T` while it encodes and commits,
and `2 × T` during the seal (the fragment and the output). A copying commit would add one `S` while a
block commits (the staged file and its copy), `2 × T` already during the commit of one large block.

**Determinism.** A staged block is byte-identical to the in-memory encode of the same table, so the
sealed bytes, `content_hash` and `manifest_hash` don't depend on the job kind, the worker count or the
window (`stream::tests::staged_table_jobs_seal_the_in_memory_product_for_every_worker_count_and_window`).
The conformance corpus is unchanged.

**Measured (consumer reproducer, ksb job, 2026-10-26).** MorePET/visionx-rs `chore/39-tessera-677-repro`
(exact nullable decoder shape, its single encode job switched to `table_job_streaming`), built against this
change (at `37fe9d7`, which committed by a 256 KiB-buffered copy instead of the rename: the encode, which
sets the peak, is the same), `ksb run --cpu 4 --exclusive-cpus --mem 12Gi`, `--workers $KSB_CPUS`, peak
`RssAnon` sampled every 10 ms (anonymous memory, not page cache):

| rows | packet-equivalent input | peak RssAnon | output | at `9424f90` |
|---:|---:|---:|---:|---:|
| 268,435,456 | 2.15 GB | 0.91 GB | 2.05 GB | 4.76 GB |
| 1,600,000,000 | 12.8 GB | 5.15 GB | 12.21 GB | — |

**Known limit: the Vortex writer's own buffering (#681).** The output Vec is gone, but RSS still grows
during the encode (~3.2 bytes/row, about 0.4 × the encoded size at the peak) and drops when the footer is
written. Root cause, measured with a counting allocator and a patched copy of the pinned Vortex: its
`ChunkedLayoutStrategy` spawns one write task per chunk and polls them with `buffered(usize::MAX)`, and the
segment sink writes segments in one global sequence order across all columns, so compressed chunks pile up
in pending tasks ahead of the sink with nothing applying backpressure. A fixed cap is not a drop-in fix:
`buffered(64)` bounds the heap, `buffered(2)` and `buffered(8)` deadlock on the cross-column order. So the
bound this amendment gives is the tessera side (the window and the staged block); the encode's RAM is
`≈ 0.4 × S` on top until #681 lands a deadlock-free bound inside Vortex. Projected for the consumer's full
2.4 G-row (19.2 GB) acquisition: ≈ 7.7 GB peak RssAnon, under its 8 GiB budget but not independent of the
input size, so #677 stays open on #681.

## Amended by ADR-0064 (2026-10-30) — streamed tables are partitioned into numbered leaves

[ADR-0064](0064-logical-tables-over-numbered-leaves.md) (Proposed) records the owner's decision on #677
(2026-10-29, option P-logical of the #681 design note). It changes three things in this ADR:

1. **"One logical block stays one canonical block" is retired for tables above `BLOCK_ROWS`.** Rule 2 of the
   #677 amendment above ("no suffixed physical blocks") no longer holds for them. A table of more than
   `BLOCK_ROWS` rows, streamed or not, is **one logical table over bounded physical blocks**: the leaves
   `<table>/0000`, `<table>/0001`, … (ADR-0064 §1), each carrying a sealed partition marker (§2), which every
   logical reader presents as one table (§3, §7). A table of at most `BLOCK_ROWS` rows stays one canonical block
   with the bare name, byte-identical to before.
2. **`table_job_streaming`'s "any size" contract (alpha.4) is withdrawn.** Above `BLOCK_ROWS` it fails with a
   typed error naming `StreamWriter::push_table`, which partitions, stages each block's input on disk and
   dispatches each block as it fills (ADR-0064 §5). At or below `BLOCK_ROWS` it is unchanged.
3. **The *Known limit* paragraph of the #677 amendment is superseded by ADR-0064 §8's bound** for `push_table`
   with `W` encode workers and `n` blocks:

   ```text
   peak heap     ≤  min(W, n) · p_block + s,   s = 2 · ROWS_PER_GROUP · r_mem + 4 MiB
   staged input  ≤  W + 1 blocks, the partial one included
   ```

   `p_block` is one block's measured encode peak (46.1 MB for the consumer shape at 8 CPUs, #681 note §4.7);
   it still holds the Vortex writer's width terms `D(width)` and `K(width)`, but no longer a term that grows with
   the table's rows. Size independence is asserted with a 5% plateau tolerance from `16 W` blocks on (§8). The
   staged input is page cache, which counts toward the cgroup's `memory.current` and is RAM on tmpfs, so the
   session directory SHOULD be on a disk-backed filesystem; the ksb RSS proof samples `RssAnon`, `memory.current`
   and `memory.stat` and refuses a tmpfs staging directory (ADR-0064 §8.1). Its results will be recorded here.

The rest of this ADR (the encoder, the window, the staged commit, the RAII and journal rules, the disk paragraph)
is unchanged; each leaf is one staged job under the window.
