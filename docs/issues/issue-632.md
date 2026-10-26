---
type: issue
state: closed
created: 2026-10-18T23:15:03Z
updated: 2026-10-25T14:38:52Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/632
comments: 2
labels: bug, priority:high, effort:small, area:io, area:integrity
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-26T07:24:31.109Z
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

---

# [Comment #1]() by [gerchowl]()

_Posted on October 25, 2026 at 09:11 AM_

**Triage (architect, 2026-10-25): decided, ready to implement. No ADR needed.**

Decision: normalise masked slots to the dtype default **in the chunk-index digest pre-image** (batch `table_chunk_index`, the streaming live fold in `TableStreamWriter::flush`, and the multi-block path). This applies the invariant that `ColumnData::Nullable` and `encode` already document: values under a NULL are not identity. It isn't a new format rule.

`to_le_bytes` stays as it is, because it also feeds the fragments and the Python zero-copy export, and changing it would widen the blast radius beyond the defect. If a reviewer finds another identity-bearing consumer of the raw bytes under a NULL, that consumer gets the same normalisation, listed in the PR.

Acceptance:
- **Regression test:** two tables that differ only under a NULL give equal `encode` bytes and an equal chunk-index root, on the batch, streaming and multi-block paths. Show the pre-fix failure.
- **Corpus:** goldens are unchanged. Ingest already zeroes masked slots (ADR-0056 H5), so any golden that moves is a finding to report, not something to regenerate past.
- **CHANGELOG:** state that a library-written product with non-default bytes under a NULL gets a new chunk-index root (and `content_hash`).

Order: first in the next wave (small, high).


---

# [Comment #2]() by [gerchowl]()

_Posted on October 25, 2026 at 02:38 PM_

Fixed by #662 (merged 68ea65b): masked slots are normalised to the dtype default in the chunk-index digest pre-image through one shared helper, on the batch, streaming and multi-block paths. `to_le_bytes` is unchanged and corpus goldens didn't move. A library-written product with non-default bytes under a NULL gets a new chunk-index root. The recipe stays `chunk_index@1`; the reasoning is at the recipe constant.

