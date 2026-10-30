---
type: issue
state: open
created: 2026-10-25T11:18:53Z
updated: 2026-10-30T01:07:09Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/667
comments: 1
labels: feature, priority:high, effort:medium, area:io
assignees: none
milestone: none
projects: none
parent: 654
children: none
synced: 2026-10-30T08:02:31.136Z
---

# [Issue 667]: [feat(query): positional zip of row-aligned table blocks (ADR-0062 PR2)](https://github.com/vig-os/tessera/issues/667)

Phase 2 of #654, designed in ADR-0062 §5 and §8 "PR2".

- **Scope:** positional zip in `tessera_io::scan` and `tessera-query`:
  - declared mode: the sealed `TableSpec.aligned_to` (`RowAlignment` with no public constructor, recording the
    source block group's digest), written only by the named derive paths of ADR-0062 §5, with its seal-time
    check;
  - key mode: a caller-named, NULL-free key that must be strictly increasing across the emitted rows in table
    order, equal across inputs, proven by a **key-only pre-pass** over the whole scanned range before any row
    is released (ADR-0062 §5, owner decision (c)): within a partition the chain runs across row groups and
    units on the last checked key of the nearest preceding emitted unit; across partitions a boundary stage,
    started for all partitions at first execute, checks each adjacent pair; each pre-pass runs on its
    partition's producer thread, never on the fill pool (nothing that waits on a fill may run there, ADR-0062
    §3, §5). A **release gate** holds every
    row until the pre-pass and every boundary check have passed, so a pushed `LIMIT`, a `LIMIT` above the
    scan or a dropped stream only ever sees verified rows; the row pass re-checks the keys it decodes and
    each partition's first and last key against the registered ones. The gate is a query-wide cell resolved
    once, to passed or to the error of the first pre-pass or boundary check that fails. Every producer waiting
    at the gate, reaching it later or having resolved it sends that error on its own stream and stops,
    releasing its pins and slots, so whichever partition the consumer polls returns the error; no producer may
    wait on the gate without also observing its failure state (ADR-0062 §5 "A failed check resolves the
    gate"). Tier 1 prunes only on the key
    predicate when the key is every input's indexed column, and the zip re-applies that predicate before
    release; pruned blocks are never read;
  - the manifest-only checks before the first row; tier-1 pruning applied to all inputs together; no in-scan row
    filter under a zip.
- **Acceptance:**
  - Equality: a zip equals the hash join on the key column for aligned inputs, with and without filters.
  - Refusal before any row: different total rows, different per-block rows, a missing or ambiguous `sources`
    role, a missing `content_hash`, a wrong source `manifest_hash`, a source block-group digest that differs from
    the recorded one, a missing block prefix, and a column collision (other than a key-mode key) each produce
    `AlignmentError`.
  - Key mode: a tie, an inversion, a NULL key, a key altered in row group *g*, a tie on a unit boundary inside
    one partition and a tie on a partition boundary each fail the query with an error naming the row.
    ADR-0062 §5's counterexample (A with keys `[…, 5] [3] [5, …]`, B the same with the two `5` rows swapped,
    queried with `WHERE key >= 5` so the middle group is pruned) is refused, with the gap inside one block and
    with the gap a whole pruned block, with both `5`s in one partition and in two; asserted on the released
    batches, no swapped pair is ever zipped. An inversion across a pruned gap and across a partition edge is
    refused the same way. Key mode reads no byte of a pruned block (its `bytes_fetched` is 0). The key column
    is exempt from the collision refusal and emitted once. `WHERE key >= 5 AND e > 3` prunes on the key
    conjunct only. An event-id key under `WHERE row_index <> 2` (the key is not the indexed column) switches
    tier 1 off for the zip, so the `[1, 5] [5] [7]` swap of §5 is checked and refused. A scanned boundary key
    that differs from the registered one fails the query. A consumer that executes the partitions one after
    another completes without hanging. The docs say that pruning in key mode trusts the `.cidx` statistics.
  - Design constraint, not checked off here — key-mode spool slots (ADR-0062 §5 and §7 "Spool slots"; the spool
    lands in #668, whose acceptance tests it, and PR2's key-mode code is built to the rule): read from the spool, a key-mode zip holds per input and
    partition its pinned first and last block, the block being scanned and the prefetched next one, so it
    needs at most `4 · k · P` slots; `plan_scan` computes the plan's need, makes it the default and refuses a
    configured `spool_blocks` below it with a typed error before any row. Tested on an object store **and** on
    a local source with the spool opt-in: at the default, a key-mode zip whose consumer drains one partition
    before the others completes; one slot below the need is refused by `plan_scan`; a boundary block shared
    by two partitions (pinned by both) is spooled once and counted once; the spool's high-water mark never
    exceeds the computed need.
  - Release gate (owner decision (c), ADR-0062 §5): a key-mode zip with a pushed `LIMIT` or an early stop is
    refused until the key-only pre-pass has verified strict increase over the whole scanned range. Test:
    A with keys `[5, 6] [3, 5]` in two row groups, B the same with the two `5` rows swapped, queried with
    `LIMIT 1`, fails the query and returns no row; this test fails on a neighbour-check-only implementation,
    which returns the misaligned pair. The same case with the two row groups in two partitions, and with no
    `LIMIT` but a consumer that drops the stream after its first batch, returns no row either. On aligned
    inputs the pre-pass completes before the first batch is released, and `LIMIT n` then returns exactly
    `n` correctly paired rows.
  - Gate failure (ADR-0062 §5 "A failed check resolves the gate"), on an unspooled local source: a key-mode zip
    with `k = 2` and `P = 2` whose consumer executes and polls only the first partition. With an inversion inside
    the second partition's run, the polled stream returns that inversion's error, naming the input and the row,
    within the test's timeout, and both producer threads join through the test hook of "Producer lifetime". The
    same holds for a failed boundary check: each run strictly increasing and the first partition's last key equal
    to the second's first, a tie on the partition boundary, whose error names the input and the row as every
    key-mode failure does (ADR-0062 §5). A gate-without-failure implementation (a gate that only signals passing,
    so a producer waits at it without observing its failure state) leaves the first partition waiting at the gate,
    its stream never yields, and the test's timeout fails it on every run in both cases. One that sends the error
    only on the failing partition's stream fails it too: the polled stream never yields the error.
  - Writer side: generic `ingest` and `commit --add-block` refuse a spec carrying `aligned_to`;
    `derive_aligned` refuses a transform that changes a batch's row count.
  - Fill pool bound (ADR-0062 §3 `mem(fills)`, §7), on an unspooled local source: a declared-mode zip with
    `k = 2` and `P = 2` whose inputs each have at least two blocks, with both inputs sharing block
    boundaries and each partition's run starting on a block boundary in every input (so the four first
    blocks are distinct and no partition waits on another's cell), and a fill gate (a test gate that holds
    every fill). Once both partitions are executed, each has requested its two inputs' first-block fills
    together (§7), four in all (no partition decodes while its fills are held, so none prefetches). The pool
    exposes the number of OS threads it has created during the query (counted by the pool when it creates a
    thread, before that thread runs), its running count (fills that have reached the gate), its queued count
    (fills submitted that no thread has taken; a fill a thread has taken but that has not reached the gate
    is neither) and its idle-thread count (a created thread that holds no fill, whether or not it has run
    yet). The pool creates its `P` threads when the scan starts, before any fill is submitted (ADR-0062 §3).
    A second test gate holds both partitions' producers before their first fill request; with it closed, the
    test asserts threads created = `P` and no fill submitted, then opens it. The test then waits until
    running = `P`, queued = 4 − `P` and no thread is idle, or running > `P`, or threads created ≠ `P`; it
    then asserts threads created = `P`, running ≤ `P` and queued = 4 − `P`, with the queued fills holding no
    buffer, and after the fill gate opens and all four fills complete it asserts threads created = `P`
    again. A pool of `P` threads always reaches the first condition and stays in it, since the fill gate
    stops any fill from finishing, and passes. A pool that creates a different number of threads (a spare
    thread, a dispatcher, one per fill, or fewer than `P`) ends the wait through threads created ≠ `P` and
    fails on every run whatever the interleaving; one that creates threads lazily, on demand, fails the
    check before the first fill, when it has created none, and one that creates them after a fill finishes
    fails the final count; and one that runs more than `P` fills fails on running ≤ `P`. Any other pool that
    never settles fails on the test's timeout.
  - Pre-pass threads (ADR-0062 §5, §3 "nothing that waits on a fill may run on the fill pool"), on an unspooled
    local source: a key-mode zip with `k = 2` and `P = 2` over aligned inputs, each partition's run starting on a
    block boundary in every input, with the fill gate of "Fill pool bound" holding every fill. The consumer
    executes only the first partition. The test waits until the pool's running count is `P` and both partitions'
    pre-passes are observed waiting on a verify-once cell, the second partition's included, although it has not
    been executed. A pre-pass is not a fill and never counts as running. It asserts that each pre-pass runs on its
    partition's producer thread (the pre-pass records its thread id, which the test compares with the producer's
    and with every fill-pool thread's) and that the pool has created exactly `P` threads. It then opens the fill
    gate and executes the second partition, and the query completes with rows equal to the hash join. An
    implementation that schedules a pre-pass on the fill pool keeps that pool thread while the pre-pass waits on
    its fills, so at most `P − 1` fills can run (none when both pre-passes are there), running never reaches `P`,
    the fills the pre-passes wait on stay queued, and the test's timeout fails it on every run whatever the
    interleaving.
  - Producer lifetime (ADR-0062 §5), on an unspooled local source: a key-mode zip with `P = 2` over aligned
    inputs, each partition's run yielding more than `batches_buffered` + 1 batches, whose consumer executes only
    the first partition and reads one batch, then waits until both producers are observed blocked on a full
    channel; a second `execute` of the first partition then returns the error naming it; the consumer then drops
    its stream and the plan. Both producer threads, the unexecuted partition's included, join within the test's
    timeout, through a test hook that exposes the producers' join handles. An implementation that keeps an
    unexecuted partition's receiver alive after the plan is dropped leaves its producer blocked, and the join
    times out.
  - Bounded memory: the ksb run of ADR-0062 §3 repeated on a zip of two blocks; the anonymous peak stays within
    the per-query formula including the key-mode buffers.
- **Depends on:** PR1.








---

# [Comment #1]() by [gerchowl]()

_Posted on October 30, 2026 at 01:07 AM_

Blocked by 708: three ADR-0062 rules (any pre-pass error resolves the gate, a panicking producer resolves it, the orphaned-slot owner) must be amended before this PR implements the gate and spool.

