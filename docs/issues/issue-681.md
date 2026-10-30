---
type: issue
state: open
created: 2026-10-26T13:46:38Z
updated: 2026-10-30T01:38:41Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/681
comments: 3
labels: bug, priority:backlog, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-30T08:02:26.213Z
---

# [Issue 681]: [fix(io): Vortex 0.75 writer holds encoded segments until the footer (unbounded per-chunk fan-out)](https://github.com/vig-os/tessera/issues/681)

Found while closing gap 1 of #677 (file-backed single-table sink). With the encoded output streamed to a
file, the encode's anonymous memory is ~5x lower than at `9424f90`, but it still grows with the table: the
pinned Vortex 0.75 writer (`gerchowl/vortex@608d8a1`) holds encoded segments in memory until the end of
the stream.

## Evidence

Consumer reproducer (MorePET/visionx-rs `chore/39-tessera-677-repro`, exact nullable decoder shape,
switched to `table_job_streaming`), ksb, 4 pinned cores, 12 GiB:

| rows | peak RssAnon | output bytes | at `9424f90` |
|---:|---:|---:|---:|
| 268,435,456 | 909,967,360 | 2,048,683,867 | 4,758,114,304 |
| 1,600,000,000 | 5,152,997,376 | 12,214,036,720 | — |

RssAnon rises ~3.2 bytes/row during the encode and drops when the footer is written.

A counting global allocator around `table::encode_streaming_to(spec, groups, sink)` (1024 row-groups,
13 listmode columns) shows the bytes are encoded segments that have not reached the sink yet: at
row-group 896, 584 MB live and 347 MB written of ~1 GB produced. A single random `u64` column (no dict):
445 MB live, 27 MB written. A single sequential `u64` column delivers nothing to the sink before the end.
valgrind massif attributes the live bytes to compressed buffers (`bitpack_encode` via
`CascadingCompressor::compress`).

## Root cause

`ChunkedLayoutStrategy::write_stream` (`vortex-layout/src/layouts/chunked/writer.rs`) spawns one task per
chunk and polls them with `stream.buffered(usize::MAX)`. Each task holds its compressed chunk until
`BufferedSegmentSink::write` can write it, and segments are written in ONE global `SequenceId` order across
all columns. Nothing applies backpressure to the producer, so compressed chunks pile up in pending tasks.

Bounding that fan-out is not a drop-in fix. Measured with a patched copy: `buffered(64)` bounds the live
heap (~190 MB plateau, sink keeps pace), but `buffered(2)` and `buffered(8)` **deadlock**, because a
bounded column pipeline waits on another column's earlier sequence id that the table fan-out
(`kanal::bounded(1)` per column) can't deliver. A worker pool on the encode's runtime does not change
this (it only changes how fast the pending tasks are drained).

## What's needed (design question)

A deadlock-free bound on in-flight encoded segments in the Vortex writer that keeps the segment order,
and so the bytes and `content_hash`, unchanged. Candidates: a byte-budgeted fan-out that always admits the
chunk holding the lowest pending sequence id (the same rule as tessera's `StreamWriter` window, ADR-0026
amendment), upstream in vortex and carried on the fork pin (#480). Until then, the RAM of a single huge
table encode is ~0.4 x its encoded size, documented in ADR-0026's "single-table output sink" amendment.

---

# [Comment #1]() by [gerchowl]()

_Posted on October 29, 2026 at 11:43 AM_

Design note merged (PR #692, b6de2d1): `docs/spikes/681-vortex-writer-backpressure.md`, 3 rounds, an independent pre-mortem and 2 re-reviews. **Owner decision pending: P / P-logical (recommended) / R for #677.** Nits from the final re-review, to fix in the follow-up ADR (its first implementation PR):
1. The decision table's cost column for P-logical must include §5.1 "writers switch": every multi-block writer's output is renamed (GE ingest `events_NNNN`, `TableMultiBlockSink`), so `content_hash`es change beyond #677 and the multi-block corpus goldens move. "Readers don't change" holds only for P.
2. Grouping isn't unambiguous: a user's `pairs/0003` next to the writer's `pairs/0000…0002` joins the group. Reserve numbered leaves, or require a contiguous run from `0000`. Also define a product that mixes `pairs_NNNN` and `pairs/NNNN`.
3. Disk staging's "no RAM term" ignores page cache, which counts toward cgroup `memory.current` and is RAM on tmpfs. The RSS proof must also sample `memory.current`, or require staging on a real disk.
4. `staged_peak` is counted only for complete blocks, so it doesn't test the `W+1` bound. Give `peak(16) ≤ peak(4)+s` an explicit `s` (the measured rise at W=4 was 11% from 16 to 64 blocks).
5. ADR-0062's `RowAlignment` doc ("`events` = `events_NNNN`") needs amending in the same ADR.

---

# [Comment #2]() by [gerchowl]()

_Posted on October 30, 2026 at 01:26 AM_

**Status update (2026-10-30).** The owner decided on **P-logical** for #677 (see the decision comment on #677). Streamed tables are written as `pairs/NNNN` leaves and read as one logical table. The bound is flat at 128–143 MB from 67 M to 1.07 G rows at W=4, without any Vortex change. ADR-0064 is being drafted, and it folds in the 5 nits above.

So #677 no longer depends on this issue. What remains here is the **root cause in Vortex itself**: the 0.75 writer holds encoded segments until the footer, so one physical Vortex file's writer memory grows with its row count. That is option F in the design note, an upstream writer gate. The closest upstream report is vortex-data/vortex#10046 (DuckDB `COPY TO … FORMAT VORTEX` heap growth, open, last updated 2026-10-01). It is a related symptom, not a filed fix. Nobody is working on F yet.

This issue stays open as the upstream track. With P-logical in place it no longer blocks tessera, because every physical block is bounded by `BLOCK_ROWS`.

---

# [Comment #3]() by [gerchowl]()

_Posted on October 30, 2026 at 01:38 AM_

**Owner decision (2026-10-30): no upstream contribution for now.** P-logical (ADR-0064) bounds memory without any Vortex change, so there's no case for one. This stays a backlog record of the Vortex writer behaviour, together with its measurements and the option-F design. Reopen the question only if a use case needs a single physical Vortex file larger than `BLOCK_ROWS`.

