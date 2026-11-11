---
type: issue
state: open
created: 2026-11-11T04:18:29Z
updated: 2026-11-11T04:18:42Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/834
comments: 0
labels: feature, area:io
assignees: none
milestone: none
projects: none
parent: 799
children: none
synced: 2026-11-11T08:50:25.660Z
---

# [Issue 834]: [test(io): the #802 proof: N threads, Geant4-modulo mock, window bound, negatives (ADR-0065 §7.4)](https://github.com/vig-os/tessera/issues/834)

Part of epic #799. Design: ADR-0065 (docs/adr/0065-c-abi-bridge.md, PR #819). **Blocked until the owner accepts ADR-0065**; do not start before. Implements the #802 proof (split from #812; ADR-0065 §7.4).

## What
The determinism test suite over the sequenced input (#812, with #833's key checks): N ∈ {1, 4, 16} threads; `seq` derived from event ids under a pinned `B` while a Geant4 mock hands out blocks with `T = min(√N, N/threads)` (N = 64: T = 8, 8, 4) and `T % B == 0`; shuffled across threads; seeded sleeps; two worker counts; two windows (one small enough to block); two lanes.

## Acceptance
- Identical `content_hash`/`manifest_hash` at 1, 4, 16 threads; equal to an in-order single-thread push.
- Peak batches ahead of `next` ≤ window, peak bytes ≤ credit, blocking observed (non-vacuous).
- Negatives: `seq` = Geant4 task index with a key column → refused (#833); misaligned (`B = 8`, `T = 4`) → duplicate `seq`, refused; without a key column and with non-contiguous batches → different bytes (asserted as the documented producer error); a gap at seal → error, no file; a lost `seq` with pushers blocked → abort and stall each wake them with an error.
- Measured and reported: peak batches/bytes and time blocked under a lagging thread, to set the default `window`.

Size: ≤ ~400 lines (tests only). Depends on: #812, #833.
