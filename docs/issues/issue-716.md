---
type: issue
state: open
created: 2026-10-30T01:12:02Z
updated: 2026-10-30T01:19:50Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/716
comments: 0
labels: feature, area:docs
assignees: none
milestone: none
projects: none
parent: 677
children: none
synced: 2026-10-30T08:02:15.070Z
---

# [Issue 716]: [docs: tell MorePET/visionx-rs about push_table and pairs/NNNN (ADR-0064 step H)](https://github.com/vig-os/tessera/issues/716)

Step H of ADR-0064's implementation plan: tell the #677 consumer.

**Scope:** after PR G passes, comment on #677 and open an issue on MorePET/visionx-rs describing the switch from
`table_job_streaming` to `StreamWriter::push_table`, the new leaf names `pairs/NNNN` (and that `pairs` stays the one
logical name for `logical_table`, `read`, `sql` and the Python `table`/`logical_column`), that `table_job_streaming`
refuses tables above `BLOCK_ROWS`, the measured RSS bound, and the tessera release that carries it.

**Acceptance:** the visionx-rs issue is linked from #677 and names the release; #677 closes after G and H.

---
Design: ADR-0064 "Logical tables over numbered leaves" (`docs/adr/0064-logical-tables-over-numbered-leaves.md`,
Proposed, PR for #677). Implement only after the ADR is Accepted. Sub-issue of #677; order A → B → C1, C2 → D → E → F → G → H
(D lands only after B, C1 and C2, so no writer on `dev` writes a form a reader on `dev` can't read).

