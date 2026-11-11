---
type: issue
state: open
created: 2026-11-11T01:15:15Z
updated: 2026-11-11T04:19:01Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/806
comments: 1
labels: feature
assignees: none
milestone: none
projects: none
parent: 799
children: none
synced: 2026-11-11T08:50:39.205Z
---

# [Issue 806]: [feature(io): stream a partitioned table whose row count is known only at close](https://github.com/vig-os/tessera/issues/806)

## Gap (from the #801 consumer gap analysis)

`StreamWriter::push_table(name, spec, groups)` needs `spec.rows` before the first row. It computes the leaf count, names and the terminal marker up front, and refuses input that ends short or runs long (ADR-0064 §5.3; `tessera/crates/tessera-io/src/stream.rs:927-960`).

The #799 C++ producers don't know their row count until the run ends:
- Geant4 writes one `pethits` row per (event, crystal, photon) above threshold. The count is data-dependent.
- gPET-sim2listmode appends singles until its Poisson clock ends.
- geddf-analysis appends coincidences per chunk. Its `time_markers` length is known only at the end.

Today the only way to write such a table under the partition rule is to stage the whole table first, which is the unbounded buffering #677 removed.

## Ask
A design decision, in #800's ADR or an ADR-0064 amendment. Can a streamed partitioned table be opened without a row count, with the leaf count, last-leaf marker and bare-vs-partitioned naming decided at close?

Constraints any answer must keep:
- **Byte identity with the batch partition of the same rows.** For example, write leaves as `<table>/NNNN` while open and decide "single block, bare name" only at close, which means a rename of leaf 0 before the seal.
- **The ADR-0064 §8 memory bound.**

---

# [Comment #1]() by [gerchowl]()

_Posted on November 11, 2026 at 04:19 AM_

ADR-0065 (PR #819) specifies this issue's writer as the bridge's table path: one leaf of lookahead, so a leaf is dispatched as soon as the next leaf holds a row and only the last one waits for close; byte-identical to `TableMultiBlockSink`; leaves handed to the `StreamWriter` as ordinary staged jobs without a long-lived `&mut StreamWriter` (ADR-0065 §7.3). #808, which I filed with the same scope before seeing this issue, is closed as its duplicate; its acceptance criteria are copied below.

## Acceptance
- Byte-identical to `TableMultiBlockSink` for the same rows at 1, 2 and many leaves (test seam `block_rows = ROWS_PER_GROUP`), and for every worker count (extends `push_table_seals_the_multi_block_sink_manifest_for_every_worker_count`, `stream.rs:2113`).
- A test showing at most a bounded number of leaves of input staged while a long table streams (counts from `StreamStats`), failing against the finish-time dispatch of `TableMultiBlockSink`.
- Every staging file deleted on every error path.
- Rustdoc states the disk and heap bounds.

Size: ≤ ~400 lines. Depends on: nothing.`}

