---
type: issue
state: open
created: 2026-10-18T11:53:13Z
updated: 2026-10-18T11:54:11Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/602
comments: 0
labels: feature, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:52.292Z
---

# [Issue 602]: [[ADR-0060 P2b] feat(export): actors in the DataCite projection](https://github.com/vig-os/tessera/issues/602)

Phase **P2b** of [ADR-0060](https://github.com/vig-os/tessera/blob/dev/docs/adr/0060-agentic-actors-and-prov-mapping.md) §11 (Accepted, merged in #586). Parent: #564.


- **Scope:** §9.3: contributors, the `TechnicalInfo` summary, the same refusals as P2a, and the book
  sentence.
- **Acceptance:** a named person → `Personal` + ORCID scheme. A name-less person → not a
  contributor, but present in the summary. A software agent → never a contributor. Invalid or newer
  actors → refused. The mandatory-field test stays green.
- **Depends on:** P2a (same file, serialised to avoid conflicts).

## Dependencies

#587 → #573 → #574 → #596 → #597 → #598 → #599 → #600 → #601 → P2b → P3 → P3d → P3c

Do not dispatch before its predecessors merge.

