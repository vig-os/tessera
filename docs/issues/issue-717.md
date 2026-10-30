---
type: issue
state: open
created: 2026-10-30T01:19:25Z
updated: 2026-10-30T01:19:44Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/717
comments: 0
labels: feature, area:bindings
assignees: none
milestone: none
projects: none
parent: 677
children: none
synced: 2026-10-30T08:02:14.477Z
---

# [Issue 717]: [feat(py): Python table reads on logical table names (ADR-0064 PR C2)](https://github.com/vig-os/tessera/issues/717)

PR C2 of ADR-0064's implementation plan (§7): the Python read surfaces on logical names.

**Scope:** `read_table`, `read_table_column` (`tessera-py/src/lib.rs:116-153`) resolve the name through
`Reader::table_group` (a partitioned table concatenates its leaves); `column`, `table_dict`, `table_arrow`, `table`
(`python/tessera/__init__.py:106-133`) follow; `read_block` on a node is a typed "not a block" error; the
`logical_column` docstring, `docs/book` bindings (`write_example.py` anchors) and `tests/api_drift.py`.

**Acceptance (fails before):** `tessera.open(p).table("pairs")` returns every row of a partitioned fixture and
`read_block("pairs")` raises the typed error; the bindings doc and `api_drift` pass.

---
Design: ADR-0064 "Logical tables over numbered leaves" (`docs/adr/0064-logical-tables-over-numbered-leaves.md`,
Proposed, PR for #677). Implement only after the ADR is Accepted. Sub-issue of #677; order A → B → C1, C2 → D → E → F → G → H
(D lands only after B, C1 and C2, so no writer on `dev` writes a form a reader on `dev` can't read).

