---
type: issue
state: open
created: 2026-11-11T04:18:28Z
updated: 2026-11-11T04:29:46Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/833
comments: 0
labels: feature, area:io
assignees: none
milestone: none
projects: none
parent: 799
children: none
synced: 2026-11-11T08:50:26.285Z
---

# [Issue 833]: [feature(io): sequence-key checks on the sequenced table input: lexicographic order, block range (ADR-0065 §7.2)](https://github.com/vig-os/tessera/issues/833)

Part of epic #799. Design: ADR-0065 (docs/adr/0065-c-abi-bridge.md, PR #819). **Blocked until the owner accepts ADR-0065**; do not start before. Toward #802 (split from #812; ADR-0065 §7.2).

## What
The sequence-key checks on the sequenced table input (#812):
- the table's `sequence_key` (from the schema, #809) is checked across each batch: lexicographically **strictly increasing** when the sequence key is also one of the table's `keys` (e.g. `(event_id, hit_ordinal)`), which proves within-batch order and uniqueness; **non-decreasing** when it isn't (e.g. a multi-row table keyed by `event_id` alone); `order: "producer"` tables (empty key) are not checked;
- with a lane's block size `B ≥ 1` and key base (lane parameters, sealed per role as `capi_seq`), every row's first key component (an integer column) must lie in `[base + seq·B, base + (seq+1)·B)`; `B = 0` = no block cut;
- a violation is `Error::StreamedTable` (`TSRA_E_ORDER` at the ABI) naming the `seq`, the column and the key, and poisons the input;
- `seq` is the push index on a lane with an empty key.

## Acceptance
- A test per refusal (out of range, decreasing, duplicate key across two batches of adjacent seqs), each failing before.
- A Geant4-mock with `T = min(√N, N/threads)` blocks and `seq` taken from the task index (not `event_id / B`) is refused when `T ≠ B`.

Size: ≤ ~250 lines. Depends on: #812, #809.


