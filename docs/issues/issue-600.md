---
type: issue
state: open
created: 2026-10-18T11:53:03Z
updated: 2026-10-18T11:54:07Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/600
comments: 0
labels: feature, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:53.141Z
---

# [Issue 600]: [[ADR-0060 P1d] feat(io,cli): the `aux/actors.json` member](https://github.com/vig-os/tessera/issues/600)

Phase **P1d** of [ADR-0060](https://github.com/vig-os/tessera/blob/dev/docs/adr/0060-agentic-actors-and-prov-mapping.md) §11 (Accepted, merged in #586). Parent: #564.


- **Scope:** writer, `--actors-aux`, `inspect --full` join and orphan warning; the writer refuses
  `host`/`user`/`login`/`ip` keys and email-shaped values (§4).
- **Acceptance:** adding or removing it leaves both hashes byte-identical →
  `aux_actors_is_outside_the_seal`. A `host` key is refused → `aux_actors_refuses_personal_keys`. An
  orphan key → a warning in a trycmd case.
- **Depends on:** P1a.

## Dependencies

#587 → #573 → #574 → #596 → #597 → #598 → #599 → P1d → P2a → P2b → P3 → P3d → P3c

Do not dispatch before its predecessors merge.

