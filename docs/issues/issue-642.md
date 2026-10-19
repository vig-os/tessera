---
type: issue
state: open
created: 2026-10-19T01:15:55Z
updated: 2026-10-19T01:15:55Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/642
comments: 0
labels: refactor, priority:low, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:39.591Z
---

# [Issue 642]: [refactor(io): as_i64 validity-length contract and the double materialisation in table_block_with_index](https://github.com/vig-os/tessera/issues/642)

Follow-ups from the #634 review (#627), not blocking:

1. `table.rs:229`: the public `ColumnData::as_i64` now indexes `validity[i]`, so a `Nullable` column whose mask is shorter than its values and that skipped `validate` panics, where it used to truncate silently. Every in-tree caller validates first. Make the contract explicit (a length check that returns a typed error, or a documented precondition plus a debug-assert).
2. `table.rs:1572`: `table_block_with_index` materialises the whole column through `as_i64` only to check that it is an integer column, then builds it again per row group. This cost predates #634; check the dtype instead.

