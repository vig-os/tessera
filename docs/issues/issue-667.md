---
type: issue
state: open
created: 2026-10-25T11:18:53Z
updated: 2026-10-26T12:00:23Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/667
comments: 0
labels: feature, priority:high, effort:medium, area:io
assignees: none
milestone: none
projects: none
parent: 654
children: none
synced: 2026-10-27T07:16:25.958Z
---

# [Issue 667]: [feat(query): positional zip of row-aligned table blocks (ADR-0062 PR2)](https://github.com/vig-os/tessera/issues/667)

Phase 2 of #654, designed in ADR-0062 §5 and §8 "PR2".

- **Scope:** positional zip in `tessera_io::scan` and `tessera-query`:
  - declared mode: the sealed `TableSpec.aligned_to` (`RowAlignment` with no public constructor, recording the
    source block group's digest), written only by the named derive paths of ADR-0062 §5, with its seal-time
    check;
  - key mode: a caller-named, NULL-free key that must be strictly increasing across the emitted rows in table
    order, checked row by row within a unit and, across units, against the last emitted key of the nearest
    preceding emitted unit (a boundary stage, registered for all partitions at first execute, for partition
    boundaries); tier 1 prunes only on the key predicate when the key is every input's indexed column, and the
    zip re-applies that predicate before release; pruned blocks are never read;
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
    another completes without hanging. A boundary block shared by two partitions is spooled once, and a
    key-mode object-store scan with fewer spool slots than it needs is refused by `plan_scan`. A `LIMIT`
    query's rows are neighbour-checked as §5 states, and the docs say so, as they say that pruning in key
    mode trusts the `.cidx` statistics.
  - Writer side: generic `ingest` and `commit --add-block` refuse a spec carrying `aligned_to`;
    `derive_aligned` refuses a transform that changes a batch's row count.
  - Bounded memory: the ksb run of ADR-0062 §3 repeated on a zip of two blocks; the anonymous peak stays within
    the per-query formula including the key-mode buffers.
- **Depends on:** PR1.



