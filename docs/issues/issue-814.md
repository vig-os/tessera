---
type: issue
state: open
created: 2026-11-11T01:18:48Z
updated: 2026-11-11T04:19:42Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/814
comments: 0
labels: feature, area:bindings
assignees: none
milestone: none
projects: none
parent: 799
children: none
synced: 2026-11-11T08:50:34.900Z
---

# [Issue 814]: [feature(capi): arrays over the ABI, staged raw and encoded at finish in role order (ADR-0065 §2.2)](https://github.com/vig-os/tessera/issues/814)

Part of epic #799. Design: ADR-0065 (docs/adr/0065-c-abi-bridge.md, PR #819). **Blocked until the owner accepts ADR-0065**; do not start before. Toward #800.

## What
`tsra_array_write` (`tsra_array_view`: numpy dtype code, ndim, shape, C-contiguous little-endian buffer, axes, unit, codec), checked against the schema's `arrays` role: the buffer is staged raw in the session's staging directory before the call returns, and encoded at `finish` on the arrays lane (#832), in role order. (Metadata, provenance, aux and `finish`/`free` are #835.)

## Acceptance
- A bridge-written array product equals the batch product of the same array.
- Arrays written from several threads in any order seal in role order (byte-identical across orders).
- Undeclared role, wrong dtype or rank, and a short buffer are refused.

Size: ≤ ~250 lines. Depends on: #811, #813, #832.
