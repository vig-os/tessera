---
type: issue
state: closed
created: 2026-09-28T20:04:25Z
updated: 2026-09-29T05:39:18Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/457
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:30.644Z
---

# [Issue 457]: [table blocks cannot carry a nullable `str` or `b1` column](https://github.com/vig-os/tessera/issues/457)

`tessera-io::table::validate_nullable` rejects a validity mask on `ColumnData::Utf8` and `ColumnData::Bool`: nullability currently covers the fixed-width numeric dtypes only.

Found while building #386's generic table lane, where it is reachable from ordinary user data — a Parquet or CSV with a nullable string column is completely unremarkable. #386 handles it by **rejecting at the canonicalisation boundary** with the column named and `--exclude` / fill-the-nulls / `ingest blob` offered, rather than letting it surface from deep inside the codec with a message naming neither the column nor a remedy. That is a good error, but it is still a refusal of valid data.

Scope:

- extend the Vortex encode/decode path to carry a validity mask for `str` and `b1`
- drop the boundary rejection in `tessera-ingest::canonical::TableBuilder::push` and its test
- the `#330` corpus-stability argument applies: a non-nullable column's bytes must not move, so existing goldens must be untouched
- a corpus fixture with a nullable string column (ADR-0056 §12a(e) records why there is not one today)

Not urgent — the workaround is real and the error is clear — but it is a hole in the table primitive's dtype envelope, in the same family as #418's array-dtype work.
