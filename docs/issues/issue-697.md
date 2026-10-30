---
type: issue
state: open
created: 2026-10-29T06:18:31Z
updated: 2026-10-29T08:29:50Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/697
comments: 2
labels: bug, area:io, adr
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-30T08:02:21.909Z
---

# [Issue 697]: [design(query): object-store spool default P undercounts prefetch and zip inputs (ADR-0062 §7)](https://github.com/vig-os/tessera/issues/697)

Found while reviewing the ADR-0062 acceptance (#654). It is not part of owner decisions (a)/(b)/(c), so this
issue tracks it and the ADR acceptance does not change it.

**Problem.** ADR-0062 §7 sets the object-store spool default to `spool_blocks = P` outside key mode. Two
things need more slots than that:

- **Prefetch.** §3 lets a partition keep two blocks open, the current one and the next while prefetching
  across a block boundary. Under `VerifyBlocks` on an object store a block can only be opened from the
  spool, so a partition can hold two slots. If partitions blocked on a full channel hold two slots each, a
  partition the consumer is draining can wait forever for a slot. That is the hang ADR-0062 §5 rules out for
  key mode. The key-mode zip avoids it by not prefetching across a block boundary on an object store (§5,
  `3 · k · P`).
- **Declared-mode zips.** A declared-mode zip of `k` inputs scans `k` blocks per partition, so it needs at
  least `k · P` slots, and `2 · k · P` with prefetch. The default `P` does not count `k`.

**Ask.** Before #668 lands the object-store spool, pin the non-key slot rule in ADR-0062 §7. Either count
prefetch and `k` in the default and make `plan_scan` refuse a `spool_blocks` below it, or forbid prefetch
on spooled sources as key mode does. Then add a #668 acceptance test: an object-store scan and a
declared-mode zip at the default `spool_blocks`, whose consumer drains one partition before the others,
completes.

Gates: #668.

---

# [Comment #1]() by [gerchowl]()

_Posted on October 29, 2026 at 06:20 AM_

The same applies to a local source with the spool opt-in (ADR-0062 §7, recorded in the acceptance PR for #654): a spooled local scan outside key mode has the same default and the same prefetch rule, so whatever rule this issue settles must cover both spooled paths.

---

# [Comment #2]() by [gerchowl]()

_Posted on October 29, 2026 at 08:29 AM_

Resolved in #698 (commit 4bb21bd), ADR-0062 §7 "Spool slots". This is now normative and not provisional. Every spooled scan (an object store under `VerifyBlocks`, or the local opt-in) counts, per input and per partition, the block it is scanning plus the prefetched next one, and in key mode also the pinned first and last block. A plan therefore needs at most `2 · k · P` slots, or `4 · k · P` in key mode. `plan_scan` computes the plan's actual need, makes it the default, and refuses a smaller `spool_blocks` with a typed error before any row. A block two partitions share counts once only where both pin it (key mode). Elsewhere it counts in each partition's need. Because every partition can hold its maximum at the same time, no consumer order can hang the scan. Prefetch is counted rather than disabled because the measured 2.6–8× re-read happens inside one block, while prefetch is what hides the next block's download and hash. Its cost is spool disk, not RAM. The old default `P` hangs a declared-mode zip with k = 2 and P = 1, and the ADR now cites that case as the reason. The tests are in #668's acceptance criteria (object store and local opt-in, including the k = 2/P = 1 case and the k = 1/P = 2 shared-block case). The PR closes this issue.

