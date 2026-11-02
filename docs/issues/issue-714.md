---
type: issue
state: open
created: 2026-10-30T01:11:59Z
updated: 2026-11-02T04:40:53Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/714
comments: 0
labels: feature, area:testing, area:io
assignees: none
milestone: none
projects: none
parent: 677
children: none
synced: 2026-11-02T08:51:36.047Z
---

# [Issue 714]: [test(io): per-block heap model for push_table and its documented bound (ADR-0064 PR F)](https://github.com/vig-os/tessera/issues/714)

PR F of ADR-0064's implementation plan (§8).

**Scope:** the per-block heap model in `tessera-io/tests/staged_job_memory.rs` (counting allocator), the bound in
`push_table`'s rustdoc (`min(W, n) · p_block + s`, `s = 2 · ROWS_PER_GROUP · r_mem + 4 MiB`, the tmpfs/page-cache
caveat), and ADR-0026's #677 *Known limit* replaced by it.

**Acceptance (fails before), ADR-0064 §8's two-part in-tree test,** consumer schema (24 B/row), input generated one
row-group per pull, `with_block_rows` seam. With `ROWS_PER_GROUP` = 65 536: `s_stage` = 3 MiB, `s` = 7 MiB.
1. **Held heap:** `block_rows = 4 · ROWS_PER_GROUP` (one block `A` = 6 MiB), `W = 2`, all workers held at PR E's
   barrier until the reader stalls; assert `heap_while_held − heap_before ≤ s`. No encode runs, so `p_block` doesn't
   enter. Correct writer: one row-group (1.5 MiB) + bookkeeping. A writer that fills blocks in memory holds
   `(W + 1) · A` = 18 MiB, 11 MiB over `s`: it fails.
2. **Ceiling:** `peak(n, W) ≤ min(W, n) · p_block + s` at `W = 2`, `n ∈ {4, 8}` and `W = 1`, `n = 4`, `p_block`
   measured in the same binary from the `W = 1`, `n = 1` run, which is the calibration and not a pass/fail point
   (it would compare `p_block` with itself and could never fail), at `block_rows = 16 · ROWS_PER_GROUP` (`A` = 24 MiB). An in-memory fill
   inflates its own `p_block` by `A` but holds one more block while `W` encode, so it exceeds the bound by
   `A − s` = 17 MiB. (At the old `4 · ROWS_PER_GROUP` the excess was 6 − 7 = −1 MiB: the ceiling alone could never
   fail, hence part 1.)

Both parts are shown failing once against a `push_table` variant that fills blocks in memory.

---
Design: ADR-0064 "Logical tables over numbered leaves" (`docs/adr/0064-logical-tables-over-numbered-leaves.md`,
Proposed, PR for #677). Implement only after the ADR is Accepted. Sub-issue of #677; order A → B → C1, C2 → D → E → F → G → H
(D lands only after B, C1 and C2, so no writer on `dev` writes a form a reader on `dev` can't read).



