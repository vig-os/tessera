---
type: issue
state: open
created: 2026-10-29T14:17:28Z
updated: 2026-10-29T14:17:28Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/703
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-30T08:02:20.476Z
---

# [Issue 703]: [Worker sizing and the perf ratchet's gate read available_parallelism(), which reports the node's 84 CPUs on ksb](https://github.com/vig-os/tessera/issues/703)

## Problem

tessera sizes its own worker pools from `std::thread::available_parallelism()`: `tessera-io` `config.rs` (WriteConfig workers, `balanced` clamps), `table.rs` (the read runtime's `set_workers_to_available_parallelism()` and the grid decode's `cores`), `tessera-ingest` `ge_hdf5.rs`, and `tessera-cli` `bench*.rs`. On a machine whose CPU budget is smaller than what it reports, that over-subscribes.

The ksb ARC runners are such a machine. A pod has no CPU limit (`cpu.max` = `max`, by design: exo-pet/exo-fleet#987), so `available_parallelism()` returns the node's 84 CPUs while the pod requests 8 to 16 and shares the node. exo-fleet pins `RAYON_NUM_THREADS`, `TOKIO_WORKER_THREADS` and similar in the runner env (exo-pet/exo-fleet#1014), but Nix clears a builder's environment, so those pins never reach `nix flake check`.

Found in #700 (#688). There, the test runners now take their thread counts from `$NIX_BUILD_CORES` (`withCpuBudget` in `flake.nix`), which removed the EAGAIN failures. tessera's own sizing is Rust and was out of scope there. Consequences still in place:

- `grid_decode_perf_ratchet_narrow_table` is gated `cores >= 8` so that it skips on CI runners. On a ksb leg it sees 84 and runs, a timing assertion on a shared node. It failed once at 1.14x (needs > 1.2x) while unbudgeted tests ran beside it.
- Each test process still starts an 84-worker vortex read pool on ksb.

## Options (a decision, not decided here)

1. Honour a budget in tessera: for example `RAYON_NUM_THREADS` or a `TESSERA_MAX_THREADS`, with `available_parallelism()` as the fallback. The flake would pass `$NIX_BUILD_CORES`.
2. Gate the ratchet on an explicit opt-in (for example `TESSERA_PERF_RATCHET=1`) instead of on the reported core count.
3. Leave the code alone and give ksb runner pods a CPU limit (reverses exo-fleet#987).

