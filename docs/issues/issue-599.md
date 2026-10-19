---
type: issue
state: open
created: 2026-10-18T11:52:58Z
updated: 2026-10-18T11:54:05Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/599
comments: 0
labels: feature, python, area:bindings
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:53.474Z
---

# [Issue 599]: [[ADR-0060 P1c] feat(py): actors and `tessera.digest` in the Python bindings](https://github.com/vig-os/tessera/issues/599)

Phase **P1c** of [ADR-0060](https://github.com/vig-os/tessera/blob/dev/docs/adr/0060-agentic-actors-and-prov-mapping.md) §11 (Accepted, merged in #586). Parent: #564.


- **Scope:** `add_software_agent`/`add_person`/`add_organization`, `tessera.digest`,
  `Reader.actors`, the `write_example.py` anchor, `bindings.md` and `api_drift.py` (§10.3).
- **Acceptance:** the anchor writes and reads back the §5.1 chain, with `brief` from
  `tessera.digest`. `tessera.digest(b"")` equals the Rust digest. A bad kwarg is a `TypeError`. A
  dangling delegation at `pack` is a `TesseraError` naming index and id. The `tessera-py-import` check
  passes.
- **Depends on:** P1a, #562.

## Dependencies

#587 → #573 → #574 → #596 → #597 → #598 → P1c → P1d → P2a → P2b → P3 → P3d → P3c

Do not dispatch before its predecessors merge.

