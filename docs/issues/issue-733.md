---
type: issue
state: closed
created: 2026-11-02T12:19:22Z
updated: 2026-11-03T00:46:44Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/733
comments: 1
labels: bug, priority:medium
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-03T08:59:46.355Z
---

# [Issue 733]: [core(partition): typed error for forged leaf indices; read path and seal disagree on nested names with a stray marker](https://github.com/vig-os/tessera/issues/733)

Follow-up from the review of PR #732 (#709 A2), head fbe9978. These do not block #709, but they leave the error type and the read/seal contract inconsistent.

1. **Untyped error.** `tessera-core/src/block/partition.rs:325`: `leaf_name(node, p.index)?` can return a bare `Error::InvalidBlockName` when a long node name has a forged large index. Every resolver failure should be `Error::LogicalTable { reason }`; map this one to `NonCanonicalLeaf`. Add a test that fails before the fix.
2. **Read and seal disagree.** `partition.rs:225`: the read path skips the `StrayMarker` check for any name containing `/`. `table_group("a/pairs")` returns `LegacyFlat` with a marked `a/pairs_0001`, while `seal` refuses the same product through node `a`. ADR-0064 §2.4's literal wording is "no parent node". Decide one rule (an ADR-0064 clarification if needed), apply it on both paths, and pin it with a read-and-seal test.
3. **`DuplicateIndex` can't be reached in practice.** It needs two blocks with the same name, and `seal`'s leaf rule does not refuse duplicate block names. Check whether ADR-0063 forbids duplicate names; if it does, enforce that at seal (separately filed if larger).

Refs: #709
---

# [Comment #1]() by [gerchowl]()

_Posted on November 2, 2026 at 12:27 PM_

Item 3: ADR-0063 does not state that block names are unique, and `ProductBuilder::seal` does not refuse a duplicate (only the write session does). Filed separately as #734 (not fixed in the ADR-0064 PR B branch). Items 1 and 2 are fixed on `feature/710-table-group-readers`.

