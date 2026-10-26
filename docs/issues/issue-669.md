---
type: issue
state: open
created: 2026-10-25T11:18:55Z
updated: 2026-10-25T11:18:55Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/669
comments: 0
labels: feature, priority:low, effort:small, area:io
assignees: none
milestone: none
projects: none
parent: 654
children: none
synced: 2026-10-26T07:24:20.967Z
---

# [Issue 669]: [feat(query): float predicate pushdown, gated on a NaN/±0.0 differential test (ADR-0062 §4)](https://github.com/vig-os/tessera/issues/669)

Deferred by ADR-0062 §4 (#654). Float predicates are `Unsupported` in the provider's pushdown class, so float
filters get no `.cidx` or zone-map pruning.

**Why deferred:** Vortex's zone pruning drops rows rather than keeping them, so `Inexact` can't repair a wrong
prune. Its NaN guard applies only when a zone carries a `NaNCount` statistic
(`vortex-array/src/scalar_fn/fns/binary/mod.rs:187-204`). Whether every zone carries it, and whether zone
`min`/`max` order −0.0/+0.0 as IEEE totalOrder does (which DataFusion uses), are unaudited.

**To close:** amend ADR-0062 with a differential test (pushdown on vs off) covering NaN, ±0.0 and ±∞ in every
zone shape the deterministic compressor produces (Pco included), then add `f4`/`f8` to the class.

