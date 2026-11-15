---
type: issue
state: open
created: 2026-11-15T02:18:16Z
updated: 2026-11-15T02:18:16Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/859
comments: 0
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-15T08:48:29.277Z
---

# [Issue 859]: [tessera-io: group_rows doesn't check dtypes, so a wrong-typed full row-group fails late with a misleading 'decoded N rows' error](https://github.com/vig-os/tessera/issues/859)

Found by the fresh review of #847 (#806), 2026-11-15. This is pre-existing: `push_table` behaves the same way.

A u64 column pushed for a u2-declared spec, in a FULL row-group of 65,536 rows, is accepted: `push` returns Ok and `close` returns Ok(1). It fails only at `finish`, with `fragment: column 'e' decoded 262144 rows, header says 65536`, which misdiagnoses the problem. Off the grid, the same group fails at once with `extend: dtype u8 != u2`.

Cause (per the review): `group_rows` checks rows, not dtypes, on the fast path for grid-aligned groups.

Fix: check each column's dtype against the spec in `group_rows`, on every path, as a typed `StreamedTableError` at `push`. Test: a full row-group with the wrong dtype is refused at push, for both `push_table` and `open_table`. Show that it fails before the fix.

Refs: #806
