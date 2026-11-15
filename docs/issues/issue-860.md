---
type: issue
state: open
created: 2026-11-15T02:18:33Z
updated: 2026-11-15T02:18:33Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/860
comments: 0
labels: area:testing, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-15T08:48:28.907Z
---

# [Issue 860]: [test(io): open_table follow-ups from the #847 review (1-row cases, heap bytes, crash doc)](https://github.com/vig-os/tessera/issues/860)

Nits from the fresh review of #847 (#806). Not blocking.
- Mutant B (`rows_of(&buf) > 0` → `> 1`) makes a 1-row table seal with `rows: 0`, silently losing the row. Only the lookahead test catches it. Add row counts 1 and k·ROWS_PER_GROUP+1 to `open_table_seals_…_at_every_leaf_count`.
- The new tests assert staged *blocks* (W+1), not heap *bytes*. Add a heap-bytes assertion, or document that push_table's memory test covers it.
- The `OpenTable` rustdoc never mentions crash/recover. Add a line pointing to ADR-0065 §3: recovery yields a prefix of whole leaves, not a resumable product.

Refs: #806
