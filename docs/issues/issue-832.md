---
type: issue
state: open
created: 2026-11-11T04:18:26Z
updated: 2026-11-11T04:29:48Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/832
comments: 0
labels: feature, area:io
assignees: none
milestone: none
projects: none
parent: 799
children: none
synced: 2026-11-11T08:50:26.881Z
---

# [Issue 832]: [feature(io): lanes on StreamWriter and WriteSession, sealed in lane order (ADR-0065 §7.3)](https://github.com/vig-os/tessera/issues/832)

Part of epic #799. Design: ADR-0065 (docs/adr/0065-c-abi-bridge.md, PR #819). **Blocked until the owner accepts ADR-0065**; do not start before. Toward #800 / #802 (ADR-0065 §7.3).

## What
Lanes on `StreamWriter` and `WriteSession`, so one product holds several independently streamed tables without one waiting for another:
- a job is submitted on a lane (one lane per table role, plus one for arrays); each lane has its own sequence counter, and the committer commits each lane's jobs in that lane's order, independently of the other lanes (the `commit_loop` reorder buffer keyed by `(lane, seq)`);
- the journal line records the lane, so `recover` keeps it;
- `seal` orders the manifest's blocks by lane — the table lanes by role name, then the arrays lane last — then by commit order within a lane; never by cross-lane commit timing;
- `CreditWindow`'s no-stall rule (`stream.rs:495`) becomes per lane: each lane's next job to commit is always admitted to encode;
- the seal-time integrity cross-check becomes per lane: each lane's running Merkle accumulator equals the root over that lane's digest subsequence of the sealed manifest (replacing today's single commit-order root check, `write.rs` seal).

## Acceptance
- Two lanes fed in randomised interleavings seal byte-identical products, equal to a batch `ProductBuilder` of the same blocks in lane order (fails before: today blocks seal in commit order).
- A single-lane product is byte-identical to today's (corpus untouched).
- `recover` replays a multi-lane journal and seals the same product.

Size: ≤ ~400 lines. Depends on: nothing (parallel with #806).


