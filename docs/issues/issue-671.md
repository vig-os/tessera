---
type: issue
state: open
created: 2026-10-25T11:18:58Z
updated: 2026-10-25T11:18:58Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/671
comments: 0
labels: feature, priority:low, effort:medium, area:io
assignees: none
milestone: none
projects: none
parent: 654
children: none
synced: 2026-10-26T07:24:20.004Z
---

# [Issue 671]: [feat(query): positional zip over a Collection (ADR-0062 §5)](https://github.com/vig-os/tessera/issues/671)

Deferred by ADR-0062 §5/§6 (#654). Positional zip (PR2) zips row-aligned blocks of single products. Zipping
across a Collection (each member's table zipped with its own aligned derived table, then concatenated) is not
in PR1–PR3.

**Needs:** pairing rules (which member's derived product aligns with which source, by `sources` edge and the
recorded source digest), refusal when a member lacks its partner, and partitioning so each unit zips one
aligned pair. Builds on PR2 (zip) and PR3 (collections).

