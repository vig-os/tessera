---
type: issue
state: open
created: 2026-11-11T01:00:40Z
updated: 2026-11-11T01:00:40Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/798
comments: 0
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:43.531Z
---

# [Issue 798]: [perf(io): group commit — one journal fsync per batch instead of ~77 per block (ADR-0026/#695 amendment)](https://github.com/vig-os/tessera/issues/798)

## Context
From #793 (strace on dev 6a30780): each `WriteSession` commit fsyncs the fragment, its directory and the journal (#695).
- `content_hash_is_identical_across_workers_and_windows` costs 1,536 fsyncs (20 blocks × 12 configurations).
- `session_leaf_memory` costs 999.
- On a write-loaded ext4 volume (one concurrent `dd` writer), fsync p99 is 26 s, and the committer waits in `jbd2_log_wait_commit`. That is a real product cost on DAQ hosts with busy disks, not only in CI.

Owner 2026-11-11: tests move to a RAM fs (#793). Group commit is filed separately as a product improvement.

## Ask (design first)
An ADR-0026/#695 amendment for **group commit**: one journal fsync per batch of committed fragments, with the crash semantics stated.
- What does the registry watermark promise after a crash mid-batch?
- How do direct `append_block` callers, which the batch doesn't cover, keep per-call durability?

Then implement, with a test that counts fsyncs before and after (e.g. an `strace -c`-backed harness or an injected sync counter) and the crash-recovery tests unchanged.

