---
type: issue
state: open
created: 2026-10-18T23:15:03Z
updated: 2026-10-19T00:48:30Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/632
comments: 0
labels: bug, priority:high, area:io, area:integrity
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:42.242Z
---

# [Issue 632]: [fix(io): table chunk-index digest depends on the bytes under a NULL](https://github.com/vig-os/tessera/issues/632)

## Defect

The table chunk-index row-group digest is "every column's LE bytes for the group, in column order"
(`table_chunk_index`, and the streaming live fold in `TableStreamWriter::flush`). For a nullable column
those bytes are `ColumnData::to_le_bytes` = `values ‖ packed validity`, and `values` is the **raw** buffer,
including whatever the producer left under each NULL.

`encode` normalises every masked slot to the dtype default (the `ColumnData::Nullable` rustdoc: "Values
under a NULL are not preserved … makes the encoded bytes — and therefore `content_hash` — independent of
whatever happened to sit under a null"). The chunk index does not. So two producers writing the same
logical table — same present values, same validity, different bytes under a NULL — get a byte-identical
table block but **different chunk-index roots**, hence a different `.cidx` sidecar block and a different
`content_hash`.

## Reach

Not reachable through `tessera ingest`: the ingest boundary zeroes masked slots (ADR-0056 H5,
`tessera-ingest/src/canonical.rs`). Reachable through the library/binding write API whenever the caller
hands over a `ColumnData::Nullable` whose masked slots are not the default, with a chunk-index stat column
set (`table_block_with_index` / `stream::…` / `TableStreamWriter::with_live_index`).

Found while fixing #627 (which made a nullable *stat* column work; a nullable *non-stat* column already fed
the digest before #627).

## Fix direction (needs a decision)

Normalise masked slots to the dtype default in the digest pre-image (or in `to_le_bytes` itself, which also
feeds fragments and the Python zero-copy export), so the chunk index honours the same invariant as `encode`.
Moves the chunk-index root for any existing product whose nullable columns carried non-default bytes under
a NULL, so it needs a determinism call (and a corpus check) rather than a drive-by. Add a test: two tables
differing only under a NULL → equal `encode` bytes AND equal `table_chunk_index` root.

