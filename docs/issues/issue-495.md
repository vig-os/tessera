---
type: issue
state: open
created: 2026-09-29T05:43:29Z
updated: 2026-09-29T11:43:45Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/495
comments: 8
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:12.198Z
---

# [Issue 495]: [ci(nix-check): the x86_64 leg's runtime varies 2.1x and its bad draws hit the cap](https://github.com/vig-os/tessera/issues/495)

## Observation

The `nix flake check` legs are not slow so much as **inconsistent**, and the inconsistency is
one-sided. Measured leg durations, same workflow, all GitHub-hosted:

| commit | x86_64 | aarch64 |
|---|---|---|
| 7d2eaf8d | 43m success | 41m success |
| 33aa5134 | 46m success | 43m success |
| (vortex pin PR) | 57m success | 43m success |
| 1442ba12 (#461) | **89m TIMEOUT** | 54m success |

- x86_64's own range: **43–89m, a 2.1× spread**
- aarch64's own range: **41–54m, a 1.3× spread**

## The axis is variance, not architecture

Worth stating because the single-sample reading is tempting and wrong. From the 89m/54m run alone it
looks like "x86_64 is ~1.7× slower than aarch64", and that reading was proposed. But on the two runs
above it, x86 is within **5–7%** of aarch64. There is no architectural constant to tune for; there is a
wide distribution on one arch whose upper tail crosses the cap.

The distinction changes the fix. "x86 is slower" suggests rebalancing work between arches. "x86's own
spread is 2.1×" suggests headroom plus finding the cause of the tail — which is what this issue is for.

`nix-check.yml`'s header already documented this variance before any of the above ("observed 40m (x86)
and 51m (aarch64) on one run, then >60m on x86 for the very next one"), so the behaviour is
long-standing; what is new is that the tail now exceeds the cap.

## Immediate mitigation (separate PR)

`timeout-minutes` 90 → 120: the worst observed draw plus ~35%. That stops the cap cutting real work —
#461's leg was killed at 89 minutes having done nothing wrong — but it **raises the ceiling rather than
making anything faster**, so it is a mitigation and not this issue's fix.

## What would actually fix it

The sampler added in #488 now records free memory, disk and load average every 30s on every leg, printed
into the streamed step log so it survives an eviction. A long x86 draw can therefore now be attributed
rather than guessed:

- **memory pressure** — peak on a *passing* leg is already 10.2 GB (x86) / 11.6 GB (aarch64) of 16 GB,
  so a bad draw plausibly swaps. Baseline from #488's green legs.
- **I/O starvation** — visible as load average high while memory is fine.
- **simply slower silicon** — resource traces flat and unremarkable, duration up anyway. This is the
  outcome that would justify accepting the variance and only keeping headroom.

Until one long draw is captured with traces, any structural change is a guess. Two specific things
should wait for that evidence:

1. **`--max-jobs` / `--cores`.** Capping nix's concurrency is the candidate fix for the *separate*
   eviction class (#483), but it trades wall-clock for peak footprint. At 89 of 90 minutes that trade
   converted intermittent failures into reliable ones, which is why the timeout bump is a hard
   prerequisite for it rather than an optional companion.
2. **Splitting or trimming heavy checks.** Note nix builds derivations in parallel, so splitting a
   check into two makes two schedulable units where there was one and can *raise* peak footprint. An
   earlier ENOSPC bug in this repo was correctly fixed by splitting, which makes this the natural and
   wrong inference to draw — match the axis to the precedent.

## Contributing factor worth naming

The vortex fork pin (#480) makes ~27 vortex crates compile from a git source with **no binary-cache
substituter**, where they were previously downloaded. That raised compile load on every leg. It was the
right call for #468/#472 and it is bounded — the pin is temporary — but it is part of why the tail moved
now rather than earlier.

Refs #483, #488, #461
---

# [Comment #1]() by [gerchowl]()

_Posted on September 29, 2026 at 06:57 AM_

## Answered: it is MEMORY. The sampler caught an eviction in the act.

The sampler added in #488 captured a full eviction on #500's x86_64 leg
([run log](https://github.com/vig-os/tessera/pull/500)). Memory available fell **monotonically** to
nothing and then the runner died:

```
[res] 06:42:17 | mem used=10822M avail=5167M | disk avail=60177M | load 16.11
[res] 06:42:47 | mem used=11103M avail=4886M | disk avail=60000M | load 15.01
[res] 06:43:17 | mem used=12618M avail=3371M | disk avail=59838M | load 14.27
[res] 06:43:47 | mem used=14335M avail=1654M | disk avail=59639M | load 13.85
[res] 06:44:17 | mem used=14359M avail=1630M | disk avail=59458M | load 13.63
[res] 06:44:47 | mem used=15064M avail= 924M | disk avail=59286M | load 13.59
[res] 06:45:17 | mem used=15125M avail= 864M | disk avail=59097M | load 13.41
[res] 06:45:47 | mem used=15577M avail= 412M | disk avail=58946M | load 13.30
06:46:08  ##[error]The runner has received a shutdown signal …
06:46:08  ##[error]Process completed with exit code 143.
```

**412 MB of 16 GB left, 21 seconds before the kill.** That is the OOM killer taking the runner agent —
which is why the job could never print an error of its own, and why every one of these evictions looked
like a generic shutdown.

**Disk is definitively excluded, with a number:** it never dropped below **57.6 GB free** across the
entire run, moving only ~1 GB while memory collapsed by 4.7 GB. The disk hypothesis is closed.

The aarch64 leg of the same run died the same way, peaking at 15,633 MB.

## What this changes

`--max-jobs` / `--cores` is no longer a guess. The remaining question is only *which* lever, and the
trace answers that too.

**Load average 13–16 on 4 cores.** nix's `max-jobs` defaults to the core count, so 4 derivations run
concurrently, each spawning roughly 4 compiler threads — 4 × 4 = 16, which matches the observed load
exactly. Peak memory is driven by the number of concurrent `rustc`/LLVM processes, i.e. the product.

That makes **`--cores 2` the better first lever, not `--max-jobs 2`:**

| lever | concurrent rustc | wall-clock cost |
|---|---|---|
| today (`max-jobs 4`, `cores 4`) | ~16 | baseline, but evicts |
| `--cores 2` | ~8 | derivation-level parallelism **preserved** |
| `--max-jobs 2` | ~8 | serialises whole derivations — closer to 2× on the parallel portion |

Both roughly halve the concurrent compiler count, but `--cores` keeps four derivations in flight, so it
pays much less wall-clock. That matters because #498 raised the cap to 120 min against x86 draws already
reaching 89 — a lever that doubles the parallel portion could reach the new cap too.

**The change is self-verifying.** The sampler reports peak memory on every leg, so `--cores 2` should
visibly drop peak from ~15.6 GB. If it does not, the model is wrong and `--max-jobs` is next.

## Ordering

1. **#498** (timeout 90 → 120) — merged/merging first, as agreed. Prerequisite: any wall-clock trade
   against an 89-of-90-minute draw converts intermittent failures into reliable ones.
2. **`--cores 2`** — with the sampler confirming the peak drops.
3. `--max-jobs` only if `--cores` proves insufficient.

## Note on the contributing factor

Peak has climbed across observed runs: 10.2 GB → 12.0 GB → **15.6 GB (evicted)**. The vortex fork pin
(#480) is part of that — ~27 crates now compile from a git source with no binary-cache substituter where
they were previously downloaded. The pin is temporary and was the right call for #468/#472, but it moved
these legs from "tight" to "over the edge", so removing it later should give headroom back.

Refs #483, #488, #500

---

# [Comment #2]() by [gerchowl]()

_Posted on September 29, 2026 at 08:45 AM_

### A paired control for the OOM diagnosis, from #500's own branch

Run [36529815712](https://github.com/vig-os/tessera/actions/runs/36529815712) is the last `ci/499-branch-gate-parity` leg **before** `--cores 2` (#498) was merged in, and run 36543170354 is the same branch **after**. Same source tree, same checks, only the CI knob differs — so this is a cleaner A/B than #498's own pair, where the branch content also changed.

**Control (0e6e9a1e, no `--cores 2`) — both legs evicted:**

| leg | duration | peak mem | avail at death | load at death | outcome |
|---|---|---|---|---|---|
| x86_64 | 34m33s | **15577 MB** | 412 MB | 13.30 | exit 143 |
| aarch64 | 43m02s | **15633 MB** | 313 MB | 28.95 | exit 143 |

Both died the same way, within 16 GB:

```
[res] 06:45:47 | mem used=15577M avail=412M | load 13.30 14.03 11.24
##[error]The runner has received a shutdown signal...
##[error]Process completed with exit code 143.
```

Two details worth keeping:

1. **The eviction is memory, not disk.** `/ avail` never dropped below **58.9 GB** on x86 and **84.8 GB** on aarch64 across all 70 samples. The earlier disk hypothesis is falsified for good on this run.

2. **aarch64 showed the thrash tail explicitly.** It pegged at 15.60 GB with ~340 MB available for **3m14s** (06:51:20 → 06:54:34) while load climbed 15.54 → 28.95 — that is the box swapping itself to death, not a check failing. x86 died 21 s after crossing 15.58 GB, so it never displayed the tail; the two legs are the same failure caught at different phases.

That answers the "cause stayed inferred" caveat in the workflow comment: with the sampler in place, the eviction is now *observed*, and the memory ceiling is the mechanism.

Treatment numbers for this same branch go in the next comment when the run settles.


---

# [Comment #3]() by [gerchowl]()

_Posted on September 29, 2026 at 09:57 AM_

### Treatment arm: `--cores 2` on the same branch — both legs green, ~4 GB of headroom recovered

Run [36543170354](https://github.com/vig-os/tessera/actions/runs/36543170354) is `ci/499-branch-gate-parity` **after** `--cores 2` merged in; the control above is the same branch **before**. Same source tree, same 18-check board, only the knob moved.

| leg | control | | treatment | | Δ peak |
|---|---|---|---|---|---|
| | duration | peak | duration | peak | |
| x86_64 | 34m33s → **evicted** | 15577 MB | **84m46s → SUCCESS** | 11957 MB | −3620 MB (−23%) |
| aarch64 | 43m02s → **evicted** | 15633 MB | **65m10s → SUCCESS** | 11669 MB | −3964 MB (−25%) |

Headroom is the number that matters, and it moved by an order of magnitude:

| | control min-avail | treatment min-avail | control max-load | treatment max-load |
|---|---|---|---|---|
| x86_64 | 412 MB | **4032 MB** | 13.30 | **9.14** |
| aarch64 | 313 MB | **4277 MB** | 28.95 | **10.15** |

Disk was never in play in either arm: minimum free was **60.4 GB** (x86) and **81.9 GB** (aarch64) across 301 samples.

The load figures confirm the mechanism rather than just the outcome. The control's aarch64 leg reached **28.95** on a 4-core box — that is not work, it is runnable threads piling up behind memory pressure. Under `--cores 2` the same leg peaks at **10.15** and finishes.

**Verdict on the fix:** the eviction is resolved, not merely deferred. Both legs now complete with ~4 GB spare against the same 16 GB ceiling that killed both before.

**What stays open (this issue):** wall-clock. x86 at **84m46s** leaves 35 minutes against the 120-minute cap — 30% headroom, on the arch whose runtime is the variable one (43–89 min observed historically). aarch64 at 65m is comfortable. Nothing to change yet; one more datapoint from a heavy branch is the right next input, and #461 supplies it.


---

# [Comment #4]() by [gerchowl]()

_Posted on September 29, 2026 at 10:01 AM_

### Correction: the control's 34m/43m are times-to-death, not durations — and the `null`-step heuristic has a counterexample

Two corrections to the tables above, the first to my own presentation.

**1. No slowdown ratio can be computed against the control arm.** The control's 34m33s and 43m02s are *time-to-eviction*, not completion times — those runs never finished. A run dying of memory pressure is also pathologically slow near the end (the aarch64 trace spends its last 3m14s thrashing at ~340 MB available), so the figure is both incomplete and inflated. My tables label the cells `→ evicted`, but they sit under a shared "duration" header next to the treatment's real durations, which invites exactly the comparison that cannot be made. Read the control column as **"died at"**, not "took".

The A/B above remains valid for what it was built to show — eviction vs. no eviction, and the peak/headroom/load deltas, all of which are measured on both arms. It is only the wall-clock **ratio** that has no denominator.

A same-arch, both-completed pair does exist, on the #461 branch: aarch64 went **53m (cores=4) → 69m (cores=2), +30%**. That is the first real cost figure for the fix.

**2. The `null` step-conclusion heuristic does not reliably identify evictions.** The proposal was that an evicted leg records `null` step conclusions (post-steps never run), distinguishing it from a genuine check failure without needing logs. This run is a counterexample:

```
CONTROL 36529815712 (x86_64) — EVICTED, exit 143:
   nix flake check = failure        Post Install Nix = skipped        job = failure

TREATMENT 36543170354 (x86_64) — completed:
   nix flake check = success        Post Install Nix = success        job = success
```

The evicted leg recorded `failure`, not `null`, with post-steps `skipped`. So there are at least **two eviction shapes**: one where the step process takes SIGTERM and records `failure`/`skipped` (this run), and one where the runner agent dies abruptly enough that step conclusions stay `null` (#461's x86 leg). Only the second matches the heuristic.

Using `null` as the discriminator therefore produces **false negatives** — an eviction that looks exactly like a genuine `nix flake check` failure, which is the more dangerous direction of error for triage. The reliable signal remains the log signature:

```
##[error]The runner has received a shutdown signal...
##[error]Process completed with exit code 143.
```

`null` steps are *sufficient* evidence of an eviction, not *necessary*. Worth stating that way in any triage note, since the tempting reading is a biconditional.


---

# [Comment #5]() by [gerchowl]()

_Posted on September 29, 2026 at 10:05 AM_

### The reliable eviction discriminator: check-run annotations — but match BOTH messages

Following the `null`-step counterexample above, the authoritative signal is the **check-run annotation**. GitHub attributes the cause itself, and — the part that matters for retrospective triage — it **outlives the logs**, which 404 once retention expires.

```sh
gh api repos/vig-os/tessera/check-runs/$JOB_ID/annotations --jq '.[]|"\(.annotation_level): \(.message)"'
```

Confirmed against both arms of the #500 A/B:

| run | leg | annotation |
|---|---|---|
| 36529815712 (control, evicted) | x86_64 | `failure: Process completed with exit code 143.` |
| 36529815712 (control, evicted) | aarch64 | `failure: Process completed with exit code 143.` |
| 36543170354 (treatment, green) | both | *(no failure annotation)* |

**The caveat that keeps this from being a one-liner: the two eviction shapes carry different messages.** #461's x86 leg annotates as

```
failure: The hosted runner lost communication with the server. Anything in your workflow that
         terminates the runner process, starves it for CPU/Memory, or blocks its network access
         can cause this error.
```

while both legs here annotate as `Process completed with exit code 143.` Same cause, two attributions — depending on whether the step process was signalled (and recorded its own exit) or the runner agent died first. Matching only the "lost communication" string would classify this run's evictions as ordinary check failures, which is the false-negative direction again.

So the triage rule is:

```sh
gh api repos/vig-os/tessera/check-runs/$JOB_ID/annotations \
  --jq '.[]|select(.annotation_level=="failure")
          |select(.message|test("exit code 143|lost communication with the server"))'
```

Anything else annotated `failure` on a nix leg is a genuine check failure and should be triaged as one. This supersedes both earlier proposals: exit-143-in-the-logs (authoritative but expires) and `null` step conclusions (not necessary, only sufficient).


---

# [Comment #6]() by [gerchowl]()

_Posted on September 29, 2026 at 10:06 AM_

### Third datapoint, from the heaviest branch: the fix holds, but the margin is ~half what #500 suggests

#461 (run 36544027509, `cores=2`) went green on both arches. It is the heaviest branch in the repo, so it bounds the worst case rather than describing a typical one — which makes it the datapoint that should drive this issue, not #500's.

| | peak used | min available | max load1 | duration | outcome |
|---|---|---|---|---|---|
| #500 control (`cores=4`) | 15577 MB | **412 MB** | 13.30 | died at 34m | **EVICTED** |
| #500 treatment (`cores=2`) | 11957 MB | 4032 MB | 9.14 | 84m46s | success |
| **#461 x86_64 (`cores=2`)** | **13534 MB** | **2454 MB** | 8.99 | 85m00s | success |
| #461 aarch64 (`cores=2`) | 11536 MB | 4410 MB | 9.79 | 69m00s | success |

**The headroom conclusion changes.** #461's x86 leg peaks 1.6 GB above #500's treatment and sits only **2.0 GB below the peak that evicted**. Real headroom on the worst branch is **2454 MB**, not the ~4 GB #500 showed. Quoting #500 alone would overstate the room by roughly 2×.

Concretely: the vortex pin (#481) moved peak from ~10 GB to 15.6 GB. A future change of that size would put this branch over the ceiling again, and it would go first.

**Wall-clock:** 85 min against the 120-minute cap — 35 minutes spare, so this stays a watch item rather than a decision. Note it is **not** a `cores=2` slowdown datapoint: this branch's x86 has no completion baseline, because the pre-fix run was itself an eviction (confirmed via the annotation query above). The aarch64 pair **53m → 69m (+30%)** remains the only real cost figure for the fix.

**Disk:** minimum 60.2 GB / 83.0 GB free across 308 samples. Falsified again, on the heaviest branch this time — that hypothesis has now been tested on five legs across three branches and has never once been supported.

**Suggested disposition:** keep open, and treat 2.4 GB as the true margin. The next thing that raises peak memory should be measured on #461's branch specifically, not on a light one.


---

# [Comment #7]() by [gerchowl]()

_Posted on September 29, 2026 at 10:18 AM_

### Recurrence under `--cores 2` — and it is a THIRD failure shape (exit 137, OOM-killer)

#497 run [36545148315](https://github.com/vig-os/tessera/actions/runs/36545148315), aarch64 (job 109329873366), died at 48m with `--cores 2` active. The workflow comment at `nix-check.yml:146` said *"if it does not [stay below 15.6 GB], this model is wrong and `--max-jobs` is next"*. It did not. The model was wrong.

**This is a distinct signature from both earlier shapes:**

```
3234 Killed    nix flake check -L --keep-going --fallback --cores 2
nix flake check exited 137
```

`137 = 128+9 = SIGKILL` — the kernel OOM-killer killed `nix` itself. The earlier evictions were `143 = 128+15 = SIGTERM` (runner agent terminated). The triage rule above needs a third arm: **exit 137 is the kernel killing us; 143 is the runner being reclaimed.** 137 is the more actionable one, because it means we exceeded the ceiling rather than the host deciding to take the box back.

**The trace shows a spike, not accumulation:**

| time | used | avail | load1 |
|---|---|---|---|
| 09:30:17 | 4129 MB | 11818 MB | 8.10 |
| 09:34:17 | 6956 MB | 8991 MB | 8.10 |
| **09:38:17** | **15531 MB** | **416 MB** | 7.68 |

**+8.5 GB inside one 4-minute window.** Baseline had been 3–7 GB for the preceding 45 minutes at a steady load of ~8. Seven derivations were live in that window (`ingest-producer-equality`, `gate-a-sql`, `registry-roundtrip`, `gate-a-workspace-features`, `tessera-py-import`, `tessera-wheel`, `seal-profile-determinism`, `tessera-nextest`), and the log shows what was being compiled as it spiked:

```
09:36:45 tessera-nextest>  Compiling vortex-bytebool v0.75.0 (gerchowl/vortex?rev=608d8a1…)
09:37:10 tessera-nextest>  Compiling vortex-file v0.75.0
09:37:34 tessera-nextest>  Compiling datafusion-functions-nested v54.0.0
09:37:43 tessera-nextest>  Compiling zarrs v0.23.13
09:38:32 Killed
```

**Why `--cores 2` was the wrong lever, stated plainly.** Memory scales with the number of **concurrent derivations** — each is a separate cargo/rustc process tree holding its own heap. `--cores` caps threads *within* one derivation, which is the weaker term: it bought ~24% on a light branch (#500: 15.6 → 12.0 GB) and is now exhausted. Load held at 8.10 with `--cores 2`, i.e. **four derivations still in flight**, which is exactly the quantity that multiplies memory. The comment I wrote reasoned about `--cores` *versus* `--max-jobs` as alternatives and never considered combining them — that framing is the error, not the measurement.

Disk, again: minimum free **81.8 GB**. Sixth leg, fourth branch.


---

# [Comment #8]() by [gerchowl]()

_Posted on September 29, 2026 at 11:43 AM_

### Experiment verdict: `--max-jobs 2 --cores 4` — 3 of 4 legs meet the pre-set bar, all 4 improve on both axes

Run on scratch branches via `workflow_dispatch` (`exp/495-maxjobs2-cores4-on497` / `-on461`, runs 36554974768 / 36554978705); shared CI unchanged. Criteria were fixed **before** the run: **peak < 12288 MB AND wall-clock < 100 min**.

| leg | baseline (`--cores 2`) | experiment (`--max-jobs 2 --cores 4`) | Δ peak | Δ time | verdict |
|---|---|---|---|---|---|
| #497 x86_64 | 82m · 14651 MB · 1338 MB free | **80m · 11171 MB · 4818 MB free** | **−3480 MB (−24%)** | −2m | **PASS** |
| #497 aarch64 | 66m · 11503 MB · 4444 MB free¹ | **54m · 11431 MB · 4516 MB free** | −72 MB | −12m | **PASS** |
| #461 x86_64 | 85m · 13534 MB · 2454 MB free | 76m · **12980 MB** · 3008 MB free | −554 MB | −9m | **FAIL** (over by 692 MB) |
| #461 aarch64 | 69m · 11536 MB · 4410 MB free | **55m · 10690 MB · 5256 MB free** | −846 MB | −14m | **PASS** |

¹ That baseline is **attempt 2**. Attempt 1 of the same run (job 109329873366) was the OOM-kill: 15531 MB, exit 137 at 48 min. So under `--cores 2` this leg was *flaky near the ceiling* rather than deterministically dead — which is the expected signature of a memory-pressure problem, and worth stating because quoting only the successful retry would understate the baseline's instability.

**Report against the bar as written: 3 PASS, 1 FAIL.** #461's x86 leg lands at 12980 MB, 692 MB over the threshold I set in advance. I am not reinterpreting the criterion after seeing the number — that is the whole reason it was fixed beforehand.

**What the data nonetheless shows:** the setting **dominates** the status quo on every leg measured — lower peak *and* lower wall-clock, four for four. The largest win is #497's x86 leg, which went from 1338 MB of headroom (a leg that survived by ~1.3 GB) to 4818 MB, a 3.6× improvement.

**A prediction of mine that was wrong, in the useful direction.** I argued this combination would keep wall-clock *roughly neutral*, reasoning that total CPU stays at ~8 (2×4 vs 4×2). Wall-clock instead improved on all four legs: −2, −12, −9, −14 minutes. The likely mechanism is that lower memory pressure removes reclaim/swap stalls, so the box spends more of its time doing work — i.e. the memory fix paid a wall-clock dividend rather than costing one. Max load1 held at 8.3–8.6 across all four, confirming total CPU was in fact unchanged.

**Recommendation:** adopt `--max-jobs 2 --cores 4`, with the caveat stated rather than buried — **#461's x86 leg does not clear the bar**, and 3.0 GB of headroom on the heaviest branch means the ceiling risk is *reduced, not eliminated*. #506's paths filter does **not** help that branch (it matches the encoder-surface filter, correctly). The remaining levers for it are a larger runner class, or `--max-jobs 2 --cores 2` at a wall-clock cost that these numbers suggest may be smaller than feared.


