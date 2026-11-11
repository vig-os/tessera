---
type: issue
state: open
created: 2026-11-11T01:02:43Z
updated: 2026-11-11T02:37:01Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/802
comments: 3
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:41.388Z
---

# [Issue 802]: [feature(io): deterministic sealed bytes regardless of producer-thread scheduling](https://github.com/vig-os/tessera/issues/802)

Sub-issue 3 of #799. Many C++ threads feed one product; content_hash must not depend on arrival order (explicit sequence key or ordered lanes). Extends the #677 worker-count invariance to producer threads; a test runs N threads under randomized scheduling and asserts byte identity.
---

# [Comment #1]() by [gerchowl]()

_Posted on November 11, 2026 at 01:10 AM_

Design steer 2026-11-11 (full text on #800): determinism comes from an explicit, dense, producer-assigned `seq` per batch, not from thread order. Geant4 MT hands events to threads dynamically, but its seeds are per-event, so `seq` = event-block index. The bridge commits in `seq` order behind StreamWriter's bounded credit window (back-pressure). Same seq→batch mapping → same bytes for every thread count.

---

# [Comment #2]() by [gerchowl]()

_Posted on November 11, 2026 at 01:26 AM_

Ordering analysis, updated for the owner's `seq` steer: §4.4 of [801-consumer-gap-analysis.md](https://github.com/vig-os/tessera/blob/docs/801-consumer-gap-analysis/docs/spikes/801-consumer-gap-analysis.md#44-what-802-has-to-provide) (PR #820).
1. Event content is thread-count-independent: with `seedOncePerCommunication = 0`, event *i* gets seed pair *i* (`G4TaskRunManager.cc:629-661`). The interleaving across threads is not.
2. The key is the steer's dense `seq`, defined as **`seq = event_id / B`** for a block size `B` the producer fixes and records in the session metadata. Round 1's `[lo, hi)` segments map onto it exactly.
3. Contradiction with "seq = the event-block index": Geant4's block size `T = min(sqrt(N), N / pool size)` (`G4TaskRunManager.cc:293-333`) follows the thread count once it exceeds `sqrt(N)`. `SetEventModulo` is capped the same way, and g4ring's Stage 2 value is too (`PhaseSpaceTasks.h:32-46`). The producer must pin `T % B == 0` and check it. A misaligned run is a duplicate-`seq` error.
4. Proposed amendments: (a) to the partition item: if leaves are cut at `BLOCK_ROWS` over the `seq`-ordered rows, `B` drops out of the bytes; (b) to the test item: the same N events at 1, 4 and 16 threads with Geant4's **default** modulo must give an identical hash with `B` pinned, and must differ or be rejected without it.


---

# [Comment #3]() by [gerchowl]()

_Posted on November 11, 2026 at 02:34 AM_

Amendment to the seq design (from the #820 review): Geant4's task run manager sets eventModulo = min(sqrt(N), N/pool size) (G4TaskRunManager.cc:292-323). Its block cut therefore depends on the thread count once threads > sqrt(N). seq is deterministic only with a producer-fixed block size B, pinned via SetEventModulo as Stage 2 does (g4ring.cpp:616-628), or seq = event_id / B. B becomes a declared, recorded session parameter. The determinism test runs 1/4/16 threads with dynamic block take, plus a negative case for a thread-dependent cut.

