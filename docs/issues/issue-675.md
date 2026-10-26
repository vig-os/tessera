---
type: issue
state: open
created: 2026-10-25T11:19:04Z
updated: 2026-10-25T11:19:04Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/675
comments: 0
labels: question, priority:medium, area:core
assignees: none
milestone: none
projects: none
parent: 654
children: none
synced: 2026-10-26T07:24:17.799Z
---

# [Issue 675]: [question: record the indexed column in the table .cidx spec in the ADR-0059 event (ADR-0062)](https://github.com/vig-os/tessera/issues/675)

Open question from ADR-0062 §4 (#654), for the owner and the ADR-0059 event (#539, #347).

A table `.cidx` does not record which column it indexes, so tier-1 pruning trusts the `row_index` convention
(`tessera-io/src/multiblock.rs:280-284`). ADR-0059's format event is the cheapest moment to add the indexed
column's name to the table `.cidx` spec, and to decide whether a table may carry indexes on more than one
column.

**Recommendation:** add the column name in the ADR-0059 event; leave multi-column table indexes out until a
consumer asks.

