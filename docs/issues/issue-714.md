---
type: issue
state: open
created: 2026-10-30T01:11:59Z
updated: 2026-10-30T01:19:48Z
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
synced: 2026-10-30T08:02:15.979Z
---

# [Issue 714]: [test(io): per-block heap model for push_table and its documented bound (ADR-0064 PR F)](https://github.com/vig-os/tessera/issues/714)

PR F of ADR-0064's implementation plan (§8).

**Scope:** the per-block heap model in `tessera-io/tests/staged_job_memory.rs` (counting allocator), the bound in
`push_table`'s rustdoc (`min(W, n) · p_block + s`, `s = 2 · ROWS_PER_GROUP · r_mem + 4 MiB`, the tmpfs/page-cache
caveat), and ADR-0026's #677 *Known limit* replaced by it.

**Acceptance (fails before):** `peak(n, W) ≤ min(W, n) · p_block + s` at `W = 2`, `n ∈ {4, 16}` and `W = 1`,
`n ∈ {1, 4}`, at `block_rows = 4 · ROWS_PER_GROUP` via the `with_block_rows` seam, `p_block` measured in the same
binary; shown failing once against a variant that fills blocks in memory.

---
Design: ADR-0064 "Logical tables over numbered leaves" (`docs/adr/0064-logical-tables-over-numbered-leaves.md`,
Proposed, PR for #677). Implement only after the ADR is Accepted. Sub-issue of #677; order A → B → C1, C2 → D → E → F → G → H
(D lands only after B, C1 and C2, so no writer on `dev` writes a form a reader on `dev` can't read).

