---
type: issue
state: open
created: 2026-11-08T02:43:13Z
updated: 2026-11-08T02:43:13Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/770
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-08T08:33:51.770Z
---

# [Issue 770]: [docs(adr): ADR-0064 §8.1 plateau as a slope test over all runs (second amendment)](https://github.com/vig-os/tessera/issues/770)

Second amendment to ADR-0064 §8/§8.1 (after #760 / PR #764), per the owner's ruling on #715 (2026-11-08).

**Problem.** The plateau criterion (§8.1 criterion 2, amended by #760 A.2) compares pairwise medians of 3 runs at W=4 with τ = 5%. G r3 (`tessera-715-r3-sweep-2`) measured medians of 189.5 / 202.9 / 195.9 MiB at n = 64 / 256 / 1024. These are not monotone in n, and the spread within one point is 8.6%. So the pairwise check fails on noise from concurrent encode peaks rather than on growth.

**Decision (owner).** Replace the pairwise median comparison with a slope test: fit every run's peak RssAnon against n and require the slope ≤ `c_leaf` + a margin derived from the measured noise. Keep criterion 1 (ceiling, max over every run) unchanged.

**Acceptance**
- The amendment states the estimator (e.g. OLS or Theil–Sen over all W=4 runs, n ≥ 16W), the runs it uses, and how the margin is derived (from the slope's standard error or confidence interval, not fitted to make the r3 data pass).
- It shows with the recorded numbers that:
  - the r3 data passes the test;
  - the zone-statistics regression (≈0.058 B/row, +47 MB across the sweep, ADR-0064 §4.2/§8) and the pre-#759 per-leaf cost (≈12.8 KB/leaf) would both fail it.
- It states the power: the smallest per-leaf/per-row growth that fails at k=3 runs per point, and whether k must change.
- Rustdoc, ADR-0026 and FEATURE-MATRIX are consistent where they quote §8.1.
- τ, s, s_rss, the ceilings and the criterion-6 setup are unchanged unless the ADR argues otherwise explicitly.

Refs: #715, #677

