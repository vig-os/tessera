---
type: issue
state: open
created: 2026-10-18T11:52:48Z
updated: 2026-10-18T11:54:01Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/597
comments: 0
labels: feature, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:54.195Z
---

# [Issue 597]: [[ADR-0060 P1b] feat(cli,ingest): write actors from the CLI and ingest specs](https://github.com/vig-os/tessera/issues/597)

Phase **P1b** of [ADR-0060](https://github.com/vig-os/tessera/blob/dev/docs/adr/0060-agentic-actors-and-prov-mapping.md) §11 (Accepted, merged in #586). Parent: #564.


- **Scope:** the staging `Header.actors` (§10.2); `[[product.actors]]`; `ingest --actors @file`; the
  ingest re-seal copying incoming actors and the early-return guard (§10.1); `commit
  --add-actor/--keep-actors/--remove-actor` (§10.4).
- **Acceptance:** a spec whose only provenance is `[[product.actors]]` seals them →
  `spec_with_only_actors_seals_them`. Incoming `--actors` survive the re-seal →
  `ingest_reseal_keeps_incoming_actors`. A resumed stage with actors is refused by a header without
  the field → `stage_header_refuses_unknown_actors`. A `--keep-actors` commit keeps the implementer,
  drops the approver, and accepts a re-added approver → a trycmd case. A dangling delegation is
  refused, naming the actor.
- **Depends on:** P1a.

## Dependencies

#587 → #573 → #574 → #596 → P1b → P1e → P1c → P1d → P2a → P2b → P3 → P3d → P3c

Do not dispatch before its predecessors merge.

