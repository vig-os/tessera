---
type: issue
state: closed
created: 2026-11-11T01:18:39Z
updated: 2026-11-11T04:19:03Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/808
comments: 1
labels: feature, area:io
assignees: none
milestone: none
projects: none
parent: 799
children: none
synced: 2026-11-11T08:50:38.099Z
---

# [Issue 808]: [feature(io): partitioned table writer with one-leaf-lookahead dispatch, no up-front row count (ADR-0065 §7.3)](https://github.com/vig-os/tessera/issues/808)

Part of epic #799. Design: ADR-0065 (docs/adr/0065-c-abi-bridge.md, PR for #800). **Blocked until the owner accepts ADR-0065**; do not start before.

## What
A `tessera-io` partitioned table writer for streams whose row count is not known up front (ADR-0065 §7.3):

- takes arbitrary batches, stages durable row-group fragments in the `TableMultiBlockSink` format;
- dispatches leaf `i` as the non-terminal `<name>/{i:04 as soon as leaf `i + 1` holds a row (one leaf of lookahead), so only the last leaf waits for `finish`; a table that never exceeds `BLOCK_ROWS` stays the single block `<name>` (small-stays-single, ADR-0026 §4);
- hands each completed leaf to the `StreamWriter` as an ordinary staged job when it completes, without holding a `&mut StreamWriter` for its life (`accumulate.rs:426` today), so #812's thread-safe sequenced input can drive it.

## Acceptance
- Byte-identical to `TableMultiBlockSink` for the same rows at 1, 2 and many leaves (test seam `block_rows = ROWS_PER_GROUP`), and for every worker count (extends `push_table_seals_the_multi_block_sink_manifest_for_every_worker_count`, `stream.rs:2113`).
- A test showing at most a bounded number of leaves of input staged while a long table streams (counts from `StreamStats`), failing against the finish-time dispatch of `TableMultiBlockSink`.
- Every staging file deleted on every error path.
- Rustdoc states the disk and heap bounds.

Size: ≤ ~400 lines. Depends on: nothing.`}

---

# [Comment #1]() by [gerchowl]()

_Posted on November 11, 2026 at 04:19 AM_

Duplicate of #806, which the #801 gap analysis filed first with the same scope; ADR-0065 now cites #806.

