---
type: issue
state: open
created: 2026-11-25T04:18:19Z
updated: 2026-11-25T04:18:19Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/987
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-25T10:51:43.674Z
---

# [Issue 987]: [perf(repo): the cost of dropping a short-lived collection lineage — measure, then choose reclaim-on-forget or an overlay](https://github.com/vig-os/tessera/issues/987)

Deferred from the #947 ADR (ADR-0067, Proposed), decision D7. Feature request with a measurement first.

## Motivation

kit drops tens of short-lived workspace lineages per document per day (#947, second comment, item 3). Dropping one is
`forget` on its lineage followed by `gc`. `gc` marks from every root and then lists every object in `objects/`, so the
cost of discarding a workspace grows with the repository, not with what the workspace wrote.

## What the ADR fixes and what it leaves to this issue

ADR-0067 fixes the semantics any faster path must keep: after a drop, the set of stored objects equals what a full
`gc` would leave, retention (#906) and WORM holds included, under the one fenced writer (ADR-0036, 2026-11-24
amendment). It does not choose the mechanism, because the choice depends on numbers nobody has.

## Options to measure

1. **Full `gc` after `forget`** (today). Mark cost = the versions and members reachable from all roots; sweep cost =
   every object.
2. **Reclaim on forget.** Mark from every other root, then delete only the forgotten lineage's closure minus the
   mark. Skips the sweep's listing; the mark of the other roots remains.
3. **A per-workspace object area** that reads fall back from to the main one and that is dropped as a whole. A
   change to the repository layout (ADR-0036 §1) and to the durability order.

## Acceptance

- [ ] The probe from #947's comment: create and drop 1,000 workspaces of about 200 rows each in a repository of
  10^7 rows; report time and bytes of each discard for options 1 and 2 (3 if 1 and 2 both grow with the repository).
- [ ] A decision recorded as an amendment to ADR-0067 D7, and the chosen path built with a test that its result
  equals a full `gc`.

## Related

#947, #781, #906, #905; ADR-0036, ADR-0067.

