---
type: issue
state: open
created: 2026-11-09T16:12:57Z
updated: 2026-11-09T16:12:57Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/793
comments: 0
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-10T09:13:24.080Z
---

# [Issue 793]: [test(io): stream workers_and_windows + session_leaf_memory hang under ksb CI load (>3,240 s vs 400 s / >840 s vs 0.9 s), timing out the x86 nix check](https://github.com/vig-os/tessera/issues/793)

## Defect
Two tessera-io tests stop making progress inside the `workspace-test` (nextest) flake check on a ksb ARC runner. That pushes the x86_64 `nix flake check` job past its 2 h limit, so CI is red for every PR whose nextest derivation isn't cached.

| run | test | result |
|---|---|---|
| PR #767 branch, run 42622689513 (green) | `tessera-io stream::tests::content_hash_is_identical_across_workers_and_windows` | PASS 400.253 s |
| same | `tessera-io::session_leaf_memory session_leaf_memory` | PASS 0.926 s |
| PR #786, run 42569492335 attempt 2, job 129059193111 | `content_hash_is_identical_across_workers_and_windows` | still running at **> 3,240 s**, job cancelled at the 2 h limit |
| same | `session_leaf_memory` | still running at **> 840 s** |

Attempt 1 of the same run timed out the same way. PR #786 doesn't touch tessera-io (it adds the hdf5-free `tessera-identity` crate). Runner: ksb-vig-os-large, node load ≈ 80–87, `mem avail` ≈ 1.25 TB, so not memory.

A 0.9 s test running for more than 840 s is a hang, not slowness under load. Even the 400 s green timing is suspect for a 20-block in-memory test.

## Hypotheses (to confirm or refute with evidence, not to assume)
1. **Thread-spawn failure turned into a wait.** On ksb pods `available_parallelism()` reports the 84 node CPUs, the pod caps pids at 4,096, and Nix strips the runner's thread pins. nextest runs many test processes at once, each with rayon/std pools plus `StreamWriter` workers (up to 16). If a worker spawn fails (EAGAIN), or a pool sizes itself to 84 threads, the writer may wait forever for a worker that never started. A failed spawn must surface as an `Err`; it must never become a hang. See the ksb-arc-nix gotchas and the EAGAIN friction already seen on flock#2617.
2. A credit-window or commit-order deadlock in `StreamWriter` (d3bf9f9, #677) that only shows up under particular scheduling, e.g. the `2/1B` window with 16 workers.
3. Contention on a shared lock (a counting global allocator in `session_leaf_memory`, or the tempdir on a loaded disk).

## Ask
- Root-cause it, with a reproducer that hangs (or takes ≫ normal) before the fix and is bounded after it, e.g. thread-limit the test process (`ulimit -u`, or a cgroup `pids.max`) and/or `taskset -c 0-1` with high `--test-threads`.
- Fix the cause. Never raise a timeout or skip the test, and add no retry.
- Any thread budget must come from a real limit (cgroup cpu.max and pids.max, or `$NIX_BUILD_CORES`), not from `available_parallelism()` alone.
- If a hang is possible at all, the writer must turn spawn failures and lost workers into errors.

