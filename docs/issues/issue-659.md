---
type: issue
state: closed
created: 2026-10-25T09:27:33Z
updated: 2026-10-25T14:38:56Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/659
comments: 1
labels: bug, priority:high, effort:small, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-26T07:24:24.043Z
---

# [Issue 659]: [fix(io): decode returns codec fill under a NULL, not the documented dtype default](https://github.com/vig-os/tessera/issues/659)

## Defect

The `ColumnData::Nullable` rustdoc promises: "a decoded column carries `0` (or `0.0`) wherever `validity`
is false". ADR-0056 §5 / `tessera-ingest::canonical::normalise_null_slots` repeat it ("reads back `0.0`
with `validity = false`").

Decode does not keep that promise. `extend_field` copies the decompressed Vortex buffer verbatim, and
the Vortex compressor is free to put any value under a null slot to improve compression. Encode itself
is fine: it writes `0` under every NULL, and the file bytes are deterministic. Reproducer: a nullable
`i8` column `k = r*3 - 7` that is NULL at every 5th row decodes row 0 (a NULL) as `-4`, the neighbouring
present value, not `0`.

## Reach

Every decode entry point that materialises a nullable column (`decode`, `decode_projected`,
`decode_column`, `decode_rows`, and so the Python and CLI reads). Present values and the mask are exact,
so code that honours `validity` is unaffected. Code that compares whole columns with `==`, hashes the
decoded values (e.g. recomputing a chunk-index digest from a decoded group, #632), or exports the raw
buffer sees codec fill instead of the documented default. The existing round-trip test passes only
because its data happens not to trigger a fill.

Found while writing the #632 regression tests.

## Fix

Normalise masked slots to the dtype default in the nullable decode arm (one pass over the appended
range), reusing the same masked-slot helper the #632 digest uses. Read side only: no stored bytes,
digests or goldens move. Test: a column that triggers the codec fill decodes to `0` under every NULL,
on every read entry point.

---

# [Comment #1]() by [gerchowl]()

_Posted on October 25, 2026 at 02:38 PM_

Fixed by #662 (commit b3f85db, merged 68ea65b): decode yields the dtype default under every NULL, per the `ColumnData::Nullable` contract. Test: `decode_yields_the_dtype_default_under_every_null`.

