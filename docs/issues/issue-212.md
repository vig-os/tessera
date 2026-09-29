---
type: issue
state: closed
created: 2026-06-26T11:40:04Z
updated: 2026-09-28T17:54:10Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/212
comments: 1
labels: area:io, area:bindings
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:04.735Z
---

# [Issue 212]: [tessera-py: expose Vortex table column projection (read_table_column)](https://github.com/vig-os/tessera/issues/212)

## Context
The #143 cross-ecosystem bench showed Tessera's table **column read ≈ full read** (1336 vs 1180 MB/s) because the Python binding read the whole Vortex block and selected one column, while Parquet/ROOT/HDF5 do real per-column projection (2.3–2.8 GB/s).

The Vortex backend supports projection (`ScanBuilder::with_projection(select([col], root()))`); it just wasn't wired through.

## Done-when
- `tessera_io::table::decode_column(spec, blob, name)` reads only the projected column's segments, bit-exact with the matching column of `decode`.
- `tessera-py` `Reader.read_table_column(block, column)` exposes it.
- Bench shows a real speedup over full-read.

## Result
Landed: column read **1336 → 4493 MB/s** (3.4×), now the fastest column read among all formats benched. Rust test `decode_column_projection_matches_full_decode` + py smoke projection assertion + bench updated.
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: commit b819d3b 'feat(io,py): table column projection — read one column, not the whole block (#212)'.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

