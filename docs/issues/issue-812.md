---
type: issue
state: open
created: 2026-11-11T01:18:45Z
updated: 2026-11-11T04:19:40Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/812
comments: 0
labels: feature, area:bindings
assignees: none
milestone: none
projects: none
parent: 799
children: none
synced: 2026-11-11T08:50:35.967Z
---

# [Issue 812]: [feature(io): thread-safe seq-ordered table input on StreamWriter, reusing CreditWindow and the ordered commit (ADR-0065 §7.2-§7.3, #802)](https://github.com/vig-os/tessera/issues/812)

Part of epic #799. Design: ADR-0065 (docs/adr/0065-c-abi-bridge.md, PR #819). **Blocked until the owner accepts ADR-0065**; do not start before. Implements the core of #802 (owner steer 2026-11-11).

## What
A thread-safe **sequenced table input** on `StreamWriter`, one per lane (#832), in `tessera-io` (ADR-0065 §7.2-§7.3). `StreamWriter` today accepts jobs only in push order (`submit` assigns `self.seq`, `stream.rs:1092-1101`; `push` takes `&mut self`), so this is the minimal extension, with **no second scheduler**:
- `push(&self, seq, TableData)`, `mark_done(&self, seq)`, `close(expected_count)`; dense `seq` from `first_seq`, each exactly once;
- admission gate = a second `CreditWindow` (`stream.rs:414`) with limits `window` batches and a byte credit; its no-stall rule (`:495`) keyed to the next due `seq`: the due `seq` is never held by the gate; any other blocks while `seq >= next + window` or the credit is held; `refuse_instead_of_block` turns every such wait into an immediate terminal error;
- refusals, each poisoning the input and waking every blocked push (as `CreditWindow::close`, `:567`): `seq >= next + max_ahead`; duplicate (`seq < next` or already entered — reserved under the gate lock on entry); gap or `expected_count` mismatch at `close`; pipeline death;
- a lost `seq`: `abort(reason)` from any thread, and a **finite** stall deadline (no advance of `next` while a push is blocked, default 10 min, configurable up to 24 h, no "disable"), a typed `Error::StreamedTable { reason: Stalled { next } }`;
- release = the committer's ordered pattern (`commit_loop`, `:1144-1172`) applied to batches; released batches go into #806's partition writer; full leaves become ordinary staged jobs on the lane.
- (Sequence-key checks are #833; the full determinism proof is #834.)

## Acceptance
- Shuffled pushes from 4 threads seal the in-order bytes (smoke; the full suite is #834).
- A test per refusal and wake-up path (duplicate incl. two blocked pushes of one `seq`, horizon, gap, count mismatch, abort, stall with a short deadline, pipeline failure), each without a hang (the harness uses a timeout only as a hang detector).

Size: ≤ ~400 lines. Depends on: #806, #832.
