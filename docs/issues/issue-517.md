---
type: issue
state: open
created: 2026-09-29T13:03:08Z
updated: 2026-10-05T01:04:05Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/517
comments: 4
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-05T08:17:44.218Z
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

---

# [Comment #2]() by [gerchowl]()

_Posted on October 4, 2026 at 09:32 PM_

### Re-measured after #536 — the conclusion above is reversed

The design numbers in this issue pooled three checks that shared the derivation name `tessera-nextest-0.0.0` (#535). With #536's distinct names, per-derivation spans are attributable. Below: medians over 3 post-#536 runs per arch (runs 36597559869, 36608335562, 36608750748), `--max-jobs 2 --cores 4`.

| derivation | x86_64 median (max) | aarch64 median (max) |
|---|---:|---:|
| `tessera-sql-nextest` (`sql-tests`) | **19.5** (27.2) | **19.0** (21.0) |
| `tessera-seal-profile-determinism` | 14.5 (15.8) | 11.0 (12.0) |
| **`tessera-deps`** (shared prefix) | **14.0** (14.2) | **10.5** (10.6) |
| `tessera-cloud-nextest` (`minio-range-read`) | 10.8 (11.8) | 8.6 (8.7) |
| `tessera-cli-cloud` | 9.8 (11.0) | 7.5 (8.0) |
| `ingest-gate-a-npy-only` | 8.1 | 7.0 |
| `tessera-py` | 7.8 | 4.7 |
| `ingest-gate-a-sql` / `-workspace-features` / `-reduced` / `-default` | 7.0 / 6.8 / 6.5 / 6.4 | 5.7 / 6.5 / 5.1 / 5.1 |
| **`tessera-nextest` (`workspace-test`)** | **4.6** | **3.5** |
| `tessera-clippy` | 4.5 | 3.6 |
| leg wall-clock | 77–79 | 59–61 |

**`workspace-test` is 4.6 min, not 58.5.** The 58.5 was a first-line-to-last-line span across three derivations.

#### What that changes

The earlier conclusion — "serial floor = deps + nextest = 77.9 min, so any split gains ≤ 6.9 min at 3.4× the runner-minutes" — is **wrong**. Corrected:

- **Serial floor** = deps + longest single check = 14.0 + 19.5 (median) … 27.2 (max) ≈ **34–41 min**, against a **78-min** leg.
- The legs are **throughput-bound, not critical-path-bound**: ~106 min of check work (x86) through two `max-jobs` slots, plus the 14-min deps prefix. That is why the leg is ~2× its floor.

So a split **can roughly halve wall-clock** — the opposite of what was concluded above. And it is no longer only a memory fix: with #507's dependency cache, every job restores the 14-min prefix in ~1–2 min instead of rebuilding it, which also collapses the runner-minute penalty the original analysis charged per job.

Caveat on the method: each span is measured while sharing a 4-core runner with one other derivation, so a derivation alone on its own runner would likely be somewhat faster — these are upper bounds for a split, which only strengthens the conclusion.

A design built on these numbers follows once #507 lands, since the split's cost depends on it.


---

# [Comment #3]() by [gerchowl]()

_Posted on October 5, 2026 at 12:03 AM_

### Design, from measured numbers (builds on #544's dependency cache)

**Inputs.** Per-check cost = the check's own derivation plus any *private* intermediates. Medians come from post-#536 runs, with Gate A `-reduced` taken from the first post-#516 run. From the derivation graph, only **one** intermediate is shared between checks: `tessera-py`, needed by `tessera-py-import` and `tessera-wheel-import`. Splitting any other pair costs nothing extra. 14 of 25 checks need `tessera-deps`, which every job restores through #544's action (~40 s on a hit).

#### Groups — 6 jobs per arch, balanced against the longest single check

| job | checks | x86 min (sum of medians) |
|---|---|---:|
| `sql` | sql-tests | 19.5 |
| `seal` | seal-profile-determinism · workspace-clippy | 19.0 |
| `cloud` | minio-range-read · **ingest-gate-a-sql** | 17.8 |
| `registry` | registry-roundtrip (incl. tessera-cli-cloud) · **ingest-gate-a-reduced** | ~18.5 |
| `python` | **ingest-gate-a-workspace** · tessera-py-import · tessera-wheel-import (share tessera-py) · workspace-test | 19.2 |
| `rest` | **ingest-gate-a** · every other check (doctest, producer-equality, wasm, fmt, guardrails, snapshots, mdbook, reference-reader, test-coverage, bench-ecosystems, oci-roundtrip, dev-shell) | ~11 |

**Never two Gate A checks in one job.** The post-#514 peaks (13.7–14.1 GB, 1.9–2.3 GB free) were each two dev-profile Gate A builds overlapping. One Gate A check per job makes that overlap structurally impossible, rather than a scheduling accident. This is asserted, not just intended.

#### Expected

| | today (dev) | with #544 cache | **#544 + split** |
|---|---:|---:|---:|
| wall-clock per arch, warm | 77–79 / 59–61 | 41 / 45 | **~22–26** (setup ~2 + restore ~1 + longest group ~20) |
| wall-clock, deps changed (cold key) | same | +deps ~9–14 | ~36–40 |
| runner-minutes per arch, warm | ~78 / 60 | ~41 / 45 | ~105–120 (≈2.7× today's cached leg) |
| worst memory overlap | 2 Gate A (14.1 GB) | same | ≤1 Gate A per job |

Runner-minutes rise; wall-clock roughly halves again. GitHub-hosted minutes on a public repo are not billed, but **concurrency is finite**: 2 arches × 6 jobs = 12 concurrent jobs per PR, and several open PRs can queue against the org's limit. If that bites, the groups merge pairwise (3 jobs/arch, ~35 min) without touching the rest of the design.

On a cold key every job builds `tessera-deps` itself — duplicate runner time only, no wall-clock cost. That happens only on PRs that change the dependency derivation, because #544's warmer keeps the `dev`-scope cache current.

#### Structure — local parity kept

1. **`plan`** — evaluates the groups from the flake (`legacyPackages.<system>.ciGroups`) into the matrix, and runs `nix flake check --no-build`, so every output is still *evaluated* exactly as before. Nothing is hard-coded in YAML.
2. **`check (<arch>, <group>)`** — restores deps, then `nix build -L --keep-going --fallback .#checks.<sys>.<each check in group>`, with the sampler kept. It records a per-check verdict from the store, as the current summary does.
3. **`nix flake check`** (rollup, the single required status) — collects every job's verdicts, prints **every check's** result per arch, and fails unless **the set of verdicts equals the set of checks** (exhaustive at run time, as well as at eval time).

Local `nix flake check` is unchanged: same checks, same flake.

#### Grouping lives in the flake, and is exhaustive by construction

`ciGroups` names the groups. `rest` is computed as *every check not listed elsewhere*, so adding a check can never drop it from CI. Evaluation **throws** if a listed name is not a check, if a check appears in two groups, or if two `ingest-gate-a*` checks share a group. Every one of those rules gets a negative test.


---

# [Comment #4]() by [gerchowl]()

_Posted on October 5, 2026 at 01:04 AM_

Implemented in #548, stacked on #544. Measured on GitHub-hosted runners via `workflow_dispatch`:

| | dev today | #544 alone (warm) | split, cold key | **split, warm** |
|---|---:|---:|---:|---:|
| wall-clock (min) | 77–79 / 59–61 | 41 / 45 | 34 | **18** |
| peak memory | 13.7–14.1 GB | 11.4–11.7 GB | ≤ 9.7 GB | ≤ 10.4 GB |
| runner-min warm (x86 / arm) | 78 / 60 | 41 / 45 | — | 70.5 / 56.3 |

Both runs passed the gate with 50/50 verdicts and 2/2 flake evaluations (runs 37246299669 and 37248665133). On the warm run, every job restored `tessera-deps` and built it 0 times. The critical path is now `sql-tests` (17.4 min on x86). One Gate A build per job is asserted at eval time, which removes the #495 overlap.

