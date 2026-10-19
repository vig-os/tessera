---
type: issue
state: closed
created: 2026-10-18T22:23:56Z
updated: 2026-10-19T01:55:38Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/627
comments: 1
labels: bug, priority:high, area:core
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:43.367Z
---

# [Issue 627]: [fix(core): table_chunk_index panics on a nullable column with nulls (range end index out of range)](https://github.com/vig-os/tessera/issues/627)

**Repro (minimal):** a table with 3 rows, 1 of them null, in a nullable `i8` column that gets chunk statistics. Building the table's chunk index panics:

```
range end index 3 out of range for slice of length 2
  at table.rs:1487:47 (table_chunk_index)
```

`cargo test` exits with rc=101. Observed at commit `e771a9c`. The stats path seems to index the non-null values with the full row count. Real-data trigger: an event table with optional string and integer columns.

**Expected:** nulls are counted in the stats (null_count) and excluded from min/max, and no panic. This is related to the chunk-index work in #532 / #522 / #523.
---

# [Comment #1]() by [gerchowl]()

_Posted on October 19, 2026 at 01:55 AM_

Fixed by #634 (merge e244940): chunk statistics now slice each row group before taking stats and apply validity before the u64 range check, on the batch, streaming and multi-block paths. NULLs are excluded from min/max, and an all-NULL chunk uses the empty-chunk identity. Follow-ups: #632 (digest over the bytes under a NULL), #633, #642. Ships in 0.1.0-alpha.2.

