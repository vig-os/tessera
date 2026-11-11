---
type: issue
state: open
created: 2026-11-11T04:18:30Z
updated: 2026-11-11T04:18:43Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/835
comments: 0
labels: feature, area:bindings
assignees: none
milestone: none
projects: none
parent: 799
children: none
synced: 2026-11-11T08:50:24.995Z
---

# [Issue 835]: [feature(capi): metadata, provenance, aux provenance and identity, finish/free (ADR-0065 §9)](https://github.com/vig-os/tessera/issues/835)

Part of epic #799. Design: ADR-0065 (docs/adr/0065-c-abi-bridge.md, PR #819). **Blocked until the owner accepts ADR-0065**; do not start before. Toward #800 (split from #814).

## What
- `set_study`, `set_field`, `set_extra`, `add_source`, `set_producer`, `set_generation`: held and validated by the bridge, applied at `finish` through `StreamWriter::finish_with`;
- `finish`: closes nothing implicitly (every table must be closed), seals, writes `aux/provenance.json` (`stamp_ingest_provenance`) and, if set, the ADR-0047 identity envelope via `tessera-identity`; `free` without `finish` aborts (`StreamWriter::abort`) and removes the staging directory.

## Acceptance
- Metadata set after the last push is sealed.
- `requires_generation` refuses a product without a generation at finish, writing no file.
- The identity envelope round-trips with `tessera reidentify`, and adding it leaves `content_hash`/`manifest_hash` unchanged.
- `free` without `finish` leaves no output and no staging directory, with every thread joined.

Size: ≤ ~350 lines. Depends on: #814, the finish_with/abort issue, PR #786.
