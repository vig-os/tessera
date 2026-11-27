---
type: issue
state: closed
created: 2026-11-27T03:49:03Z
updated: 2026-11-27T07:59:05Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/1021
comments: 2
labels: chore, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-27T10:30:53.943Z
---

# [Issue 1021]: [feat(io): SchemaMismatch carries the column and field that differ, typed](https://github.com/vig-os/tessera/issues/1021)

From the review of PR #1014 (#1011). `SchemaMismatch { leaf, detail }` only identifies the differing column and field (dtype, nullable, unit, scale, referencing) in free text. Callers, and the tests, can only grep that string, and every detail prints all fields for both sides. Add typed members (`column`, `field`, `expected`, `found`) to the error, which is `#[non_exhaustive]`, and keep the message the same.

Refs: #1011
---

# [Comment #1]() by [gerchowl]()

_Posted on November 27, 2026 at 05:00 AM_

Also, from the round-2 review of PR #1014: the doc comment at `tessera/crates/tessera-core/src/block/partition.rs:176` still lists only "count, name, dtype or nullability". It should also name unit, scale and referencing (#1011).

---

# [Comment #2]() by [gerchowl]()

_Posted on November 27, 2026 at 06:30 AM_

**Architect design ruling (2026-11-27):** a malformed TableSpec gets its own error variant, `MalformedTableSpec`; it is not a mismatch. `SchemaMismatch` carries `kind: SchemaMismatchKind`, one of:
- `ColumnCount { expected, found }`
- `ColumnName { index, expected, found }`
- `Field { column, field: ContractField, expected, found }`

All of these are `#[non_exhaustive]`. The comparison semantics do not change: it still decides by full equality, and now reports the first difference.

