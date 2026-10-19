---
type: issue
state: open
created: 2026-10-18T11:53:08Z
updated: 2026-10-18T11:54:09Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/601
comments: 0
labels: feature, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:52.741Z
---

# [Issue 601]: [[ADR-0060 P2a] feat(export): actors in the RO-Crate projection](https://github.com/vig-os/tessera/issues/601)

Phase **P2a** of [ADR-0060](https://github.com/vig-os/tessera/blob/dev/docs/adr/0060-agentic-actors-and-prov-mapping.md) §11 (Accepted, merged in #586). Parent: #564.


- **Scope:** §9.2 in full: `CreateAction`, producer `SoftwareApplication`, qualified associations,
  roles, delegation (`actedOnBehalfOf` plus a scoped `prov:Delegation`), `wasDerivedFrom` /
  `wasRevisionOf`, `PropertyValue`s, the `.`-joined `@id` scheme, and refusal of invalid or newer input.
- **Acceptance:** the whole-document flattening assertion holds with actors. Actors `impl` and
  `impl-model` plus a `sampling` key `a.b` produce no duplicate `@id` → `rocrate_ids_never_collide`.
  Every actor and role is reachable from `#generation`. The approver is `agent`; software is never
  `agent`. A verifying product with a dangling `acted_on_behalf_of` → `InvalidActor`, and one with a
  newer field → `NewerFormatFields`, with no crate written → `export_refuses_invalid_or_newer_actors`.
  JSON-LD expansion against the pinned 1.1 context gives `prov#qualifiedAssociation` (vendored context).
  No sealed byte changes.
- **Depends on:** P1a, PR #541 merged (§9.5).
- **Split if needed:** (1) activity, associations and roles; (2) delegation, `PropertyValue`s and
  source edges.

## Dependencies

#587 → #573 → #574 → #596 → #597 → #598 → #599 → #600 → P2a → P2b → P3 → P3d → P3c

Do not dispatch before its predecessors merge.

