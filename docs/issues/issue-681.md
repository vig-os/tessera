---
type: issue
state: open
created: 2026-10-26T13:46:38Z
updated: 2026-10-26T13:46:38Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/681
comments: 0
labels: bug, priority:high, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-27T07:16:20.378Z
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

