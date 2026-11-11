---
type: issue
state: closed
created: 2026-11-11T01:00:28Z
updated: 2026-11-11T05:55:47Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/797
comments: 0
labels: documentation
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:44.086Z
---

# [Issue 797]: [docs(adr): ADR-0064 §8.1 criterion 2 as a within-run (fixed-effects) slope — owner ruling 2026-11-11](https://github.com/vig-os/tessera/issues/797)

## Owner ruling (2026-11-11)
ADR-0064 §8.1 criterion 2 is judged by a **within-run** slope, because the ksb HDD is degraded (exo-fleet#980).

## Why
G r4 (PR #767 @ fa09a81) ran the full sample: W=4 k=3 (N=9) and W=1 k=6 (N=24).
- The across-run OLS slope of peak RssAnon fails at W=4: b̂ 40,437 B/leaf against a bound of 31,109 (SE 12,208). W=1 passes: 5,072 against 10,121.
- Within every W=4 run RssAnon is flat over the tenths of input. Peaks come early (about leaf 7), and the slope against leaves-at-peak is −26,778 ± 46,234.
- The across-run peak tracks how loaded the disk was during each run: stalls hold more payloads in flight, and the long runs overlapped the most contention. A.4 already rules that disk-backed timings are context, not evidence. This extends the same reasoning to peaks driven by the disk.

## Amendment (dated, and fixed before the estimator is computed on the r4 data)
- **Estimator:** fixed effects.
  - y_rt = RssAnon peak in tenth t of run r; x_rt = leaves committed at the end of tenth t.
  - Model y_rt = α_r + b·x_rt + ε: a per-run intercept absorbs that run's I/O state, and the slope b is common.
  - Samples are taken only once x ≥ 16W, the same warm-up threshold as before.
  - The ADR states and justifies the SE method; cluster-robust by run with df = R − 1 is preferred, since tenths within a run are autocorrelated.
- **Pass rule:** unchanged form, b̂ ≤ c_leaf + t₀.₉₅,df·SE, plus the resolution clause 2·t·SE ≤ 121,635 B/leaf. Inconclusive fails.
- **Scope:** the estimator applies to both W=1 and W=4. The across-run slope is still computed and reported as context, labelled as confounded by the disk.
- **Fail-before:** the pre-#759 writer (r1) must still fail.
- No other criterion changes.

