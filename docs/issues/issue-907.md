---
type: issue
state: open
created: 2026-11-18T20:57:34Z
updated: 2026-11-18T20:57:43Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/907
comments: 0
labels: feature, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-19T09:52:08.226Z
---

# [Issue 907]: [feat(io): timer flush for low-rate capture — bounded staging latency (ADR-0027, unbuilt)](https://github.com/vig-os/tessera/issues/907)

Context: the DAQ discussion for the kit's acquisition sessions (2026-11-18), following #203 and #794. ADR-0027 already designs this, and `accumulate.rs` names it as "later":

> A low-rate acquisition may **flush partial fragments on a timer** (bounded durability latency), not only when the buffer is full … The flush trigger is the *only* change (timer ∨ buffer-full); the mechanism is unchanged.

It isn't built, and no issue tracks it.

## Problem

Staging is triggered only by a full row group (`ROWS_PER_GROUP = 2¹⁶` rows). `OpenTable` and `TableStreamWriter` hold up to a row group of pending rows in RAM until then.

For a slow stream, that means hours with nothing durable. At 10 Hz, one row group fills in about **1.8 h**: a crash loses that window, and #794's commit tap stays silent the whole time. A writer should never sit on data indefinitely just because the rate is low.

## Proposal

- A **flush deadline** per table or lane: `max_staging_latency: Duration`. When it elapses, the pending partial group spills to a durable staging fragment, even though the group isn't full.
- This changes **staging only**. The sealed block still re-compacts to the fixed grid, so `content_hash` and `manifest_hash` don't depend on the timer, the push sizes or the rate (ADR-0026 and ADR-0027 invariant).
- Partial fragments are fsync'd and journalled like full ones, so `recover` keeps them.
- `StreamStats` reports timer flushes separately from full-group flushes.
- **Open:** whether a timer flush also emits a #794 event, with partial-group stats, or only an ingress-tap event (#908). It must never fold a partial group into the live MMR as if it were a final leaf.

## Acceptance

- [ ] A 10 Hz synthetic stream with `max_staging_latency = 1 s` has every row durable within 1 s plus the fsync time. Test: kill and `recover`.
- [ ] Property test: `content_hash` is identical with and without timer flushes, across push sizes, rates, worker counts and windows.
- [ ] No partial group ever enters the live MMR or chunk index as a final leaf.
- [ ] The timer adds no thread per table; one deadline wheel serves the writer.

Related: #203, #794, #795, #908, #909, ADR-0026, ADR-0027.

