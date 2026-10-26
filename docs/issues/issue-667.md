---
type: issue
state: open
created: 2026-10-25T11:18:53Z
updated: 2026-10-25T12:56:20Z
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
synced: 2026-10-26T07:24:21.926Z
---

# [Issue 667]: [feat(query): positional zip of row-aligned table blocks (ADR-0062 PR2)](https://github.com/vig-os/tessera/issues/667)

Phase 2 of #654, designed in ADR-0062 §5 and §8 "PR2".

- **Scope:** positional zip in `tessera_io::scan` and `tessera-query`:
  - declared mode: the sealed `TableSpec.aligned_to` (`RowAlignment` with no public constructor, recording the
    source block group's digest), written only by the named derive paths of ADR-0062 §5, with its seal-time
    check;
  - key mode: a caller-named, NULL-free key that must be strictly increasing, checked while streaming and
    across unit boundaries;
  - the manifest-only checks before the first row; tier-1 pruning applied to all inputs together; no in-scan row
    filter under a zip.
- **Acceptance:**
  - Equality: a zip equals the hash join on the key column for aligned inputs, with and without filters.
  - Refusal before any row: different total rows, different per-block rows, a missing or ambiguous `sources`
    role, a missing `content_hash`, a wrong source `manifest_hash`, a source block-group digest that differs from
    the recorded one, a missing block prefix, and a column collision each produce `AlignmentError`.
  - Key mode: a tie, an inversion, a NULL key, a key altered in row group *g*, and a tie placed exactly on a
    unit boundary (the two rows in different partitions; each unit reads the key of the row before its range)
    each stop the scan with an error naming the row; every emitted row comes from rows before it.
    A tie straddling a tier-1-pruned row group, and one straddling a whole pruned block, are caught the same
    way: the predecessor row is always read (its block verified under the active integrity mode first) and
    counted in `boundary_rows_read`.
  - Writer side: generic `ingest` and `commit --add-block` refuse a spec carrying `aligned_to`;
    `derive_aligned` refuses a transform that changes a batch's row count.
  - Bounded memory: the ksb run of ADR-0062 §3 repeated on a zip of two blocks; the anonymous peak stays within
    the per-query formula including the key-mode buffers.
- **Depends on:** PR1.


