---
type: issue
state: open
created: 2026-10-18T23:15:04Z
updated: 2026-10-19T00:48:33Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/633
comments: 0
labels: bug, priority:medium, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:41.923Z
---

# [Issue 633]: [fix(io): streaming live index folds empty stats where the batch index errors](https://github.com/vig-os/tessera/issues/633)

## Defect

`TableStreamWriter::with_live_index(col)` folds `ChunkStats::identity()` for a row-group whenever `col`
cannot supply integer stats — the column is missing, is a float/bool/string column, or is a `u64` column
holding a value above `i64::MAX` (`accumulate.rs`, `flush`: `.and_then(|(_, c)| c.chunk_stats())
.unwrap_or_else(ChunkStats::identity)`). The batch `table_chunk_index` over the same rows returns an
**error** in each of those cases.

So the module's documented contract — the live fold "reconciles exactly with a batch `table_chunk_index`
over the same rows" — fails silently: the live aggregate reports `count == 0` with no `min`/`max`, which
pruning reads as "this group holds nothing" and skips. A wrong stat silently prunes real data away.

## Fix direction

Validate the stat column in `with_live_index` (exists, integer dtype) and make `flush` propagate a
per-group failure (the `u64 > i64::MAX` case can only be seen per group) as an `Error` instead of folding
the identity. `with_live_index` returning `Result` is a public API change, so decide that shape first.
Test: each of the three cases errors on both the batch and the streaming path.

Found while fixing #627 (out of scope there: a different root cause).

