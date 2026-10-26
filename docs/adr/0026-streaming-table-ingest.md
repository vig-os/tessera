# ADR-0026 — Streaming, chunked table writes for >RAM ingest

Status: **Proposed** (2026-06-26) · Extends ADR-0024 (table payload), ADR-0025 (ingest), relates to
S17 (`WriteSession`). Blocks the >RAM half of `#208` (GE listmode `events_2p` ≈ 10⁸ rows ≈ 5–7 GB).

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

**Scope.** `P` is still one job's encoded payload in RAM. The second half of #677 (a file-backed encode
sink for one canonical table block, and a streaming commit from the staged file) extends this contract to
the encoded output itself, so a single block larger than RAM is bounded too.
