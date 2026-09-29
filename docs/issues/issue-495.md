---
type: issue
state: open
created: 2026-09-29T05:43:29Z
updated: 2026-09-29T06:57:32Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/495
comments: 1
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:23.834Z
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

