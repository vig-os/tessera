---
type: issue
state: open
created: 2026-10-30T01:07:07Z
updated: 2026-10-30T01:07:07Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/708
comments: 0
labels: documentation
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-30T08:02:19.115Z
---

# [Issue 708]: [docs(adr): ADR-0062 gate/slot rules — any pre-pass error, panic resolves the gate, orphaned-slot owner (before #667/#668)](https://github.com/vig-os/tessera/issues/708)

These are the three nits from the final fresh review of PR #698 (ADR-0062 Accepted, head 4ed6135). The review found no
blockers. Two of the nits are rules the implementation needs, so this issue **must be resolved by an ADR-0062
amendment before #667 (PR2) and #668 (PR3) implement the release gate and the spool**.

1. **The gate's early-resolve list names only key-check failures** (`docs/adr/0062-streaming-table-provider.md` ~:785-786:
   tie, inversion, NULL, mismatch). Fill/integrity, I/O and decode errors in a pre-pass are covered only by the
   general phrase "first pre-pass … that fails". Add "or any error" to the list. Add a PR2 "Gate failure" case with a
   corrupt block in partition 2's run.
2. **A pre-pass that panics.** No rule says a producer thread that panics resolves the gate to an error. Without it, the
   other partitions wait forever. Specify a drop guard (or the equivalent) that resolves the gate on unwind. Add a test:
   a pre-pass panics, and the query returns an error within the timeout.
3. **Who frees an orphaned slot?** ~:788 and ~:1311 (the PR3 test asserting no slot remains once the fill pool is idle)
   imply that a slot whose fill finishes after its requesting producer has stopped gets freed. No rule says by whom.
   State the owner, e.g. the fill completion releases a slot whose cell has no live holder, and point the PR3
   assertion at that rule.

