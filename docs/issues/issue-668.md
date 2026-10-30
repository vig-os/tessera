---
type: issue
state: open
created: 2026-10-25T11:18:54Z
updated: 2026-10-30T01:07:10Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/668
comments: 1
labels: feature, priority:medium, effort:large, area:io
assignees: none
milestone: none
projects: none
parent: 654
children: none
synced: 2026-10-30T08:02:30.727Z
---

# [Issue 668]: [feat(query): multi-product and Collection scans, local then object store (ADR-0062 PR3)](https://github.com/vig-os/tessera/issues/668)

Phase 3 of #654, designed in ADR-0062 §6, §7 and §8 "PR3".

- **Scope:** multi-product and Collection scans: member resolution (ADR-0049 §4), `manifest_hash` pinning,
  sub-collection flattening with the cycle guard, (product, block, row-group run) units, the schema policy
  (identical, or explicit common columns), the opt-in product column, table/scan-node statistics. Then the
  object store: the per-store I/O thread, the ranged `VortexReadAt`, the VerifyBlocks spool (with its slot rule and the local opt-in of
  ADR-0062 §7), and the
  `RangeSource` seam that lets a consumer's DataFusion `ObjectStoreRegistry` (object_store 0.13) feed the reader.
- **Acceptance:**
  - Equality: a collection scan equals the `UNION ALL` of its products' single-product scans.
  - Refusal before any row: a member `manifest_hash` mismatch, an unresolvable member, and a schema mismatch.
  - Object store: against MinIO (extending the `minio-range-read` flake check), results equal the local scan;
    pruned blocks issue zero GETs, in a key-mode zip too; the spool stays within its bound; a store taken from a DataFusion
    `ObjectStoreRegistry` reads the same rows as one opened by `open_url`.
  - Spool slots (ADR-0062 §7 "Spool slots", resolving #697), on an object store and on a local source with the
    spool opt-in: `plan_scan` computes the plan's need (at most `2 · k · P`, `4 · k · P` in key mode), makes it
    the default and refuses a configured `spool_blocks` below it with a typed error before any row. At the
    default, a declared-mode zip with `k = 2` and `P = 1`, a prefetching single-product scan and a key-mode zip
    each complete when the consumer drains one partition before the others; a block shared by two partitions is
    re-fetched and re-hashed outside key mode by a partition that reaches it after the other released it
    (counted in each partition's need), and pinned once in key mode (counted once); the spool's high-water
    mark never exceeds the computed need, including `k = 1`, `P = 2`, runs `{1, 2, 3}` / `{3, 4, 5}` with the
    first partition blocked holding blocks 1 and 2 while the second holds 4 and 5 (need 4, not 3;
    blocks of one window each and larger than a partition's channel, as in the next criterion).
  - Split block outside key mode (ADR-0062 §7 "Once per block"), on an object store and on a local source with
    the spool opt-in: a single-product scan with `k = 1`, `P = 2` and runs `{1, 2, 3}` / `{3, 4, 5}`, at the
    default `spool_blocks` (`2 · k · P` = 4), with every block one window (at most `rows_per_window` rows, §3,
    so a partition prefetches the next block while decoding the current one) and larger than a partition's
    channel (more than `batches_buffered · batch_rows` rows), so the first partition blocks on its full
    channel inside block 1 holding blocks 1 and 2. A test gate holds the second partition's producer before it
    touches block 3 until the first partition is observed in that state; the consumer then drains the second
    partition to its end before reading the first. The query completes; block 3's hash is computed twice
    (counted at the verify-once cell) and its bytes fetched twice. Under a hashed-once implementation that
    keeps block 3 spooled for the first partition, slots 1 to 4 stay held, the second partition never gets a
    slot for block 5, and the test's timeout fails it on every run.
  - Fill independent of its owner (ADR-0062 §7 "Once per block"), on an object store and on a local source
    with the spool opt-in (#666 tests the unspooled leg): runs `{1, 2, 3}` / `{3, 4, 5}` with every block one
    window (at most `rows_per_window` rows), block 1 no larger than a partition's channel and block 2 larger,
    so the first partition empties block 1 into its channel, enters block 2 and starts block 3's fill as its
    prefetch. Two test gates fix the order. Gate 1 holds the second partition's producer before it touches
    block 3 and opens when the first partition has registered as the starter of block 3's verify-once cell.
    Gate 2, on the `RangeSource` (or the local hash pass), holds block 3's fill and opens when the first
    partition is observed blocked on its full channel inside block 2. The consumer drains the second partition
    first: its wait on block 3's cell resolves and the query completes, with block 3 hashed once. A fill run
    on the starting partition's producer keeps that producer inside the gated fill, so it never blocks on its
    channel, gate 2 never opens, the cell never resolves, and the test's timeout fails it on every run. With a
    corrupted byte in block 3 the query fails with the integrity error and returns no row of block 3.
  - Waiter holds the copy (ADR-0062 §7 "Once per block"), on an object store and on a local source with the
    spool opt-in: runs `{1, 2, 3}` / `{3, 4, 5}`, every block one window, block 1 no larger than a
    partition's channel and blocks 2 and 3 larger. Gate 1 holds the first partition before it touches block 3
    until the second partition has registered as the starter of block 3's cell; gate 2 holds the first
    partition after block 3's cell resolves and before it opens block 3. The consumer starts draining the
    second partition only once the first partition has registered as a waiter on block 3's cell (its prefetch
    from inside block 2) and is observed blocked on its full channel inside block 2 or held at gate 2; until
    then the second partition is blocked on its own full channel inside block 3. Drained to its end, the
    second partition scans block 3 and moves on to blocks 4 and 5. The test asserts block 3's copy is still
    spooled after the second partition has moved past it, then opens gate 2 and drains the first partition;
    the query's rows equal the unspooled scan's, with block 3 hashed once and at most 4 slots in use. An
    implementation that counts the waiter as a holder only when it opens the copy releases block 3 when the
    second partition moves on and reuses its slot for block 5, so the assertion fails, and the first
    partition reads a missing or reused spool file.
  - Fill pool bound, read from the spool: #667's criterion repeated on an object store and on a local
    source with the spool opt-in.
  - Pre-pass threads, read from the spool: #667's "Pre-pass threads" repeated on an object store and on a local
    source with the spool opt-in, the fill gate holding each block's download or copy into the spool. The specified
    design completes within the default `spool_blocks` (`4 · k · P`); a pre-pass scheduled on the fill pool keeps
    running below `P`, and the test's timeout fails it on every run.
  - Producer lifetime, read from the spool: #667's "Producer lifetime" repeated on an object store and on a local
    source with the spool opt-in; after both producers join, the spool holds no slot and no spooled file of the query,
    and an implementation whose stopped producer keeps its pins fails that assertion.
  - Gate failure, read from the spool: #667's "Gate failure" repeated on an object store and on a local source
    with the spool opt-in, in both cases; after the error, and once both producer threads have joined through the
    test hook of "Producer lifetime" and the fill pool is idle, the spool holds no slot and no spooled file of the
    query, so every pin and slot has been released. A gate-without-failure implementation times out as in #667.
  - Local spool opt-in: with it, a byte changed in the local file after its block was spooled and hashed
    reaches no row; without it, a local scan creates no spool file.
  - Bounded memory: the ksb run of ADR-0062 §3 over a many-product collection.
- **Depends on:** PR1. **Split if needed:** (a) local multi-product; (b) object store.





















---

# [Comment #1]() by [gerchowl]()

_Posted on October 30, 2026 at 01:07 AM_

Blocked by 708: three ADR-0062 rules (any pre-pass error resolves the gate, a panicking producer resolves it, the orphaned-slot owner) must be amended before this PR implements the gate and spool.

