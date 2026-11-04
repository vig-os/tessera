---
type: issue
state: open
created: 2026-11-04T00:44:48Z
updated: 2026-11-04T00:44:48Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/761
comments: 0
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-04T08:35:50.413Z
---

# [Issue 761]: [feat(io): self-scaling worker count for push_table — adaptive W under a hard cap (CPU quota, RAM-derived, user max)](https://github.com/vig-os/tessera/issues/761)

**Ask (owner, 2026-11-04):** instead of a fixed W, `push_table` should scale its parallelism automatically toward the optimum, with a max cap.

**Why now:** the #715 proof found the optimal W depends on the environment. On ksb's contended shared HDD (exo-pet/exo-fleet#980), W=4 is *slower* than W=1, with 3× run-to-run spread. On `/dev/shm`, W=4 is 2.78× faster. A fixed W is wrong in one of those places.

**What makes it safe:**
- **Identity:** the output is W-independent. #715 criterion 5 measured equal canonical manifests and `content_hash` at W=1 and W=4. A controller that changes W mid-write can't change the bytes, and a test must pin that across a W schedule.
- **Memory:** the ADR-0064 §8 bound is `min(W, n)·p_block + s` (plus `c_leaf·n` after #760), so a hard cap on W gives a hard memory ceiling:
  `W_max = min(user cap, cgroup CPU quota, ⌊(RAM budget − R₀ − s − c_leaf·n) / p_block⌋)`.
  The CPU quota must come from the cgroup, not `available_parallelism()` (#703: that reports 84 node CPUs in a ksb pod).

**Controller sketch (to be decided in the design):**
- Start at W=1.
- Increase W while rows/s improves by more than a threshold and the reader isn't stalled on staged I/O.
- Back off when workers wait on the staged reads or the disk. The `StreamStats` counters (`producer_stalls`, `peak_staged_input_*`) already expose these signals.
- Use hysteresis, never exceed `W_max`, and make the decision trace observable in `StreamStats`.

**Prior art in-tree:** `WriteConfig::balanced` plus `tessera bench write --auto` is a one-shot warm-up heuristic, bench-only and listmode-only. #368 added unified worker and RAM caps with flag > env > conf > default precedence.

**Process:** design first (an ADR or an ADR-0064 amendment: the controller, the cap derivation, the knob's public API), then implementation. Tests:
- byte identity under a forced W schedule;
- the memory ceiling at `W_max`;
- convergence on a disk-bound vs a CPU-bound fixture (timing graded only on tmpfs; #760).

Not part of the current #677 goal; queued after it.

Refs: #677
