---
type: issue
state: open
created: 2026-10-30T01:11:54Z
updated: 2026-10-30T01:19:43Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/711
comments: 0
labels: feature, area:bindings
assignees: none
milestone: none
projects: none
parent: 677
children: none
synced: 2026-10-30T08:02:17.432Z
---

# [Issue 711]: [feat(cli): ls/tree/read/sql on logical table names (ADR-0064 PR C1)](https://github.com/vig-os/tessera/issues/711)

PR C1 of ADR-0064's implementation plan (§7): the CLI read surfaces on logical names.

**Scope:**
- `ls FILE`: one line per partitioned table (`pairs/  table, k blocks, N rows`); `ls FILE pairs`: columns and leaves.
- `tree`: one node per logical table, leaves under `--full`.
- `read` / `sql`: answer `pairs` (through PR B); the "not a table" guard (`nav.rs:1178`) also answers a node.

**Acceptance (each fails before on a partitioned fixture):** the `ls`, `tree`, `read`, `sql` outputs above, as
trycmd cases.

---
Design: ADR-0064 "Logical tables over numbered leaves" (`docs/adr/0064-logical-tables-over-numbered-leaves.md`,
Proposed, PR for #677). Implement only after the ADR is Accepted. Sub-issue of #677; order A → B → C1, C2 → D → E → F → G → H
(D lands only after B, C1 and C2, so no writer on `dev` writes a form a reader on `dev` can't read).

