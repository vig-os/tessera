---
type: issue
state: closed
created: 2026-10-25T09:24:08Z
updated: 2026-10-25T14:38:54Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/658
comments: 1
labels: bug, priority:high, effort:small, area:io, area:integrity
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-26T07:24:24.486Z
---

# [Issue 658]: [fix(io): encode_streaming drops Column::nullable — streamed nullable tables encode silently wrong](https://github.com/vig-os/tessera/issues/658)

## Defect

`table::encode_streaming` derives the Vortex struct dtype from `empty_column(&c.dtype)`, which ignores
`Column::nullable`. For a spec with a nullable column the declared dtype says `k=i64` while every chunk
built by `ColumnData::Nullable::to_vortex` is `k=i64?`.

Inside vortex the mismatch panics in the write task: the `ArrayStreamAdapter` `debug_assert_eq!` in debug
builds, and a hard `assert_eq!` in vortex-layout's `SequentialStreamAdapter` (`sequence.rs:312`) in release
builds. The panic does not reach the caller, so `encode_streaming` returns `Ok` with bytes that differ from
`encode` of the same rows, **in both profiles**. (An earlier version of this issue said release builds write
the mismatch unchecked. That was wrong: review established they panic too.)

## Reach

Every streamed write of a table with a nullable column: `TableStreamWriter::finish` and
`TableMultiBlockSink` (via `table_job_from_fragments`). The batch `encode` is unaffected. The existing
nullable streaming tests (#627) exercise only the live index and never call `finish`, so nothing caught it.
`tessera ingest` is not affected today: its streamed products (GE listmode) carry no nullable columns.

Found while writing the #632 streaming regression test.

**Existing data:** a streamed nullable table sealed by v0.1.0-alpha.2 or alpha.3 (nullable columns landed in #330, after alpha.1; library API only) carries
these wrong bytes. Re-write such products from their source after this fix. Ingest is not affected.

## Fix

Build the dtype from `ColumnData::empty_for(column)` (nullable-aware). Also refuse a chunk whose dtype
differs from the declared one with an error the caller sees, instead of relying on a panic inside vortex. Test: `encode_streaming` over nullable columns (multi
row-group, NaN present, non-default values under NULLs) equals the batch `encode` bytes and decodes.

---

# [Comment #1]() by [gerchowl]()

_Posted on October 25, 2026 at 02:38 PM_

Fixed by #662 (commit a0fd4ee, merged 68ea65b): `encode_streaming` builds the stream dtype from the declared columns, nullability included, and checks each chunk's dtype against it. Regression tests are `encode_streaming_*`.

