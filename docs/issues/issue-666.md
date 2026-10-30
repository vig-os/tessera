---
type: issue
state: open
created: 2026-10-25T11:18:51Z
updated: 2026-10-29T13:46:42Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/666
comments: 0
labels: feature, priority:high, effort:large, area:io
assignees: none
milestone: none
projects: none
parent: 654
children: none
synced: 2026-10-30T08:02:31.537Z
---

# [Issue 666]: [feat(query): streaming single-product TableProvider with chunk pruning and bounded memory (ADR-0062 PR1)](https://github.com/vig-os/tessera/issues/666)

Phase 1 of #654, designed in ADR-0062 (§1–§4, §7, §8 "PR1"; `docs/adr/0062-streaming-table-provider.md`).

- **Scope:**
  - `tessera_io::scan`: `plan_scan`, scan units, `TableScan`, `ScanBudget`, the per-query scan-partition cap,
    explicit row-range window scans with `SplitBy::RowCount`, the layout-reader cache and a fresh
    `MokaSegmentCache` per opened block, tier-1 `.cidx` pruning, `Integrity::{VerifyBlocks, SealOnly}` with
    verify-once per block and the scan's fill pool of `P` threads (ADR-0062 §3, §7: a block's hash pass never
    runs on a partition's producer), the local byte-range `VortexReadAt`, and metrics.
  - A new `tessera-query` crate: `TableProvider` over one product's logical table, `Expr` → `Predicate` for the
    closed class (every pushed filter `Inexact`), metrics surfaced through `EXPLAIN ANALYZE`, re-export of
    `datafusion`.
  - `tessera sql` migrated onto it; the materialising path survives only as the test oracle.
- **Acceptance:**
  - Equality: a query corpus (projections, every operator in the class, NULLs, aggregates, `LIMIT`) on single-
    and multi-block (`events_NNNN`) fixtures equals the `MemTable` path's result.
  - Pushdown safety: the pushdown-on vs pushdown-off differential property test passes (every dtype in the
    class, NULLs, boundary values, data shaped so each integer/string encoding is chosen).
  - Instrumented pruning: with the indexed column selecting *k* of *N* row groups, `row_groups_pruned = N − k`;
    blocks that can't match have `bytes_fetched = 0`; a zone-prunable predicate reads fewer bytes than the same
    projection without it.
  - Window bytes: window scans read the same bytes as one scan of the block, on a table with a `utf8` and a
    dictionary-encoded column.
  - Bounded memory: the CI high-water test of ADR-0062 §3, on a runner with more cores than the budget; the ksb
    run of §3 (anonymous-memory peak, pinned `target_partitions`, dedicated cores via `ksb run --exclusive-cpus`, scale invariance, and the
    negative control: the materialising oracle path in the same cgroup must be OOM-killed), with its numbers in the performance book.
  - Integrity: a flipped payload byte in a scanned block fails the query with `Error::Integrity` and no row of
    that block is emitted; the same flip in a tier-1-pruned block does not fail it, and the docs say so.
  - Fill pool (ADR-0062 §3, §7 "Once per block"), on an unspooled local source: with a test gate holding
    both partitions' producers before their first fill request, the pool has created exactly `P` threads
    (counted at creation) and no fill is submitted; after the query it has still created `P`. A
    single-product scan with `k = 1`, `P = 2` and runs `{1, 2, 3}` / `{3, 4, 5}`, every block one window (at
    most `rows_per_window` rows), block 1 no larger than a partition's channel and block 2 larger, and a
    consumer that drains the second partition first, completes with block 3 hashed once when two more test
    gates fix the order: one holds the second partition's producer before it touches block 3 until the first
    partition has registered as the starter of block 3's verify-once cell (its prefetch from inside block
    2), and one holds block 3's hash pass until the first partition is observed blocked on its full channel
    inside block 2. A hash pass run on the starting partition's producer, or as a future on its runtime,
    never resolves the cell, and the test's timeout fails it on every run. With a corrupted byte in block 3,
    the hash pass fails and resolves the cell to the error: the second partition, waiting on that cell,
    returns `Error::Integrity` rather than hanging, the query fails with it, and no row of block 3 is
    returned; an implementation that resolves the cell only on success, or reports the error only to the
    partition that started the fill, times out. #668 repeats this test read from the spool. While their
    fills are held, a single-product scan cannot submit more than `P` fills, so the pool's queue is first
    exercised by #667's "Fill pool bound". The pool's submission API takes a fill (a block, its source and
    its verify-once cell), not a closure, so no task that waits on a fill can be scheduled on it (ADR-0062
    §3); #667's "Pre-pass threads" tests the one such task the ADR defines.
  - Gate B snapshots regenerated with the inert-delta proof.
- **Depends on:** ADR-0062 accepted.
- **Split if needed:** (a) `tessera_io::scan` + tests; (b) `tessera-query` + the `tessera sql` migration.





