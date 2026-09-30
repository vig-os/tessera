---
type: issue
state: open
created: 2026-09-29T13:03:08Z
updated: 2026-09-29T16:08:57Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/517
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:06.776Z
---

# [Issue 517]: [ci(nix): split nix flake check into parallel jobs — measured design (it is a memory fix; the cache is the wall-clock fix)](https://github.com/vig-os/tessera/issues/517)

## Summary: the split is a MEMORY fix, not a wall-clock fix — and the numbers say so clearly

Measured from #500's successful x86_64 leg (run 36543170354, 84.8 min). Per-derivation spans, not estimates.

| check derivation | duration | starts @ | ends @ |
|---|---|---|---|
| `tessera-nextest` | **58.5 min** | 25.9 | 84.4 |
| `tessera-seal-profile-determinism` | 31.9 | 45.2 | 77.2 |
| `tessera-cli-cloud` | 22.8 | 22.4 | 45.2 |
| `tessera-py` | 18.1 | 41.1 | 59.2 |
| `tessera-ingest-gate-a-workspace-features` | 16.6 | 51.9 | 68.5 |
| `tessera-ingest-gate-a-reduced` | 13.8 | 38.0 | 51.8 |
| `tessera-ingest-gate-a-default` | 12.9 | 22.4 | 35.4 |
| `tessera-ingest-gate-a-sql` | 12.1 | 68.8 | 80.9 |
| `tessera-clippy` | 9.2 | 22.4 | 31.6 |
| `tessera-cli` / `tessera-test` / cheap gates | ≤3.4 each | | |
| **`tessera-deps`** (shared prefix) | **19.4** | 3.0 | 22.4 |

### The ceiling on any split

```
serial floor = deps (19.4) + longest single check, nextest (58.5) = 77.9 min
current leg                                                       = 84.8 min
=> maximum wall-clock gain from ANY grouping                       =  6.9 min  (8%)
```

Nothing starts until `tessera-deps` completes, and `nextest` alone is 58.5 min. **No arrangement of jobs beats 77.9 min** without making one of those two cheaper.

### The cost

Without a store cache every job re-pays `tessera-deps`:

| jobs | runner-minutes | vs today |
|---|---|---|
| 1 (today) | 84.8 | 1.0× |
| 4 | 290.1 | **3.4×** |
| 6 | 328.8 | 3.9× |
| 8 | 367.6 | 4.3× |

**So the split buys ~8% wall-clock for 3.4× the runner-minutes.** On wall-clock alone that is a bad trade. It is worth doing for **memory** — fewer concurrent derivations per runner is what retires #495's ceiling — and the design should be justified on that basis and sized accordingly.

### Ranked against the alternatives, measured

| lever | wall-clock | runner-minutes | notes |
|---|---|---|---|
| **#507 store cache** | **−19.4 min** | **unchanged** | ~3× the split's gain, free |
| split into 4 | −6.9 min | +240% | but retires the memory ceiling |
| cache + split | −26.3 min (→58.5) | +240% | floor becomes `nextest` |
| partition `nextest` | **≈0** | — | see below |

### Why partitioning `nextest` does not work

`cargo nextest --partition count:N/M` exists and the flake already wires `partitions`/`partitionType`. It is nonetheless useless here, because **`nextest` is ~96% compilation**:

```
194 "Compiling" lines, last at 09:52:57; the derivation ends 09:55:05
test execution: 3 batches totalling ~30 seconds
```

Partitioning shards the 30 seconds of test execution across N jobs while each one re-compiles ~56 minutes. It would *increase* total cost for no wall-clock gain.

### A finding that may matter more than the split

`tessera-deps` compiles **535** crates. `tessera-nextest` — which inherits `cargoArtifacts` — then compiles **194** more, of which **180 are third-party and 93 were already built by `tessera-deps`**.

Roughly half of `nextest`'s third-party compilation is redundant with the artifact it is supposed to be reusing. The cause is not established (a feature-set or target-set mismatch between `buildDepsOnly commonArgs` and what `cargoNextest` resolves is the obvious candidate), so this is a **finding, not a diagnosis**. It is worth investigating before or alongside the split, because it targets the 58.5-minute floor that bounds every design here — including the cache-plus-split ceiling of 58.5 min.

## Proposed design

Three jobs, matching the constraints:

1. **`evaluate`** — `nix eval --json ".#checks.${SYS}" --apply builtins.attrNames` → a JSON matrix. The check list comes from the flake; nothing is hard-coded, so local `nix flake check` and CI stay in parity.
2. **`check`** (matrix) — `nix build -L .#checks.${SYS}.<name>` per group, with the sampler retained.
3. **`nix flake check`** (rollup) — `needs: check`, names every check's verdict, stays the single required status so branch protection is unchanged.

**Grouping** (balanced from the measured durations, post-deps ≈55–58 min each, so none extends the 77.9 floor):

| group | members | ≈min |
|---|---|---|
| A | `workspace-test` (nextest) | 58.5 |
| B | `ingest-gate-a-*` (4) | 55.4 |
| C | `seal-profile-determinism` + `cli-cloud` | 54.7 |
| D | `tessera-py` + `clippy` + `cli` + `test` + fmt/doctest/snapshots/wasm/guardrails | ≈53 |

Four groups, not eight: beyond four, extra jobs add runner-minutes for **zero** wall-clock, because the floor is already `deps + nextest`.

### One correctness requirement for the grouping

The group map must be **exhaustive by construction**. Assign the known-heavy checks explicitly, route everything else to a default group, then **assert that the union of the groups equals the evaluated attribute list and fail if it does not**. Otherwise a newly added check silently belongs to no group, never runs, and the rollup reports green — an unmeasured check is indistinguishable from a passing one. This is the same failure shape as #483's cancelled-checks summary and is cheap to prevent here.

Refs: #495, #507

---

# [Comment #1]() by [gerchowl]()

_Posted on September 29, 2026 at 04:08 PM_

**Correction to the design numbers above.** Every figure attributed to `nextest` here — the **58.5 min serial floor**, **~96% compilation / ~30 s of tests**, and therefore the **6.9 min maximum split gain** — pooled three checks that shared the derivation name `tessera-nextest-0.0.0` (`workspace-test`, `sql-tests`, `minio-range-read`; #535). The 58.5 min was a span from the first to the last log line across all three, not one derivation's duration. **The per-check table above should not be used to size groups** until `workspace-test` is re-measured on its own, which #536 makes possible. The other rows (`tessera-deps` 19.4 min, `seal-profile-determinism`, `ingest-gate-a-*`, clippy, etc.) have unique names and stand; so does #507's 23% cache estimate, which rests only on `tessera-deps`.

