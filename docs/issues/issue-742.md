---
type: issue
state: open
created: 2026-11-03T00:46:38Z
updated: 2026-11-03T00:46:38Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/742
comments: 0
labels: priority:low, area:testing
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-03T08:59:44.170Z
---

# [Issue 742]: [io(multiblock): .cidx pruning test bounds bytes, not the leaves read; CHANGELOG for the StrayMarker name](https://github.com/vig-os/tessera/issues/742)

Follow-up nits from the review of PR #739 (#710 B2), head a4390d9.

1. `tessera-io/src/multiblock.rs:459`: `read < smallest_leaf` stands in for "no leaf payload was read". A partial read of a pruned leaf, such as a footer probe, would still pass. Assert the claim itself: record which archive members, or which byte ranges by member, the reader touched, and require that no pruned leaf's member appears. Show it fails when the prune is disabled.
2. The reader's `StrayMarker` error now names the marked block, not the queried name (`tessera-core/src/block/partition.rs:224-229`). Add a CHANGELOG line.
3. (Record only) `a4390d9` is typed `docs:` but also adds a core test. It is already on dev (squash merges are disabled); nothing to do.

Refs: #710
