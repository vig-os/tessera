---
type: issue
state: open
created: 2026-10-29T14:17:28Z
updated: 2026-11-22T17:36:17Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/703
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-23T10:16:34.227Z
---

# [Issue 703]: [Worker sizing and the perf ratchet's gate read available_parallelism(), which reports the node's 84 CPUs on [batch-host]](https://github.com/vig-os/tessera/issues/703)

## Problem

tessera sizes its own worker pools from `std::thread::available_parallelism()`: `tessera-io` `config.rs` (WriteConfig workers, `balanced` clamps), `table.rs` (the read runtime's `set_workers_to_available_parallelism()` and the grid decode's `cores`), `tessera-ingest` `ge_hdf5.rs`, and `tessera-cli` `bench*.rs`. On a machine whose CPU budget is smaller than what it reports, that over-subscribes.

The [batch-host] ARC runners are such a machine. A pod has no CPU limit (`cpu.max` = `max`, by design: [private-repo]#987), so `available_parallelism()` returns the node's 84 CPUs while the pod requests 8 to 16 and shares the node. exo-fleet pins `RAYON_NUM_THREADS`, `TOKIO_WORKER_THREADS` and similar in the runner env ([private-repo]#1014), but Nix clears a builder's environment, so those pins never reach `nix flake check`.

Found in #700 (#688). There, the test runners now take their thread counts from `$NIX_BUILD_CORES` (`withCpuBudget` in `flake.nix`), which removed the EAGAIN failures. tessera's own sizing is Rust and was out of scope there. Consequences still in place:

- `grid_decode_perf_ratchet_narrow_table` is gated `cores >= 8` so that it skips on CI runners. On a [batch-host] leg it sees 84 and runs, a timing assertion on a shared node. It failed once at 1.14x (needs > 1.2x) while unbudgeted tests ran beside it.
- Each test process still starts an 84-worker vortex read pool on [batch-host].

## Options (a decision, not decided here)

1. Honour a budget in tessera: for example `RAYON_NUM_THREADS` or a `TESSERA_MAX_THREADS`, with `available_parallelism()` as the fallback. The flake would pass `$NIX_BUILD_CORES`.
2. Gate the ratchet on an explicit opt-in (for example `TESSERA_PERF_RATCHET=1`) instead of on the reported core count.
3. Leave the code alone and give [batch-host] runner pods a CPU limit (reverses exo-fleet#987).

---

# [Comment #1]() by [gerchowl]()

_Posted on November 22, 2026 at 05:36 PM_

## Second CI failure, on code whose change doesn't touch the decode path (PR #951)

`grid_decode_perf_ratchet_narrow_table` failed again on a ksb leg, this time at **1.12×** (it needs > 1.2×). The run was
[45027807977](https://github.com/vig-os/tessera/actions/runs/45027807977/job/137224570801), `nix flake check (x86_64-linux)`
on `ksb-vig-os-large-runner-vkjkm`, with `--max-jobs 2 --cores 4`. The log line `test CPU budget: 4 (available to the build: 80)`
shows what happened: the gate saw 80 cores and ran the timing assertion beside the other test threads.

PR #951's only `tessera-io` change adds `ColumnData::values_and_validity`, an accessor that no decode or materialise path calls,
plus a unit test. To show it doesn't move the ratchet, I built a release lib-test binary from the same tree twice, once with
dev's `table.rs` and once with the PR's, then ran the ratchet alone, interleaved 5× each, under `perf stat` on ksb-ws:

| condition | dev: pass / task-clock / instructions:u | PR: pass / task-clock / instructions:u |
|---|---|---|
| `taskset 0-15`, nothing beside it | 5/5 · 1072–1111 ms · 5.234e9 | 5/5 · 1077–1136 ms · 5.234e9 |
| `taskset 0-7` + a neighbour decoding on the same 8 cores | 5/5 · 996–1012 ms · 5.230–5.231e9 | 5/5 · 986–1008 ms · 5.230–5.231e9 |

The instruction counts agree to 0.02%, so the decode does the same work with or without the change. That count doesn't
depend on load. The ratchet passes on the seat, whose cgroup and affinity match the core count the gate reads. It fails
only where the gate reads 80 cores, but the build gets 4.

Recent history: none of the 12 failed `nix-check` runs on other branches between 2026-11-15 and 2026-11-22 failed here.
So this is rare and depends on the environment, as the 1.14× case this issue records already showed. Options 1 and 2 above
would fix it. PR #951 leaves the test as it is.


